//! A slow provider cannot hold the UI publication lock or prevent bounded shutdown.
#![cfg(feature = "llm")]

#[path = "../../../tests/support/mock_llm.rs"]
mod mock_llm;

use agentsociety_core::*;
use agentsociety_desktop::worker::{Command, Worker};
use agentsociety_simulation::{LlmSettings, PolicyRuntime};
use mock_llm::{Response, Server};
use std::{
    thread,
    time::{Duration, Instant},
};

#[test]
fn saving_loading_and_branching_keep_external_call_limits() {
    let server = Server::new(|_| Response::rest());
    let directory = tempfile::tempdir().unwrap();
    let checkpoint = directory.path().join("external.json");
    let worker = Worker::with_policy(
        SimulationConfig {
            agents: 2,
            llm_agents: 1,
            policy: PolicyKind::Llm,
            ..Default::default()
        },
        PolicyRuntime::with_llm(LlmSettings {
            base_url: server.base_url.clone(),
            max_total_calls: 1,
            min_interval_ticks: 1,
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    let wait = |condition: &dyn Fn(&agentsociety_desktop::worker::View) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let view = worker.view();
            if condition(&view) {
                break;
            }
            assert!(Instant::now() < deadline, "{}", view.status);
            thread::sleep(Duration::from_millis(5));
        }
    };
    worker.send(Command::Step).unwrap();
    wait(&|v| v.snapshot.world.tick == 1);
    worker
        .send(Command::Save {
            path: checkpoint.clone(),
            overwrite: false,
        })
        .unwrap();
    wait(&|v| v.status.starts_with("Saved"));
    worker.send(Command::Load(checkpoint)).unwrap();
    wait(&|v| v.generation == 1);
    assert_eq!(worker.view().llm_usage.attempts, 1);
    let scenario_path = directory.path().join("future.json");
    std::fs::write(&scenario_path, br#"{"name":"branch","events":[]}"#).unwrap();
    worker.send(Command::Branch(scenario_path)).unwrap();
    wait(&|v| v.generation == 2);
    worker.send(Command::Step).unwrap();
    wait(&|v| v.snapshot.world.tick == 2);
    assert_eq!(server.count(), 1);
    assert_eq!(worker.view().llm_usage.attempts, 1);
    assert!(worker.view().llm_usage.fallbacks > 0);
}

#[test]
fn ui_reads_remain_available_during_timeout_and_restart_resets_request_budget() {
    let server = Server::new(|_| {
        let mut response = Response::rest();
        response.delay = Duration::from_millis(500);
        response
    });
    let config = SimulationConfig {
        agents: 10,
        llm_agents: 1,
        policy: PolicyKind::Llm,
        ..Default::default()
    };
    let policy = PolicyRuntime::with_llm(LlmSettings {
        base_url: server.base_url.clone(),
        timeout_ms: 150,
        max_total_calls: 1,
        ..Default::default()
    })
    .unwrap();
    let worker = Worker::with_policy(config.clone(), policy).unwrap();
    worker.send(Command::Step).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while server.count() == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    let started = Instant::now();
    for _ in 0..100 {
        let _ = worker.view();
    }
    assert!(started.elapsed() < Duration::from_millis(100));
    while worker.view().snapshot.world.tick == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    let completed = worker.view();
    assert_eq!(completed.llm_usage.attempts, 1);
    assert_eq!(completed.llm_usage.fallbacks, 1);
    assert_eq!(
        completed.snapshot.world.agents[0]
            .last_external_decision
            .as_ref()
            .unwrap()
            .outcome,
        ExternalOutcome::Timeout
    );
    worker.send(Command::Restart(config)).unwrap();
    while worker.view().generation == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(worker.view().llm_usage, LlmUsage::default());
    assert_eq!(worker.view().snapshot.world.tick, 0);
}
