//! Real files and SQLite transactions, including failure and corruption paths.
use agentsociety_core::*;
use agentsociety_simulation::{PolicyRuntime, RunCheckpoint, Simulation};
use agentsociety_storage::{
    EventQuery, JournalWriter, RunStore, read_checkpoint, replay_journal, write_checkpoint,
};
use std::{fs, io::Cursor};

fn simulation() -> Simulation {
    Simulation::new(SimulationConfig {
        agents: 12,
        ..Default::default()
    })
    .unwrap()
}

fn recorded(ticks: u64) -> (Vec<u8>, RunCheckpoint) {
    let mut sim = simulation();
    let mut runtime = PolicyRuntime::default();
    let mut journal = JournalWriter::new(Vec::new(), &sim, runtime.snapshot(), ticks).unwrap();
    for _ in 0..ticks {
        let report = runtime.step(&mut sim).unwrap();
        journal.append(&sim, &report, runtime.snapshot()).unwrap();
    }
    let checkpoint = RunCheckpoint::capture(&sim, &runtime).unwrap();
    journal.finish(&checkpoint).unwrap();
    (journal.into_inner(), checkpoint)
}

#[test]
fn checked_checkpoint_round_trip_and_failed_replacement_preserves_existing_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("checkpoint.json");
    let (_, checkpoint) = recorded(10);
    write_checkpoint(&path, &checkpoint, false).unwrap();
    assert_eq!(read_checkpoint(&path).unwrap(), checkpoint);
    let before = fs::read(&path).unwrap();
    assert!(write_checkpoint(&path, &checkpoint, false).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut invalid = checkpoint.clone();
    invalid.simulation.random.stream = 3;
    assert!(write_checkpoint(&path, &invalid, true).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut value: serde_json::Value = serde_json::from_slice(&before).unwrap();
    value["checksum"] = serde_json::json!("changed");
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(read_checkpoint(&path).is_err());
    write_checkpoint(&path, &checkpoint, true).unwrap();
    assert_eq!(read_checkpoint(&path).unwrap(), checkpoint);
}

#[test]
fn jsonl_round_trip_checks_every_event_and_final_checkpoint() {
    for ticks in [0, 1, 30] {
        let (bytes, expected) = recorded(ticks);
        let replay = replay_journal(Cursor::new(bytes)).unwrap();
        assert_eq!(replay.checkpoint, expected);
        assert_eq!(replay.replayed_ticks, ticks);
        assert_eq!(
            replay.verified_events,
            expected.simulation.next_event_sequence
        );
    }
}

#[test]
fn pending_social_commitments_survive_sqlite_recovery_checkpoint_and_jsonl_replay() {
    let config = SimulationConfig {
        agents: 4,
        interactions: InteractionSettings {
            negotiation: true,
            cooperation: true,
            goal_review_interval: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut sim = Simulation::new(config).unwrap();
    let runtime = PolicyRuntime::default();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("social.sqlite");
    let mut store = RunStore::create(&path, &sim, runtime.snapshot(), 2, 3).unwrap();
    let mut journal = JournalWriter::new(Vec::new(), &sim, runtime.snapshot(), 2).unwrap();
    let mut decisions: std::collections::BTreeMap<_, _> = (0..4)
        .map(|id| {
            (
                AgentId(id),
                Decision {
                    action: Action::Wait {},
                    reason: DecisionReason::NoOpportunity,
                },
            )
        })
        .collect();
    decisions.get_mut(&AgentId(0)).unwrap().action = Action::Negotiate {
        counterparty: AgentId(2),
        side: OrderSide::Buy,
        resource: Resource::Wood,
        quantity: 2,
        unit_price: 70,
    };
    decisions.get_mut(&AgentId(1)).unwrap().action = Action::Cooperate {
        partner: AgentId(3),
        producer_share: 2,
    };
    let first = sim.step_with_overrides(&decisions).unwrap();
    store.append(&sim, &first, runtime.snapshot()).unwrap();
    journal.append(&sim, &first, runtime.snapshot()).unwrap();
    let checkpoint = RunCheckpoint::capture(&sim, &runtime).unwrap();
    let checkpoint_path = directory.path().join("pending.json");
    write_checkpoint(&checkpoint_path, &checkpoint, false).unwrap();
    assert_eq!(read_checkpoint(&checkpoint_path).unwrap(), checkpoint);
    drop(store); // No final checkpoint: recover the committed tail containing reservations.
    let mut store = RunStore::open(&path, true).unwrap();
    let recovered = store.recover().unwrap();
    assert_eq!(recovered.replayed_ticks, 1);
    assert_eq!(recovered.checkpoint, checkpoint);
    let mut resumed = recovered.checkpoint.restore_engine().unwrap();
    for decision in decisions.values_mut() {
        decision.action = Action::Wait {};
    }
    decisions.get_mut(&AgentId(2)).unwrap().action = Action::AcceptProposal { proposal_id: 0 };
    decisions.get_mut(&AgentId(3)).unwrap().action = Action::AcceptProposal { proposal_id: 1 };
    let second = sim.step_with_overrides(&decisions).unwrap();
    assert_eq!(
        resumed.step_with_overrides(&decisions).unwrap().events,
        second.events
    );
    store.append(&resumed, &second, runtime.snapshot()).unwrap();
    journal.append(&sim, &second, runtime.snapshot()).unwrap();
    let expected = RunCheckpoint::capture(&sim, &runtime).unwrap();
    journal.finish(&expected).unwrap();
    store.finish(&expected).unwrap();
    assert_eq!(
        replay_journal(Cursor::new(journal.into_inner()))
            .unwrap()
            .checkpoint,
        expected
    );
    assert_eq!(store.replay().unwrap().checkpoint, expected);
    for id in 0..4 {
        let rows = store
            .query_events(&EventQuery {
                agent: Some(AgentId(id)),
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert!(
            rows.iter()
                .any(|r| matches!(r.event, Event::ProposalOpened { .. }))
        );
        assert!(rows.iter().any(|r| matches!(
            r.event,
            Event::ProposalClosed {
                outcome: ProposalOutcome::Accepted,
                ..
            }
        )));
        assert!(rows.iter().any(|r| matches!(
            r.event,
            Event::NegotiatedTrade { .. } | Event::CooperationCompleted { .. }
        )));
    }
}

#[test]
fn resumed_jsonl_segment_begins_at_its_saved_tick_and_sequence() {
    let (_, initial) = recorded(12);
    let (mut sim, mut runtime) = initial.restore().unwrap();
    let mut journal = JournalWriter::new(Vec::new(), &sim, runtime.snapshot(), 7).unwrap();
    for _ in 0..7 {
        let report = runtime.step(&mut sim).unwrap();
        journal.append(&sim, &report, runtime.snapshot()).unwrap();
    }
    let final_state = RunCheckpoint::capture(&sim, &runtime).unwrap();
    journal.finish(&final_state).unwrap();
    let replay = replay_journal(Cursor::new(journal.into_inner())).unwrap();
    assert_eq!(replay.replayed_ticks, 7);
    assert_eq!(replay.checkpoint, final_state);
}

#[test]
fn jsonl_rejects_truncation_changed_events_and_trailing_records() {
    let (bytes, _) = recorded(5);
    let text = String::from_utf8(bytes).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let missing_footer = lines[..lines.len() - 1].join("\n") + "\n";
    assert!(replay_journal(Cursor::new(missing_footer)).is_err());
    assert!(replay_journal(Cursor::new(&text[..text.len() - 2])).is_err());
    assert!(replay_journal(Cursor::new(format!("{text}{}\n", lines[0]))).is_err());
    let altered = text.replacen("\"sequence\":0", "\"sequence\":5", 1);
    assert!(replay_journal(Cursor::new(altered)).is_err());
}

#[test]
fn sqlite_recovers_committed_tail_then_resumes_and_replays_identically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("run.sqlite");
    let mut sim = simulation();
    let mut runtime = PolicyRuntime::default();
    let mut store = RunStore::create(&path, &sim, runtime.snapshot(), 10, 3).unwrap();
    for _ in 0..7 {
        let report = runtime.step(&mut sim).unwrap();
        store.append(&sim, &report, runtime.snapshot()).unwrap();
    }
    // No finish: emulate loss of the application after a committed tick.
    drop(store);
    let mut store = RunStore::open(&path, true).unwrap();
    let recovered = store.recover().unwrap();
    assert_eq!(recovered.replayed_ticks, 1);
    assert_eq!(recovered.checkpoint.simulation, sim.snapshot().unwrap());
    let (mut resumed, mut resumed_runtime) = recovered.checkpoint.restore().unwrap();
    store.extend_run(3).unwrap();
    for _ in 0..3 {
        let expected = runtime.step(&mut sim).unwrap();
        let actual = resumed_runtime.step(&mut resumed).unwrap();
        assert_eq!(actual.events, expected.events);
        store
            .append(&resumed, &actual, resumed_runtime.snapshot())
            .unwrap();
    }
    let final_state = RunCheckpoint::capture(&resumed, &resumed_runtime).unwrap();
    store.finish(&final_state).unwrap();
    drop(store);
    let mut readonly = RunStore::open(&path, false).unwrap();
    assert_eq!(readonly.replay().unwrap().checkpoint, final_state);
    assert!(readonly.extend_run(1).is_err());
    let records = readonly
        .query_events(&EventQuery {
            agent: Some(AgentId(0)),
            limit: 1000,
            ..Default::default()
        })
        .unwrap();
    assert!(!records.is_empty());
    assert!(records.iter().any(|record| matches!(
        record.event,
        Event::AgentCreated {
            agent_id: AgentId(0),
            ..
        }
    )));
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
}

#[test]
fn sqlite_failed_tick_transaction_keeps_events_policy_and_checkpoint_together() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rollback.sqlite");
    let mut sim = simulation();
    let mut runtime = PolicyRuntime::default();
    let before = RunCheckpoint::capture(&sim, &runtime).unwrap();
    let mut store = RunStore::create(&path, &sim, runtime.snapshot(), 1, 1).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_commit BEFORE INSERT ON tick_commits BEGIN SELECT RAISE(ABORT, 'test injected failure'); END;").unwrap();
    let report = runtime.step(&mut sim).unwrap();
    assert!(store.append(&sim, &report, runtime.snapshot()).is_err());
    assert_eq!(store.recover().unwrap().checkpoint, before);
    connection
        .execute_batch("DROP TRIGGER fail_commit;")
        .unwrap();
    store.append(&sim, &report, runtime.snapshot()).unwrap();
    let final_state = RunCheckpoint::capture(&sim, &runtime).unwrap();
    store.finish(&final_state).unwrap();
    assert_eq!(store.replay().unwrap().checkpoint, final_state);
}

#[test]
fn sqlite_detects_changed_rows_and_never_clobbers_an_existing_database() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("corrupt.sqlite");
    let mut sim = simulation();
    let mut runtime = PolicyRuntime::default();
    let mut store = RunStore::create(&path, &sim, runtime.snapshot(), 2, 1).unwrap();
    for _ in 0..2 {
        let report = runtime.step(&mut sim).unwrap();
        store.append(&sim, &report, runtime.snapshot()).unwrap();
    }
    drop(store);
    assert!(RunStore::create(&path, &sim, runtime.snapshot(), 2, 1).is_err());
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE events SET body='{}' WHERE sequence=(SELECT max(sequence) FROM events)",
            [],
        )
        .unwrap();
    let mut reader = RunStore::open(&path, false).unwrap();
    assert!(reader.recover().is_err());
    assert!(reader.replay().is_err());
    assert!(reader.query_events(&EventQuery::default()).is_err());
}

#[test]
fn concurrent_stale_writer_cannot_append_over_another_committed_tick() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("writers.sqlite");
    let mut sim = simulation();
    let mut runtime = PolicyRuntime::default();
    let mut first = RunStore::create(&path, &sim, runtime.snapshot(), 1, 1).unwrap();
    let mut stale = RunStore::open(&path, true).unwrap();
    let report = runtime.step(&mut sim).unwrap();
    first.append(&sim, &report, runtime.snapshot()).unwrap();
    assert!(stale.append(&sim, &report, runtime.snapshot()).is_err());
    assert_eq!(
        first.recover().unwrap().checkpoint.simulation,
        sim.snapshot().unwrap()
    );
}

#[test]
fn sqlite_detects_changed_checkpoint_even_when_no_tail_needs_replaying() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("checkpoint.sqlite");
    let mut sim = simulation();
    let mut runtime = PolicyRuntime::default();
    let mut store = RunStore::create(&path, &sim, runtime.snapshot(), 1, 1).unwrap();
    let report = runtime.step(&mut sim).unwrap();
    store.append(&sim, &report, runtime.snapshot()).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE checkpoints SET checksum='corrupted' WHERE tick=1",
            [],
        )
        .unwrap();
    let mut reader = RunStore::open(&path, false).unwrap();
    assert!(reader.recover().is_err());
    assert!(reader.replay().is_err());
}
