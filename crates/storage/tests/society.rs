//! Common assets and fiscal events survive each persistence path, including empty populations.
use agentsociety_core::*;
use agentsociety_simulation::{PolicyRuntime, RunCheckpoint, Simulation};
use agentsociety_storage::*;
use std::{fs, io::Cursor};

#[test]
fn institutional_accounts_crises_and_all_contract_owners_round_trip_through_storage() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("society.sqlite");
    let config: SimulationConfig =
        serde_json::from_str(include_str!("../../../examples/complete.json")).unwrap();
    let mut sim = Simulation::new(config).unwrap();
    let runtime = PolicyRuntime::default();
    let mut store = RunStore::create(&path, &sim, runtime.snapshot(), 120, 37).unwrap();
    let mut journal = JournalWriter::new(Vec::new(), &sim, runtime.snapshot(), 120).unwrap();
    let mut shared_delivery = None;
    for _ in 0..120 {
        let report = sim.step().unwrap();
        for record in &report.events {
            if matches!(
                record.event,
                Event::Contract {
                    event: ContractEvent::Delivered { .. }
                }
            ) && record.event.participants().len() >= 3
            {
                shared_delivery = Some(record.clone());
            }
        }
        store.append(&sim, &report, runtime.snapshot()).unwrap();
        journal.append(&sim, &report, runtime.snapshot()).unwrap();
    }
    let expected = RunCheckpoint::capture(&sim, &runtime).unwrap();
    drop(store);
    let before = fs::read(&path).unwrap();
    assert_eq!(read_run(&path).unwrap(), expected);
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut store = RunStore::open(&path, false).unwrap();
    assert_eq!(store.replay().unwrap().checkpoint, expected);
    let delivered = shared_delivery.expect("an actual multi-owner contract installment");
    for owner in delivered.event.participants() {
        let records = store
            .query_events(&EventQuery {
                agent: Some(owner),
                from_tick: delivered.tick,
                to_tick: delivered.tick,
                limit: 10000,
            })
            .unwrap();
        assert!(records.contains(&delivered), "missing common owner {owner}");
    }
    journal.finish(&expected).unwrap();
    let path = directory.path().join("society.jsonl");
    fs::write(&path, journal.into_inner()).unwrap();
    assert_eq!(read_run(&path).unwrap(), expected);
    let path = directory.path().join("society.json");
    write_checkpoint(&path, &expected, false).unwrap();
    assert_eq!(read_run(&path).unwrap(), expected);
}

#[test]
fn empty_population_still_records_global_shocks_and_policy_changes() {
    let mut config: SimulationConfig =
        serde_json::from_str(include_str!("../../../examples/complete.json")).unwrap();
    config.agents = 0;
    let mut sim = Simulation::new(config).unwrap();
    let runtime = PolicyRuntime::default();
    let mut journal = JournalWriter::new(Vec::new(), &sim, runtime.snapshot(), 200).unwrap();
    for _ in 0..200 {
        let report = sim.step().unwrap();
        journal.append(&sim, &report, runtime.snapshot()).unwrap();
    }
    let expected = RunCheckpoint::capture(&sim, &runtime).unwrap();
    journal.finish(&expected).unwrap();
    assert_eq!(
        replay_journal(Cursor::new(journal.into_inner()))
            .unwrap()
            .checkpoint,
        expected
    );
    assert_eq!(sim.cumulative().scenario_events, 5);
    assert_eq!(sim.stats().total_money, 0);
}
