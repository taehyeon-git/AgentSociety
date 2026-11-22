use crate::{LlmSettings, Simulation, SimulationSnapshot, TickReport};
use agentsociety_core::*;
use serde::{Deserialize, Serialize};

/// Portable external policy state. Credentials are re-read from the named environment variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalPolicyCheckpoint {
    /// Provider configuration and original limits; changing them during resume is rejected.
    pub settings: LlmSettings,
    /// Consumed request budgets and last request ticks.
    pub state: LlmState,
}

/// Application policy state, readable by offline replay without enabling HTTP.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyCheckpoint {
    /// Absent for a run with no external adapter.
    pub external: Option<ExternalPolicyCheckpoint>,
}

impl PolicyCheckpoint {
    /// Check provider settings, bounded counters and agent cooldown references.
    pub fn validate(&self, simulation: &Simulation) -> Result<()> {
        let invalid =
            || DomainError::Invariant("invalid policy checkpoint or request accounting".into());
        let Some(external) = &self.external else {
            return if simulation.config().policy == PolicyKind::RuleBased {
                Ok(())
            } else {
                Err(invalid())
            };
        };
        external.settings.validate()?;
        let state = &external.state;
        let usage = &state.usage;
        let opportunities = simulation
            .world()
            .agents()
            .filter(|agent| agent.policy != PolicyKind::RuleBased)
            .count() as u128
            * u128::from(simulation.world().tick());
        if usage.attempts > external.settings.max_total_calls
            || usage.accepted > usage.attempts
            || usage.usage_reports > usage.attempts
            || state.last_request.len() as u64 > usage.attempts
            || u128::from(usage.accepted) + u128::from(usage.fallbacks) > opportunities
            || u128::from(usage.accepted) + u128::from(usage.fallbacks) < u128::from(usage.attempts)
            || (usage.usage_reports == 0 && (usage.input_tokens != 0 || usage.output_tokens != 0))
            || (usage.attempts == 0 && !state.last_request.is_empty())
        {
            return Err(invalid());
        }
        for (&id, &tick) in &state.last_request {
            let agent = simulation.world().agent(id)?;
            if tick == 0
                || tick > simulation.world().tick()
                || agent.policy == PolicyKind::RuleBased
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    /// Report recorded counters even in an offline replay build.
    pub fn usage(&self) -> LlmUsage {
        self.external
            .as_ref()
            .map_or_else(LlmUsage::default, |external| external.state.usage.clone())
    }

    /// Validate monotonic request accounting against the recorded external results of one tick.
    pub fn validate_transition(
        &self,
        previous: &Self,
        simulation: &Simulation,
        events: &[EventRecord],
    ) -> Result<()> {
        self.validate(simulation)?;
        let invalid = || {
            DomainError::Invariant(
                "policy checkpoint transition disagrees with recorded requests".into(),
            )
        };
        let (Some(current), Some(previous)) = (&self.external, &previous.external) else {
            return if self.external.is_none() && previous.external.is_none() {
                Ok(())
            } else {
                Err(invalid())
            };
        };
        if current.settings != previous.settings {
            return Err(invalid());
        }
        let mut expected = previous.state.clone();
        for record in events {
            if let Event::ExternalDecisionRecorded { result } = &record.event {
                if result.provider != current.settings.provider.name()
                    || result.model != current.settings.model
                    || simulation.world().agent(result.agent_id)?.policy == PolicyKind::RuleBased
                {
                    return Err(invalid());
                }
                if result.outcome != ExternalOutcome::BudgetExhausted {
                    expected.usage.attempts =
                        expected.usage.attempts.checked_add(1).ok_or_else(invalid)?;
                    expected
                        .last_request
                        .insert(result.agent_id, result.target_tick);
                }
                if result.outcome == ExternalOutcome::Accepted {
                    expected.usage.accepted = expected.usage.accepted.saturating_add(1);
                } else {
                    expected.usage.fallbacks = expected.usage.fallbacks.saturating_add(1);
                }
            }
        }
        let a = &current.state.usage;
        let b = &previous.state.usage;
        if current.state.last_request != expected.last_request
            || a.attempts != expected.usage.attempts
            || a.accepted != expected.usage.accepted
            || a.fallbacks != expected.usage.fallbacks
            || a.input_tokens < b.input_tokens
            || a.output_tokens < b.output_tokens
            || a.usage_reports < b.usage_reports
            || a.usage_reports - b.usage_reports > a.attempts - b.attempts
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// A complete restartable run: deterministic engine state plus policy accounting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunCheckpoint {
    /// Envelope version, independently validated before any network client is constructed.
    pub checkpoint_version: u32,
    /// Fully settled engine state.
    pub simulation: SimulationSnapshot,
    /// Optional external policy configuration, usage and cooldowns.
    pub policy: PolicyCheckpoint,
}

impl RunCheckpoint {
    /// Capture a successful tick boundary; live requests are never serialized.
    pub fn capture(simulation: &Simulation, runtime: &PolicyRuntime) -> Result<Self> {
        let policy = runtime.snapshot();
        policy.validate(simulation)?;
        Ok(Self {
            checkpoint_version: 1,
            simulation: simulation.snapshot()?,
            policy,
        })
    }

    /// Validate the complete checkpoint and restore only the engine, without HTTP or credentials.
    pub fn restore_engine(&self) -> Result<Simulation> {
        if self.checkpoint_version != 1 {
            return Err(DomainError::InvalidConfig(
                "unsupported checkpoint version".into(),
            ));
        }
        let simulation = Simulation::from_snapshot(self.simulation.clone())?;
        self.policy.validate(&simulation)?;
        Ok(simulation)
    }

    /// Restore the engine and its optional adapter without issuing a model request.
    pub fn restore(&self) -> Result<(Simulation, PolicyRuntime)> {
        let simulation = self.restore_engine()?;
        let runtime = PolicyRuntime::from_checkpoint(&self.policy, &simulation)?;
        Ok((simulation, runtime))
    }
}

#[derive(Default)]
enum Mode {
    #[default]
    Rules,
    #[cfg(feature = "llm")]
    Llm(Box<ExternalRuntime>),
}

#[cfg(feature = "llm")]
struct ExternalRuntime {
    runtime: tokio::runtime::Runtime,
    policy: agentsociety_cognition::llm::LlmPolicy,
}

/// Application-side decision runtime. The authoritative engine remains synchronous.
///
/// Desktop callers run this on their worker thread; the UI never waits for HTTP.
#[derive(Default)]
pub struct PolicyRuntime {
    mode: Mode,
}

impl PolicyRuntime {
    /// Persist provider configuration and consumed budgets at a completed tick boundary.
    pub fn snapshot(&self) -> PolicyCheckpoint {
        match &self.mode {
            Mode::Rules => PolicyCheckpoint::default(),
            #[cfg(feature = "llm")]
            Mode::Llm(external) => PolicyCheckpoint {
                external: Some(ExternalPolicyCheckpoint {
                    settings: external.policy.settings().clone(),
                    state: external.policy.snapshot(),
                }),
            },
        }
    }

    /// Restore the original provider and limits; resume never grants a fresh request budget.
    pub fn from_checkpoint(checkpoint: &PolicyCheckpoint, simulation: &Simulation) -> Result<Self> {
        checkpoint.validate(simulation)?;
        let Some(external) = &checkpoint.external else {
            return Ok(Self::default());
        };
        #[cfg(feature = "llm")]
        {
            let mut runtime = Self::with_llm(external.settings.clone())?;
            if let Mode::Llm(inner) = &mut runtime.mode {
                inner.policy.restore_state(external.state.clone())?;
            }
            Ok(runtime)
        }
        #[cfg(not(feature = "llm"))]
        {
            let _ = external;
            Err(DomainError::InvalidConfig("resuming an external policy requires --features llm; offline replay remains available".into()))
        }
    }

    /// Construct optional HTTP cognition without contacting a model.
    #[cfg(feature = "llm")]
    pub fn with_llm(settings: crate::LlmSettings) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| {
                DomainError::InvalidConfig("cannot initialize async cognition runtime".into())
            })?;
        let policy = agentsociety_cognition::llm::LlmPolicy::new(settings)?;
        Ok(Self {
            mode: Mode::Llm(Box::new(ExternalRuntime { runtime, policy })),
        })
    }

    /// Fail explicitly when a run requests an external policy without an adapter.
    pub fn validate(&self, config: &SimulationConfig) -> Result<()> {
        if config.policy != PolicyKind::RuleBased && matches!(self.mode, Mode::Rules) {
            return Err(DomainError::InvalidConfig(
                "LLM/hybrid policy requires an llm-enabled build and --llm-config".into(),
            ));
        }
        Ok(())
    }

    /// Advance one tick, falling back to rules on provider failures or exhausted budgets.
    pub fn step(&mut self, simulation: &mut Simulation) -> Result<TickReport> {
        self.validate(simulation.config())?;
        if simulation.failed {
            return Err(DomainError::Invariant(
                "cannot request decisions for a failed simulation".into(),
            ));
        }
        match &mut self.mode {
            Mode::Rules => simulation.step(),
            #[cfg(feature = "llm")]
            Mode::Llm(external) => {
                let started = std::time::Instant::now();
                let batch = external
                    .runtime
                    .block_on(external.policy.prepare_with_society(
                        simulation.world(),
                        *simulation.market.prices(),
                        simulation.stats().flows.buy_demand,
                        simulation.stats().flows.sell_supply,
                        agentsociety_cognition::llm::SocietyContext {
                            proposals: simulation.proposals(),
                            interactions: &simulation.config().interactions,
                            institutions: simulation.institutions(),
                            organizations: &simulation.config().organizations,
                            scenario: simulation.scenario(),
                        },
                    ))?;
                let external_time = started.elapsed();
                let mut report = simulation.step_with_decision_batch(&batch)?;
                report.timing.external = external_time;
                report.timing.total = started.elapsed();
                Ok(report)
            }
        }
    }

    /// Usage for this run; token totals include only responses that report them.
    pub fn usage(&self) -> LlmUsage {
        match &self.mode {
            Mode::Rules => LlmUsage::default(),
            #[cfg(feature = "llm")]
            Mode::Llm(external) => external.policy.usage().clone(),
        }
    }

    /// Reset request budgets and cooldowns after a successful explicit restart.
    pub fn reset(&mut self) {
        #[cfg(feature = "llm")]
        if let Mode::Llm(external) = &mut self.mode {
            external.policy.reset();
        }
    }
}
