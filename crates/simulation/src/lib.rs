//! Single-writer tick orchestration; policy selection is separate from execution.

use agentsociety_analytics::{
    ConservationBaseline, EconomyStats, FlowTotals, audit_balances, collect, collect_with_ownership,
};
use agentsociety_cognition::{DecisionPolicy, Observation, RuleBasedPolicy};
use agentsociety_core::*;
use agentsociety_economy::{
    InstitutionBook, InstitutionSnapshot, MarketSnapshot, OrderBook, ProposalBook,
    ProposalSnapshot, apply_action, begin_scenario, settle_fiscal, update_needs, validate_scenario,
};
use agentsociety_world::{World, WorldSnapshot};
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

mod adaptation;
mod replay;
mod restore;
mod runtime;
pub use agentsociety_cognition::{LlmSettings, Provider};
pub use runtime::{ExternalPolicyCheckpoint, PolicyCheckpoint, PolicyRuntime, RunCheckpoint};

/// Portable ChaCha8 position, independent of internal buffered RNG layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RandomState {
    /// Complete generator seed.
    pub seed: [u8; 32],
    /// ChaCha stream number.
    pub stream: u64,
    /// Offset in 32-bit words.
    pub word_position: u128,
}

/// Deterministic export of a completed state. Timing is intentionally excluded.
///
/// Restored by [`Simulation::from_snapshot`]. Application request budgets are
/// stored separately in [`RunCheckpoint`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationSnapshot {
    /// Serialization schema version.
    pub format_version: u32,
    /// Exact rules version required for restore and replay.
    pub engine_version: String,
    /// Initial reproducible configuration.
    pub config: SimulationConfig,
    /// Complete world state with sorted agents.
    pub world: WorldSnapshot,
    /// Market state at a fully settled tick boundary.
    pub market: MarketSnapshot,
    /// Live bilateral proposals and their monotonic identifier; collateral stays in accounts.
    pub proposals: ProposalSnapshot,
    /// Shared ownership, funded recurring agreements and retired accounting history.
    pub institutions: InstitutionSnapshot,
    /// Scheduler cursor, active shocks, fiscal policy and government ledger.
    pub scenario: ScenarioState,
    /// Conservation reference established at initialization.
    pub baseline: ConservationBaseline,
    /// Total flows since initialization.
    pub cumulative: FlowTotals,
    /// Statistics at the latest completed tick.
    pub stats: EconomyStats,
    /// Next global event sequence.
    pub next_event_sequence: u64,
    /// Position of the deterministic policy tie-breaker stream.
    pub random: RandomState,
}

/// Nondeterministic performance observations, kept outside state and event logs.
#[derive(Debug, Clone, Copy, Default)]
pub struct StepTiming {
    /// Full tick, including decisions, accounting, and audits.
    pub total: Duration,
    /// Matching, settlement, and order expiry.
    pub market: Duration,
    /// Resource regeneration and canonical ID collection.
    pub environment: Duration,
    /// Read-only policy selection and external result recording.
    pub decisions: Duration,
    /// Intent validation, reservation, production and history updates.
    pub actions: Duration,
    /// Household consumption and need updates.
    pub household: Duration,
    /// Sales taxes, periodic cash levies and funded redistribution.
    pub fiscal: Duration,
    /// Trade-derived social feedback.
    pub social: Duration,
    /// Flow aggregation, statistics and conservation audits.
    pub analytics: Duration,
    /// Event sequencing and bounded memory updates.
    pub memory: Duration,
    /// Optional external request preparation and waiting; zero in the engine alone.
    pub external: Duration,
    /// Outcome aggregation and periodic goal review.
    pub adaptation: Duration,
}

impl StepTiming {
    /// Accumulate wall-clock stage measurements without altering deterministic state.
    pub fn accumulate(&mut self, other: Self) {
        for (total, value) in [
            (&mut self.total, other.total),
            (&mut self.market, other.market),
            (&mut self.environment, other.environment),
            (&mut self.decisions, other.decisions),
            (&mut self.actions, other.actions),
            (&mut self.household, other.household),
            (&mut self.fiscal, other.fiscal),
            (&mut self.social, other.social),
            (&mut self.analytics, other.analytics),
            (&mut self.memory, other.memory),
            (&mut self.external, other.external),
            (&mut self.adaptation, other.adaptation),
        ] {
            *total = total.saturating_add(value);
        }
    }

    /// Mean wall-clock milliseconds per stage; these are not allocation or CPU samples.
    pub fn mean_ms(&self, ticks: u64) -> BTreeMap<&'static str, f64> {
        [
            ("environment", self.environment),
            ("decisions", self.decisions),
            ("actions", self.actions),
            ("market", self.market),
            ("household", self.household),
            ("fiscal", self.fiscal),
            ("social", self.social),
            ("analytics", self.analytics),
            ("memory", self.memory),
            ("external", self.external),
            ("adaptation", self.adaptation),
        ]
        .into_iter()
        .map(|(name, duration)| (name, duration.as_secs_f64() * 1000.0 / ticks.max(1) as f64))
        .collect()
    }
}

/// Results of one fully settled and audited tick.
pub struct TickReport {
    /// Ordered structured events for this tick only.
    pub events: Vec<EventRecord>,
    /// Economic measurements at the tick boundary.
    pub stats: EconomyStats,
    /// Wall-clock performance, excluded from deterministic output.
    pub timing: StepTiming,
}

/// The only owner allowed to advance authoritative simulation state.
pub struct Simulation {
    config: SimulationConfig,
    world: World,
    market: OrderBook,
    proposals: ProposalBook,
    institutions: InstitutionBook,
    scenario: ScenarioState,
    rng: ChaCha8Rng,
    baseline: ConservationBaseline,
    cumulative: FlowTotals,
    stats: EconomyStats,
    initial_events: Vec<EventRecord>,
    next_event_sequence: u64,
    failed: bool,
}

impl Simulation {
    /// Create a reproducible world, without any network access or background jobs.
    pub fn new(config: SimulationConfig) -> Result<Self> {
        let mut world = World::seeded(&config)?;
        let market = OrderBook::new(config.initial_prices)?;
        let stats = collect(&world, config.initial_prices, FlowTotals::default())?;
        let baseline = (&stats).into();
        let initial_events: Vec<_> = world
            .agents()
            .enumerate()
            .map(|(sequence, agent)| EventRecord {
                sequence: sequence as u64,
                tick: 0,
                event: Event::AgentCreated {
                    agent_id: agent.id,
                    occupation: agent.occupation,
                    goal: agent.goal,
                },
            })
            .collect();
        let next_event_sequence = initial_events.len() as u64;
        agentsociety_memory::record_events(&mut world, &initial_events, config.memory_capacity)?;
        // Separate streams keep changes to initialization from shifting every later tie breaker.
        let mut rng = ChaCha8Rng::seed_from_u64(config.seed);
        rng.set_stream(1);
        Ok(Self {
            config,
            world,
            market,
            proposals: ProposalBook::default(),
            institutions: InstitutionBook::default(),
            scenario: ScenarioState::default(),
            rng,
            baseline,
            cumulative: FlowTotals::default(),
            stats,
            initial_events,
            next_event_sequence,
            failed: false,
        })
    }

    /// Read-only access to current agents and environmental capacity.
    pub const fn world(&self) -> &World {
        &self.world
    }
    /// Read-only live proposal state for policies and inspectors.
    pub const fn proposals(&self) -> &ProposalBook {
        &self.proposals
    }
    /// Read-only cooperative property and recurring delivery state.
    pub const fn institutions(&self) -> &InstitutionBook {
        &self.institutions
    }
    /// Current environmental and government state.
    pub const fn scenario(&self) -> &ScenarioState {
        &self.scenario
    }
    /// Initial configuration.
    pub const fn config(&self) -> &SimulationConfig {
        &self.config
    }
    /// Latest completed economic measurements.
    pub const fn stats(&self) -> &EconomyStats {
        &self.stats
    }
    /// Lifetime accepted flows, useful for experiments and summaries.
    pub const fn cumulative(&self) -> &FlowTotals {
        &self.cumulative
    }
    /// Initialization events, emitted once at tick zero.
    pub fn initial_events(&self) -> &[EventRecord] {
        &self.initial_events
    }

    /// Advance one tick with the offline policy.
    pub fn step(&mut self) -> Result<TickReport> {
        self.run_step(&RuleBasedPolicy, &BTreeMap::new(), None)
    }

    /// Advance one tick with a custom read-only policy, still validating all intents.
    pub fn step_with_policy(&mut self, policy: &dyn DecisionPolicy) -> Result<TickReport> {
        self.run_step(policy, &BTreeMap::new(), None)
    }

    /// Override individual decisions through a deterministic, logged input boundary.
    ///
    /// Unknown agent IDs reject the entire input before mutation. Invalid actions
    /// become rejection events. RNG draws are identical with or without overrides.
    pub fn step_with_overrides(
        &mut self,
        overrides: &BTreeMap<AgentId, Decision>,
    ) -> Result<TickReport> {
        self.run_step(&RuleBasedPolicy, overrides, None)
    }

    /// Apply recorded external results without a network request, or replay the same batch.
    ///
    /// Stale ticks, unknown identities, and inconsistent outcomes are rejected before
    /// mutation. Accepted intents still pass the ordinary economic validation path.
    pub fn step_with_decision_batch(&mut self, batch: &DecisionBatch) -> Result<TickReport> {
        let next_tick = self
            .world
            .tick()
            .checked_add(1)
            .ok_or(DomainError::Overflow("target tick"))?;
        if batch.target_tick != next_tick {
            return Err(DomainError::InvalidAction(
                "external batch targets a different tick".into(),
            ));
        }
        let mut overrides = BTreeMap::new();
        for (&id, result) in &batch.decisions {
            self.world.agent(id)?;
            if result.agent_id != id
                || result.target_tick != next_tick
                || (result.outcome == ExternalOutcome::Accepted) != result.decision.is_some()
                || result.summary.chars().count() > 512
                || result.model.chars().count() > 128
                || !matches!(result.provider.as_str(), "ollama" | "openai_compatible")
            {
                return Err(DomainError::InvalidAction(
                    "inconsistent external decision record".into(),
                ));
            }
            if let Some(decision) = &result.decision {
                overrides.insert(id, decision.clone());
            }
        }
        self.run_step(&RuleBasedPolicy, &overrides, Some(batch))
    }

    fn run_step(
        &mut self,
        policy: &dyn DecisionPolicy,
        overrides: &BTreeMap<AgentId, Decision>,
        batch: Option<&DecisionBatch>,
    ) -> Result<TickReport> {
        if self.failed {
            return Err(DomainError::Invariant(
                "simulation stopped after a failed tick; create a new run".into(),
            ));
        }
        for &id in overrides.keys() {
            self.world.agent(id)?;
        }
        let result = self.step_inner(policy, overrides, batch);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn step_inner(
        &mut self,
        policy: &dyn DecisionPolicy,
        overrides: &BTreeMap<AgentId, Decision>,
        batch: Option<&DecisionBatch>,
    ) -> Result<TickReport> {
        let started = Instant::now();
        let mut timing = StepTiming::default();
        let tick = self
            .world
            .tick()
            .checked_add(1)
            .ok_or(DomainError::Overflow("tick"))?;
        let mut events = begin_scenario(
            &mut self.world,
            &mut self.institutions,
            &self.config.scenario,
            &mut self.scenario,
            tick,
        )?;
        let ids: Vec<_> = self.world.ids().collect();
        timing.environment = started.elapsed();
        let stage_started = Instant::now();
        let mut decisions = Vec::with_capacity(ids.len());
        events.reserve(ids.len() * 5 + 1);
        if let Some(batch) = batch {
            for (&id, result) in &batch.decisions {
                self.world.with_agent_mut(id, |agent| {
                    agent.last_external_decision = Some(result.clone());
                    Ok(())
                })?;
                events.push(Event::ExternalDecisionRecorded {
                    result: result.clone(),
                });
            }
        }
        for &id in &ids {
            let entropy = self.rng.next_u64();
            let (selected, origin) = if let Some(external) = overrides.get(&id) {
                (external.clone(), DecisionOrigin::ExternalInput)
            } else {
                let interactions =
                    self.proposals
                        .observe(&self.world, &self.config.interactions, id, tick)?;
                let institutions =
                    self.institutions
                        .observe(&self.world, &self.config.organizations, id, tick)?;
                let obs = Observation {
                    agent: self.world.agent(id)?,
                    tick,
                    prices: self.market.prices(),
                    anchor_prices: &self.config.initial_prices,
                    demand: &self.stats.flows.buy_demand,
                    supply: &self.stats.flows.sell_supply,
                    entropy,
                    interactions: interactions.as_ref(),
                    institutions: institutions.as_ref(),
                };
                (policy.decide(&obs), DecisionOrigin::Policy)
            };
            events.push(Event::DecisionRecorded {
                agent_id: id,
                decision: selected.clone(),
                origin,
            });
            decisions.push((id, selected));
        }

        timing.decisions = stage_started.elapsed();
        let stage_started = Instant::now();
        // Rotate submission priority so low agent IDs do not always become makers.
        if !decisions.is_empty() {
            let offset = ((tick - 1) % decisions.len() as u64) as usize;
            decisions.rotate_left(offset);
        }
        // All policies observe the same completed boundary. Expired collateral is
        // released before any intent executes, so action ordering cannot revive it.
        events.extend(self.proposals.expire(&mut self.world, tick)?);
        events.extend(self.institutions.expire(&mut self.world, tick)?);
        for (id, selected) in decisions {
            let applied = if let Action::Institution { operation } = &selected.action {
                self.institutions.apply(
                    &mut self.world,
                    &self.config.organizations,
                    id,
                    operation,
                    tick,
                )
            } else if selected.action.is_interaction() {
                self.proposals.apply(
                    &mut self.world,
                    &self.config.interactions,
                    id,
                    &selected.action,
                    tick,
                )
            } else {
                apply_action(
                    &mut self.world,
                    &mut self.market,
                    id,
                    &selected.action,
                    tick,
                )
            };
            let (accepted, rejection) = match applied {
                Ok(mut accepted_events) => {
                    events.append(&mut accepted_events);
                    (true, None)
                }
                Err(
                    error @ (DomainError::InvalidAction(_)
                    | DomainError::InsufficientFunds
                    | DomainError::InsufficientInventory(_)),
                ) => {
                    let reason = error.to_string();
                    events.push(Event::ActionRejected {
                        agent_id: id,
                        reason: reason.clone(),
                    });
                    (false, Some(reason))
                }
                Err(error) => return Err(error),
            };
            self.world.with_agent_mut(id, |agent| {
                agent.current_action = selected.action.clone();
                if self.config.history_capacity > 0 {
                    while agent.history.len() >= self.config.history_capacity {
                        agent.history.pop_front();
                    }
                    agent.history.push_back(ActionRecord {
                        tick,
                        decision: selected,
                        accepted,
                        rejection,
                    });
                }
                Ok(())
            })?;
        }

        events.extend(self.institutions.produce(
            &mut self.world,
            &self.config.organizations,
            tick,
        )?);
        timing.actions = stage_started.elapsed();
        let market_started = Instant::now();
        events.extend(self.market.match_orders(&mut self.world)?);
        events.extend(self.market.expire_all(&mut self.world)?);
        events.extend(self.institutions.settle_due(&mut self.world, tick)?);
        if self.config.interactions.negotiation || self.config.organizations.enabled {
            self.market.include_negotiated_trades(&events)?;
        }
        timing.market = market_started.elapsed();
        let stage_started = Instant::now();
        for id in ids {
            events.extend(update_needs(&mut self.world, id, tick)?);
        }
        timing.household = stage_started.elapsed();
        let stage_started = Instant::now();
        events.extend(settle_fiscal(
            &mut self.world,
            &mut self.institutions,
            &mut self.scenario,
            tick,
            &events,
        )?);
        timing.fiscal = stage_started.elapsed();
        let stage_started = Instant::now();
        let social = agentsociety_social::process(
            &mut self.world,
            &events,
            tick,
            self.config.relationship_capacity,
        )?;
        events.extend(social);
        timing.social = stage_started.elapsed();
        let stage_started = Instant::now();
        events.extend(self.review_goals(&events, tick)?);
        timing.adaptation = stage_started.elapsed();
        let stage_started = Instant::now();
        let flows = FlowTotals::from_events(&events)?;
        self.cumulative.accumulate(&flows)?;
        let groups: Vec<_> = self.institutions.organizations().collect();
        let mut stats = collect_with_ownership(
            &self.world,
            *self.market.prices(),
            flows,
            &groups,
            &self.scenario.treasury,
            self.institutions.contracts().len(),
        )?;
        stats.tick = tick;
        audit_balances(&self.world, &stats, &self.baseline, &self.cumulative)?;
        self.proposals
            .audit_with_cash(&self.world, &self.institutions.personal_collateral()?)?;
        self.institutions.audit_groups()?;
        restore::audit_ledger_totals(
            &self.world,
            &self.institutions,
            &self.scenario.treasury,
            &self.cumulative,
        )?;
        self.world.advance_tick()?;
        events.push(Event::TickCompleted {
            prices: stats.prices,
            trades: stats.flows.trade_count,
            total_money: stats.total_money,
        });

        timing.analytics = stage_started.elapsed();
        let stage_started = Instant::now();
        let start_sequence = self.next_event_sequence;
        self.next_event_sequence = start_sequence
            .checked_add(events.len() as u64)
            .ok_or(DomainError::Overflow("event sequence"))?;
        let records: Vec<_> = events
            .into_iter()
            .enumerate()
            .map(|(i, event)| EventRecord {
                sequence: start_sequence + i as u64,
                tick,
                event,
            })
            .collect();
        agentsociety_memory::record_events(&mut self.world, &records, self.config.memory_capacity)?;
        timing.memory = stage_started.elapsed();
        tracing::debug!(
            tick,
            trades = stats.flows.trade_count,
            hungry = stats.hungry_agents,
            "tick completed"
        );
        self.stats = stats.clone();
        timing.total = started.elapsed();
        Ok(TickReport {
            events: records,
            stats,
            timing,
        })
    }

    /// Export a fully settled, deterministic state for inspection and comparison.
    pub fn snapshot(&self) -> Result<SimulationSnapshot> {
        if self.failed {
            return Err(DomainError::Invariant(
                "cannot export an incomplete failed tick".into(),
            ));
        }
        Ok(SimulationSnapshot {
            format_version: FORMAT_VERSION,
            engine_version: ENGINE_VERSION.into(),
            config: self.config.clone(),
            world: self.world.snapshot(),
            market: self.market.snapshot()?,
            proposals: self.proposals.snapshot(),
            institutions: self.institutions.snapshot(),
            scenario: self.scenario.clone(),
            baseline: self.baseline.clone(),
            cumulative: self.cumulative.clone(),
            stats: self.stats.clone(),
            next_event_sequence: self.next_event_sequence,
            random: RandomState {
                seed: self.rng.get_seed(),
                stream: self.rng.get_stream(),
                word_position: self.rng.get_word_pos(),
            },
        })
    }
}
