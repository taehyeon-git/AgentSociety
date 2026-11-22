//! Seeded ECS storage. Stable IDs, not ECS allocation order, determine execution.

use agentsociety_core::*;
use bevy_ecs::prelude::{Component, Entity, World as EcsWorld};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

mod restore;

#[derive(Component)]
struct AgentComponent(Agent);

/// Regional production capacity accessible through a city-wide labor abstraction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourcePool {
    /// Stock available to extract this tick (food represents agricultural capacity).
    pub available: Inventory,
    /// Capacity ceiling.
    pub capacity: Inventory,
    /// Per-tick replenishment, including abstract access to new mineral deposits.
    pub regeneration: Inventory,
}

impl ResourcePool {
    /// Replenish without overflowing or exceeding capacity.
    pub fn regenerate(&mut self) -> Result<()> {
        self.regenerate_scaled(&[10000; RESOURCE_COUNT])
    }

    /// Apply temporary basis-point factors without modifying the saved base rates.
    pub fn regenerate_scaled(&mut self, factors: &[u16; RESOURCE_COUNT]) -> Result<()> {
        for resource in Resource::ALL {
            let room = self
                .capacity
                .get(resource)
                .checked_sub(self.available.get(resource))
                .ok_or_else(|| DomainError::Invariant("environment exceeds capacity".into()))?;
            let rate = u128::from(self.regeneration.get(resource))
                * u128::from(factors[resource.index()])
                / 10000;
            self.available.add(
                resource,
                room.min(
                    u64::try_from(rate)
                        .map_err(|_| DomainError::Overflow("scaled regeneration"))?,
                ),
            )?;
        }
        Ok(())
    }
}

/// A snapshot of the city and its agents in stable ID order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSnapshot {
    /// Completed ticks.
    pub tick: Tick,
    /// City width.
    pub width: u16,
    /// City height.
    pub height: u16,
    /// Regional resource capacity, distinct from owned stock.
    pub resource_pool: ResourcePool,
    /// Agents sorted by their stable identifiers.
    pub agents: Vec<Agent>,
}

/// ECS world accessed only by the authoritative single-threaded simulation.
pub struct World {
    ecs: EcsWorld,
    entities: BTreeMap<AgentId, Entity>,
    tick: Tick,
    width: u16,
    height: u16,
    resource_pool: ResourcePool,
}

impl World {
    /// Initialize independent agents with a reproducible ChaCha8 stream.
    pub fn seeded(config: &SimulationConfig) -> Result<Self> {
        config.validate()?;
        let mut rng = ChaCha8Rng::seed_from_u64(config.seed);
        let mut agents = Vec::with_capacity(config.agents);
        for index in 0..config.agents {
            let occupation = [
                Occupation::Farmer,
                Occupation::Producer,
                Occupation::Merchant,
                Occupation::Consumer,
            ][index % 4];
            let goal = [
                Goal::BuildWealth,
                Goal::SecureFood,
                Goal::StableLife,
                Goal::TradeProfit,
            ][rng.random_range(0..4)];
            let mut skills = Skills {
                farming: rng.random_range(10..40),
                manufacturing: rng.random_range(10..40),
                gathering: rng.random_range(20..60),
                trading: rng.random_range(10..40),
            };
            let inventory = match occupation {
                Occupation::Farmer => {
                    skills.farming = rng.random_range(60..=100);
                    Inventory::new([20, 0, 0, 1])
                }
                Occupation::Producer => {
                    skills.manufacturing = rng.random_range(60..=100);
                    Inventory::new([10, 8, 4, 4])
                }
                Occupation::Merchant => {
                    skills.trading = rng.random_range(60..=100);
                    Inventory::new([15, 4, 3, 3])
                }
                Occupation::Consumer => {
                    skills.gathering = rng.random_range(60..=100);
                    Inventory::new([10, 6, 4, 1])
                }
            };
            let variation = config.initial_cash / 5;
            let wallet = config.initial_cash - variation + rng.random_range(0..=variation * 2);
            let account = Account::new(wallet, inventory);
            let goal_progress = GoalProgress::new(account.wealth(&config.initial_prices));
            agents.push(Agent {
                id: AgentId(index as u64),
                name: format!("Agent-{index:04}"),
                age: rng.random_range(18..=65),
                occupation,
                personality: Personality {
                    risk_tolerance: rng.random_range(10..=90),
                    patience: rng.random_range(10..=90),
                },
                goal,
                goal_progress,
                needs: Needs {
                    hunger: 0,
                    energy: rng.random_range(60..=100),
                    comfort: rng.random_range(40..=80),
                },
                account,
                skills,
                position: Position {
                    x: rng.random_range(0..config.map_width),
                    y: rng.random_range(0..config.map_height),
                },
                policy: if index < config.llm_agents {
                    config.policy
                } else {
                    PolicyKind::RuleBased
                },
                current_action: Action::Wait {},
                last_purchase_prices: config.initial_prices,
                history: VecDeque::new(),
                relationships: BTreeMap::new(),
                memories: VecDeque::new(),
                last_external_decision: None,
            });
        }
        Self::from_agents(config, agents)
    }

    /// Construct an initial world from explicit agents (also useful for experiments).
    pub fn from_agents(config: &SimulationConfig, mut agents: Vec<Agent>) -> Result<Self> {
        config.validate()?;
        if agents.len() != config.agents {
            return Err(DomainError::InvalidConfig(
                "agent count differs from configuration".into(),
            ));
        }
        let scale = config.agents.max(1) as u64;
        let capacity = Inventory::new([scale * 30, scale * 20, scale * 15, 0]);
        agents.sort_by_key(|agent| agent.id);
        let mut world = Self {
            ecs: EcsWorld::new(),
            entities: BTreeMap::new(),
            tick: 0,
            width: config.map_width,
            height: config.map_height,
            resource_pool: ResourcePool {
                available: capacity,
                capacity,
                regeneration: Inventory::new([scale * 3, scale * 2, scale, 0]),
            },
        };
        for agent in agents {
            agent.account.validate()?;
            if !world.contains_position(agent.position) {
                return Err(DomainError::InvalidConfig(
                    "agent position outside city".into(),
                ));
            }
            if world.entities.contains_key(&agent.id) {
                return Err(DomainError::DuplicateAgent(agent.id));
            }
            let id = agent.id;
            let entity = world.ecs.spawn(AgentComponent(agent)).id();
            world.entities.insert(id, entity);
        }
        Ok(world)
    }

    /// Number of agents.
    pub fn len(&self) -> usize {
        self.entities.len()
    }
    /// Whether the world contains no agents.
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }
    /// Number of completed ticks.
    pub const fn tick(&self) -> Tick {
        self.tick
    }
    /// Commit a completed tick; only the simulation should call this.
    pub fn advance_tick(&mut self) -> Result<()> {
        self.tick = self
            .tick
            .checked_add(1)
            .ok_or(DomainError::Overflow("tick"))?;
        Ok(())
    }
    /// Agent IDs in canonical order.
    pub fn ids(&self) -> impl ExactSizeIterator<Item = AgentId> + '_ {
        self.entities.keys().copied()
    }
    /// Stable cyclic ID order for bounded observations, without a population-sized scan.
    /// This is an identity ordering, not a measure of physical distance.
    pub fn ids_from(&self, start: AgentId) -> impl Iterator<Item = AgentId> + '_ {
        self.entities
            .range(start..)
            .chain(self.entities.range(..start))
            .map(|(&id, _)| id)
    }
    /// All agents in canonical order.
    pub fn agents(&self) -> impl Iterator<Item = &Agent> {
        self.entities.values().map(|&entity| {
            &self
                .ecs
                .get::<AgentComponent>(entity)
                .expect("private ECS index must remain valid")
                .0
        })
    }
    /// Look up an agent by stable ID.
    pub fn agent(&self, id: AgentId) -> Result<&Agent> {
        let entity = *self
            .entities
            .get(&id)
            .ok_or(DomainError::UnknownAgent(id))?;
        self.ecs
            .get::<AgentComponent>(entity)
            .map(|a| &a.0)
            .ok_or_else(|| DomainError::Invariant("missing ECS component".into()))
    }
    /// Mutate one agent under the engine's exclusive world borrow.
    pub fn with_agent_mut<T>(
        &mut self,
        id: AgentId,
        apply: impl FnOnce(&mut Agent) -> Result<T>,
    ) -> Result<T> {
        let entity = *self
            .entities
            .get(&id)
            .ok_or(DomainError::UnknownAgent(id))?;
        let mut component = self
            .ecs
            .get_mut::<AgentComponent>(entity)
            .ok_or_else(|| DomainError::Invariant("missing ECS component".into()))?;
        apply(&mut component.0)
    }
    /// Mutate two distinct accounts, used for atomic market settlement.
    pub fn with_pair_mut<T>(
        &mut self,
        first: AgentId,
        second: AgentId,
        apply: impl FnOnce(&mut Agent, &mut Agent) -> Result<T>,
    ) -> Result<T> {
        if first == second {
            return Err(DomainError::InvalidAction(
                "self-trading is prohibited".into(),
            ));
        }
        let a = *self
            .entities
            .get(&first)
            .ok_or(DomainError::UnknownAgent(first))?;
        let b = *self
            .entities
            .get(&second)
            .ok_or(DomainError::UnknownAgent(second))?;
        let mut query = self.ecs.query::<&mut AgentComponent>();
        let [mut one, mut two] = query
            .get_many_mut(&mut self.ecs, [a, b])
            .map_err(|error| DomainError::Invariant(error.to_string()))?;
        apply(&mut one.0, &mut two.0)
    }
    /// Check movement bounds.
    pub const fn contains_position(&self, position: Position) -> bool {
        position.x < self.width && position.y < self.height
    }
    /// Horizontal size of the city in cells.
    pub const fn width(&self) -> u16 {
        self.width
    }
    /// Vertical size of the city in cells.
    pub const fn height(&self) -> u16 {
        self.height
    }
    /// Regional capacity, read-only.
    pub const fn resource_pool(&self) -> &ResourcePool {
        &self.resource_pool
    }
    /// Replenish resources at the beginning of a tick.
    pub fn regenerate(&mut self) -> Result<()> {
        self.resource_pool.regenerate()
    }

    /// Replenish under deterministic temporary supply interventions.
    pub fn regenerate_scaled(&mut self, factors: &[u16; RESOURCE_COUNT]) -> Result<()> {
        self.resource_pool.regenerate_scaled(factors)
    }
    /// Transfer environmental capacity to an agent atomically.
    pub fn extract(&mut self, id: AgentId, resource: Resource, quantity: u64) -> Result<()> {
        let mut next_pool = self.resource_pool.available;
        next_pool.remove(resource, quantity)?;
        self.with_agent_mut(id, |agent| agent.account.produce(resource, quantity))?;
        self.resource_pool.available = next_pool;
        Ok(())
    }
    /// Copy serializable state without ECS implementation details.
    pub fn snapshot(&self) -> WorldSnapshot {
        WorldSnapshot {
            tick: self.tick,
            width: self.width,
            height: self.height,
            resource_pool: self.resource_pool.clone(),
            agents: self.agents().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_generation_is_repeatable_and_varied() {
        let config = SimulationConfig::default();
        let one = World::seeded(&config).unwrap().snapshot();
        assert_eq!(one, World::seeded(&config).unwrap().snapshot());
        let other = SimulationConfig { seed: 43, ..config };
        assert_ne!(one, World::seeded(&other).unwrap().snapshot());
        assert_eq!(one.agents.len(), 100);
    }

    #[test]
    fn environment_failure_does_not_credit_agent() {
        let config = SimulationConfig {
            agents: 1,
            ..Default::default()
        };
        let mut world = World::seeded(&config).unwrap();
        let before = world.snapshot();
        assert!(
            world
                .extract(AgentId(0), Resource::Metal, u64::MAX)
                .is_err()
        );
        assert_eq!(world.snapshot(), before);
    }
}
