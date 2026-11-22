use super::*;
use std::collections::BTreeSet;

impl World {
    /// Restore a completed world after validating identities, bounds and references.
    /// No ECS entities are exposed until the entire input has passed validation.
    pub fn from_snapshot(config: &SimulationConfig, snapshot: WorldSnapshot) -> Result<Self> {
        config.validate()?;
        let invalid =
            |message: &str| DomainError::Invariant(format!("invalid world snapshot: {message}"));
        if snapshot.width != config.map_width
            || snapshot.height != config.map_height
            || snapshot.agents.len() != config.agents
        {
            return Err(invalid("dimensions or population differ from config"));
        }
        let ids: BTreeSet<_> = snapshot.agents.iter().map(|agent| agent.id).collect();
        if ids.len() != snapshot.agents.len()
            || !snapshot
                .agents
                .windows(2)
                .all(|pair| pair[0].id < pair[1].id)
        {
            return Err(invalid("agents must have unique sorted IDs"));
        }
        let scale = config.agents.max(1) as u64;
        if snapshot.resource_pool.capacity
            != Inventory::new([scale * 30, scale * 20, scale * 15, 0])
            || snapshot.resource_pool.regeneration
                != Inventory::new([scale * 3, scale * 2, scale, 0])
            || Resource::ALL.iter().any(|&r| {
                snapshot.resource_pool.available.get(r) > snapshot.resource_pool.capacity.get(r)
            })
        {
            return Err(invalid("invalid environmental capacity or regeneration"));
        }
        for agent in &snapshot.agents {
            agent.account.validate()?;
            // Live proposal collateral is attributed by Simulation after the book is restored.
            if [
                agent.personality.risk_tolerance,
                agent.personality.patience,
                agent.skills.farming,
                agent.skills.manufacturing,
                agent.skills.gathering,
                agent.skills.trading,
                agent.needs.hunger,
                agent.needs.energy,
                agent.needs.comfort,
            ]
            .iter()
            .any(|&value| value > 100)
                || agent
                    .last_purchase_prices
                    .iter()
                    .any(|&price| price == 0 || price > MAX_PRICE)
                || agent.name.is_empty()
                || agent.name.chars().count() > 128
            {
                return Err(invalid("invalid bounded agent state"));
            }
            let history_len = snapshot.tick.min(config.history_capacity as u64) as usize;
            if agent.history.len() != history_len {
                return Err(invalid("incomplete or oversized action history"));
            }
            for (index, entry) in agent.history.iter().enumerate() {
                if entry.tick != snapshot.tick - history_len as u64 + index as u64 + 1
                    || entry.accepted != entry.rejection.is_none()
                    || entry
                        .rejection
                        .as_ref()
                        .is_some_and(|reason| reason.len() > 2048)
                {
                    return Err(invalid("inconsistent action history"));
                }
            }
            if agent
                .history
                .back()
                .is_some_and(|last| last.decision.action != agent.current_action)
            {
                return Err(invalid("current action differs from history"));
            }
            if agent.relationships.len() > config.relationship_capacity
                || agent.memories.len() > config.memory_capacity
            {
                return Err(invalid("retention capacity exceeded"));
            }
            for (&other, relationship) in &agent.relationships {
                if other == agent.id
                    || !ids.contains(&other)
                    || !valid_relationship(relationship, snapshot.tick)
                {
                    return Err(invalid("invalid relationship"));
                }
            }
            let mut previous = None;
            for memory in &agent.memories {
                if memory.agent_id != agent.id
                    || memory.tick > snapshot.tick
                    || memory.importance > 100
                    || previous.is_some_and(|(id, tick)| memory.id <= id || memory.tick < tick)
                    || memory
                        .content
                        .related_agent()
                        .is_some_and(|other| other == agent.id || !ids.contains(&other))
                {
                    return Err(invalid("invalid memory identity, order or timestamp"));
                }
                let valid_position = |position: &Position| {
                    position.x < snapshot.width && position.y < snapshot.height
                };
                let valid_content = match &memory.content {
                    MemoryContent::GoalAssigned { .. } => {
                        memory.tick == 0 && memory.id.0 == agent.id.0
                    }
                    MemoryContent::GoalChanged { previous, goal, .. } => {
                        memory.tick > 0 && previous != goal
                    }
                    MemoryContent::Proposal { terms, .. } => match terms {
                        ProposalTerms::Trade {
                            quantity,
                            unit_price,
                            ..
                        } => {
                            (1..=MAX_ORDER_QUANTITY).contains(quantity)
                                && (1..=MAX_PRICE).contains(unit_price)
                        }
                        ProposalTerms::Cooperation { producer_share } => {
                            (1..COOPERATIVE_GOODS).contains(producer_share)
                        }
                    },
                    MemoryContent::Cooperated { received_goods, .. } => {
                        (1..COOPERATIVE_GOODS).contains(received_goods)
                    }
                    MemoryContent::Produced { quantity, .. }
                    | MemoryContent::Consumed { quantity, .. } => *quantity > 0,
                    MemoryContent::Moved { from, to } => valid_position(from) && valid_position(to),
                    MemoryContent::Trade {
                        quantity, price, ..
                    } => {
                        *quantity > 0
                            && *quantity <= MAX_ORDER_QUANTITY
                            && *price > 0
                            && *price <= MAX_PRICE
                    }
                    MemoryContent::RelationshipChanged { relationship, .. } => {
                        valid_relationship(relationship, memory.tick)
                            && relationship.last_interaction == memory.tick
                    }
                    MemoryContent::ActionRejected { reason } => reason.len() <= 2048,
                    MemoryContent::RelationshipForgotten { .. } => true,
                    MemoryContent::Organization { event } => {
                        valid_owners(&event.participants(), agent.id, config.agents)
                    }
                    MemoryContent::Contract { event } => {
                        valid_owners(event.participants(), agent.id, config.agents)
                    }
                    MemoryContent::Scenario { event } => {
                        valid_owners(&event.participants(), agent.id, config.agents)
                    }
                };
                if !valid_content {
                    return Err(invalid("invalid memory content"));
                }
                previous = Some((memory.id, memory.tick));
            }
            if let Some(result) = &agent.last_external_decision
                && (result.agent_id != agent.id
                    || result.target_tick == 0
                    || result.target_tick > snapshot.tick
                    || (result.outcome == ExternalOutcome::Accepted) != result.decision.is_some()
                    || result.summary.chars().count() > 512
                    || result.model.is_empty()
                    || result.model.chars().count() > 128
                    || !matches!(result.provider.as_str(), "ollama" | "openai_compatible"))
            {
                return Err(invalid("invalid external decision"));
            }
        }
        let mut world = Self::from_agents(config, snapshot.agents)?;
        world.tick = snapshot.tick;
        world.resource_pool = snapshot.resource_pool;
        Ok(world)
    }
}

fn valid_relationship(edge: &Relationship, tick: Tick) -> bool {
    (-100..=100).contains(&edge.trust)
        && edge.familiarity <= 100
        && (-100..=100).contains(&edge.cooperation_score)
        && edge.last_interaction > 0
        && edge.last_interaction <= tick
        && edge.interaction_count > 0
}

fn valid_owners(owners: &[AgentId], owner: AgentId, population: usize) -> bool {
    !owners.is_empty()
        && owners.len() <= 32
        && owners.contains(&owner)
        && owners.iter().all(|id| id.0 < population as u64)
        && owners
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == owners.len()
}
