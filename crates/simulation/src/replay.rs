use super::*;
use std::collections::BTreeSet;

struct RecordedPolicy(BTreeMap<AgentId, Decision>);
impl DecisionPolicy for RecordedPolicy {
    fn decide(&self, observation: &Observation<'_>) -> Decision {
        self.0[&observation.agent.id].clone()
    }
}

impl Simulation {
    /// Reuse recorded intents and compare every regenerated event, without HTTP.
    /// Input framing errors leave the engine unchanged. A replay mismatch poisons
    /// the candidate run so its divergent partial state cannot be exported or advanced.
    pub fn replay_tick(&mut self, expected: &[EventRecord]) -> Result<TickReport> {
        let invalid =
            |message: &str| DomainError::Invariant(format!("invalid replay tick: {message}"));
        let tick = self
            .world
            .tick()
            .checked_add(1)
            .ok_or(DomainError::Overflow("replay tick"))?;
        if expected.is_empty()
            || !matches!(
                expected.last().map(|r| &r.event),
                Some(Event::TickCompleted { .. })
            )
        {
            return Err(invalid("missing tick completion"));
        }
        let mut seen = BTreeSet::new();
        let mut policy = RecordedPolicy(BTreeMap::new());
        let mut overrides = BTreeMap::new();
        let mut batch = DecisionBatch {
            target_tick: tick,
            decisions: BTreeMap::new(),
        };
        for (index, record) in expected.iter().enumerate() {
            if record.tick != tick
                || self.next_event_sequence.checked_add(index as u64) != Some(record.sequence)
                || (index + 1 < expected.len()
                    && matches!(record.event, Event::TickCompleted { .. }))
            {
                return Err(invalid(
                    "noncontiguous sequence, wrong tick or premature completion",
                ));
            }
            match &record.event {
                Event::DecisionRecorded {
                    agent_id,
                    decision,
                    origin,
                } => {
                    self.world.agent(*agent_id)?;
                    if !seen.insert(*agent_id) {
                        return Err(invalid("duplicate decision"));
                    }
                    match origin {
                        DecisionOrigin::Policy => {
                            policy.0.insert(*agent_id, decision.clone());
                        }
                        DecisionOrigin::ExternalInput => {
                            overrides.insert(*agent_id, decision.clone());
                        }
                    }
                }
                Event::ExternalDecisionRecorded { result } => {
                    self.world.agent(result.agent_id)?;
                    if result.target_tick != tick
                        || batch
                            .decisions
                            .insert(result.agent_id, result.clone())
                            .is_some()
                    {
                        return Err(invalid("duplicate or stale external result"));
                    }
                }
                Event::AgentCreated { .. } => return Err(invalid("initialization inside a tick")),
                _ => {}
            }
        }
        if seen.len() != self.world.len() {
            return Err(invalid("missing agent decision"));
        }
        for (&id, external) in &batch.decisions {
            if (external.outcome == ExternalOutcome::Accepted) != external.decision.is_some()
                || external
                    .decision
                    .as_ref()
                    .is_some_and(|decision| overrides.get(&id) != Some(decision))
                || external.summary.chars().count() > 512
                || external.model.chars().count() > 128
                || !matches!(external.provider.as_str(), "ollama" | "openai_compatible")
            {
                return Err(invalid("inconsistent external result and chosen intent"));
            }
        }
        let report = self.run_step(&policy, &overrides, Some(&batch))?;
        if report.events != expected {
            self.failed = true;
            let index = report
                .events
                .iter()
                .zip(expected)
                .position(|(a, b)| a != b)
                .unwrap_or(report.events.len().min(expected.len()));
            return Err(invalid(&format!(
                "event mismatch at tick {tick}, offset {index}"
            )));
        }
        Ok(report)
    }
}
