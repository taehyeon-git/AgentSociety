//! Restore and replay must preserve future facts, including retained memories.
use agentsociety_core::*;
use agentsociety_simulation::{PolicyRuntime, RunCheckpoint, Simulation, SimulationSnapshot};
use serde_json::json;

fn completed(agents: usize, ticks: u64) -> Simulation {
    let mut simulation = Simulation::new(SimulationConfig {
        agents,
        ..Default::default()
    })
    .unwrap();
    for _ in 0..ticks {
        simulation.step().unwrap();
    }
    simulation
}

#[test]
fn a_thousand_ticks_equal_five_hundred_save_restore_and_five_hundred_more() {
    let mut continuous = completed(100, 500);
    let checkpoint = RunCheckpoint::capture(&continuous, &PolicyRuntime::default()).unwrap();
    let checkpoint: RunCheckpoint =
        serde_json::from_slice(&serde_json::to_vec(&checkpoint).unwrap()).unwrap();
    let (mut resumed, mut runtime) = checkpoint.restore().unwrap();
    assert!(resumed.initial_events().is_empty());
    for _ in 0..500 {
        assert_eq!(
            continuous.step().unwrap().events,
            runtime.step(&mut resumed).unwrap().events
        );
    }
    assert_eq!(continuous.snapshot().unwrap(), resumed.snapshot().unwrap());
    assert_eq!(resumed.world().tick(), 1000);
}

#[test]
fn initial_empty_and_disabled_retention_states_restore() {
    for population in [0, 1, 12] {
        for ticks in [0, 1, 20] {
            let mut original = Simulation::new(SimulationConfig {
                agents: population,
                history_capacity: 0,
                memory_capacity: 0,
                relationship_capacity: 0,
                initial_cash: 0,
                ..Default::default()
            })
            .unwrap();
            for _ in 0..ticks {
                original.step().unwrap();
            }
            let mut restored = Simulation::from_snapshot(original.snapshot().unwrap()).unwrap();
            assert_eq!(restored.snapshot().unwrap(), original.snapshot().unwrap());
            assert_eq!(
                restored.step().unwrap().events,
                original.step().unwrap().events
            );
        }
    }
}

#[test]
fn malformed_snapshots_fail_before_a_live_run_exists() {
    let source = serde_json::to_value(completed(12, 40).snapshot().unwrap()).unwrap();
    let edits = [
        ("/format_version", json!(999)),
        ("/engine_version", json!("future")),
        ("/world/tick", json!(41)),
        ("/world/width", json!(0)),
        ("/world/agents/0/id", json!(1)),
        ("/world/agents/0/position/x", json!(100)),
        ("/world/agents/0/needs/energy", json!(101)),
        ("/world/agents/0/account/reserved_cash", json!(1)),
        ("/world/agents/0/account/earned", json!(99999999)),
        ("/world/agents/0/name", json!("changed identity")),
        ("/world/agents/0/history/0/tick", json!(999)),
        ("/world/agents/0/memories/0/agent_id", json!(1)),
        ("/world/agents/0/memories/0/tick", json!(999)),
        ("/world/agents/0/memories/0/id", json!(999999999)),
        ("/random/word_position", json!(0)),
        ("/random/stream", json!(2)),
        ("/random/seed/0", json!(255)),
        ("/baseline/total_money", json!(0)),
        ("/stats/gini", json!(0.123456)),
        ("/stats/total_money", json!(0)),
        ("/cumulative/trade_value", json!(0)),
        ("/next_event_sequence", json!(1)),
        ("/market/next_order_id", json!(999999)),
        ("/market/prices/0", json!(0)),
    ];
    for (path, value) in edits {
        let mut altered = source.clone();
        *altered
            .pointer_mut(path)
            .unwrap_or_else(|| panic!("missing path {path}")) = value;
        let snapshot: SimulationSnapshot = serde_json::from_value(altered).unwrap();
        assert!(
            Simulation::from_snapshot(snapshot).is_err(),
            "accepted corrupted field {path}"
        );
    }
    let mut altered = source;
    altered["world"]["agents"][0]["relationships"] = json!({"999999": {
        "trust": 1, "familiarity": 1, "cooperation_score": 0, "interaction_count": 1, "last_interaction": 1
    }});
    assert!(Simulation::from_snapshot(serde_json::from_value(altered).unwrap()).is_err());
}

#[test]
fn replay_reuses_external_overrides_and_checks_every_resulting_event() {
    let config = SimulationConfig {
        agents: 8,
        ..Default::default()
    };
    let mut original = Simulation::new(config.clone()).unwrap();
    let mut replay = Simulation::new(config).unwrap();
    for tick in 1..=60 {
        let overrides = std::collections::BTreeMap::from([(
            AgentId(tick % 8),
            Decision {
                action: Action::Rest {},
                reason: DecisionReason::RecoverEnergy,
            },
        )]);
        let report = original.step_with_overrides(&overrides).unwrap();
        assert_eq!(
            report.events,
            replay.replay_tick(&report.events).unwrap().events
        );
    }
    assert_eq!(original.snapshot().unwrap(), replay.snapshot().unwrap());
}

#[test]
fn replay_rejects_bad_framing_and_poisoned_runs_cannot_export() {
    let mut original = completed(4, 2);
    let before = original.snapshot().unwrap();
    let report = original.step().unwrap();
    let mut replay = Simulation::from_snapshot(before.clone()).unwrap();
    let mut events = report.events.clone();
    events[0].sequence += 1;
    assert!(replay.replay_tick(&events).is_err());
    assert_eq!(before, replay.snapshot().unwrap());
    let mut events = report.events;
    if let Event::TickCompleted { total_money, .. } = &mut events.last_mut().unwrap().event {
        *total_money += 1;
    }
    assert!(replay.replay_tick(&events).is_err());
    assert!(replay.snapshot().is_err());
    assert!(replay.step().is_err());
}

#[test]
fn checkpoints_validate_envelope_and_reject_unknown_top_level_fields() {
    let checkpoint = RunCheckpoint::capture(&completed(3, 3), &PolicyRuntime::default()).unwrap();
    let mut invalid = checkpoint.clone();
    invalid.checkpoint_version = 2;
    assert!(invalid.restore_engine().is_err());
    let mut value = serde_json::to_value(checkpoint).unwrap();
    value["secret_extra"] = json!(true);
    assert!(serde_json::from_value::<RunCheckpoint>(value).is_err());
}
