//! Recorded external results are validated, replayable inputs even in offline builds.

use agentsociety_core::*;
use agentsociety_simulation::{PolicyRuntime, Simulation};
use std::collections::BTreeMap;

fn batch(tick: Tick) -> DecisionBatch {
    DecisionBatch {
        target_tick: tick,
        decisions: BTreeMap::from([(
            AgentId(0),
            ExternalDecision {
                agent_id: AgentId(0),
                target_tick: tick,
                provider: "ollama".into(),
                model: "test-model".into(),
                outcome: ExternalOutcome::Accepted,
                decision: Some(Decision {
                    action: Action::Rest {},
                    reason: DecisionReason::ExternalModel,
                }),
                summary: "Recover energy.".into(),
            },
        )]),
    }
}

#[test]
fn stale_unknown_and_inconsistent_external_records_do_not_mutate_state() {
    let mut sim = Simulation::new(SimulationConfig::default()).unwrap();
    let before = sim.snapshot().unwrap();
    let mut wrong_id = batch(1);
    wrong_id.decisions.get_mut(&AgentId(0)).unwrap().agent_id = AgentId(1);
    let mut unknown = batch(1);
    let mut result = unknown.decisions.remove(&AgentId(0)).unwrap();
    result.agent_id = AgentId(9999);
    unknown.decisions.insert(result.agent_id, result);
    let mut missing_decision = batch(1);
    missing_decision
        .decisions
        .get_mut(&AgentId(0))
        .unwrap()
        .decision = None;
    let mut wrong_outcome = batch(1);
    wrong_outcome
        .decisions
        .get_mut(&AgentId(0))
        .unwrap()
        .outcome = ExternalOutcome::Timeout;
    let mut long_summary = batch(1);
    long_summary.decisions.get_mut(&AgentId(0)).unwrap().summary = "x".repeat(513);
    for invalid in [
        batch(0),
        batch(2),
        wrong_id,
        unknown,
        missing_decision,
        wrong_outcome,
        long_summary,
    ] {
        assert!(sim.step_with_decision_batch(&invalid).is_err());
        assert_eq!(sim.snapshot().unwrap(), before);
    }
    sim.step_with_decision_batch(&batch(1)).unwrap();
    assert_eq!(sim.world().tick(), 1);
}

#[test]
fn recorded_batches_reproduce_every_event_and_memory_without_network_code() {
    let config = SimulationConfig {
        agents: 20,
        policy: PolicyKind::Hybrid,
        ..Default::default()
    };
    let mut original = Simulation::new(config.clone()).unwrap();
    let mut replay = Simulation::new(config).unwrap();
    for tick in 1..=60 {
        let mut input = batch(tick);
        if tick % 3 == 0 {
            let result = input.decisions.get_mut(&AgentId(0)).unwrap();
            result.outcome = ExternalOutcome::Timeout;
            result.decision = None;
            result.summary = "Using rules after timeout.".into();
        }
        let first = original.step_with_decision_batch(&input).unwrap();
        let records: Vec<_> = first
            .events
            .iter()
            .filter_map(|record| match &record.event {
                Event::ExternalDecisionRecorded { result } => {
                    Some((result.agent_id, result.clone()))
                }
                _ => None,
            })
            .collect();
        let json = serde_json::to_vec(&DecisionBatch {
            target_tick: tick,
            decisions: records.into_iter().collect(),
        })
        .unwrap();
        let second = replay
            .step_with_decision_batch(&serde_json::from_slice(&json).unwrap())
            .unwrap();
        assert_eq!(first.events, second.events);
        assert_eq!(first.stats, second.stats);
    }
    assert_eq!(original.snapshot().unwrap(), replay.snapshot().unwrap());
}

#[test]
fn recorded_accepted_actions_still_pass_economic_validation() {
    let mut sim = Simulation::new(SimulationConfig {
        agents: 1,
        ..Default::default()
    })
    .unwrap();
    let mut input = batch(1);
    input
        .decisions
        .get_mut(&AgentId(0))
        .unwrap()
        .decision
        .as_mut()
        .unwrap()
        .action = Action::Sell {
        resource: Resource::Metal,
        quantity: 100,
        limit_price: 100,
    };
    let report = sim.step_with_decision_batch(&input).unwrap();
    assert_eq!(report.stats.flows.rejected_actions, 1);
    assert_eq!(report.stats.flows.trade_count, 0);
    assert_eq!(
        report.stats.total_money,
        sim.snapshot().unwrap().baseline.total_money
    );
}

#[test]
fn an_unconfigured_runtime_does_not_silently_claim_to_run_llm_agents() {
    let config = SimulationConfig {
        policy: PolicyKind::Llm,
        ..Default::default()
    };
    let mut sim = Simulation::new(config).unwrap();
    let before = sim.snapshot().unwrap();
    assert!(PolicyRuntime::default().step(&mut sim).is_err());
    assert_eq!(before, sim.snapshot().unwrap());
}
