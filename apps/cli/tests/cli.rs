//! End-to-end tests launch the actual CLI and inspect persisted outputs.

use serde_json::Value;
use std::{fs, process::Command};

fn command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_agentsociety-cli"));
    cmd.env("RUST_LOG", "warn");
    cmd
}

#[test]
fn selecting_an_external_policy_requires_an_explicit_adapter() {
    let output = command()
        .args(["--policy", "llm", "--ticks", "1"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("requires an llm-enabled build and --llm-config")
    );
}

#[cfg(not(feature = "llm"))]
#[test]
fn offline_binary_reports_how_to_enable_http_instead_of_silently_ignoring_it() {
    let output = command()
        .args(["--llm-config", "unused.json", "--ticks", "0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--features llm"));
}

#[test]
fn cli_exports_real_consistent_state_events_and_csv() {
    let directory = tempfile::tempdir().unwrap();
    let events = directory.path().join("events.jsonl");
    let state = directory.path().join("state.json");
    let summary = directory.path().join("summary.json");
    let csv = directory.path().join("stats.csv");
    let output = command()
        .args(["--agents", "8", "--ticks", "20", "--seed", "11", "--json"])
        .arg("--events")
        .arg(&events)
        .arg("--state-out")
        .arg(&state)
        .arg("--summary-out")
        .arg(&summary)
        .arg("--csv")
        .arg(&csv)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    let saved_summary: Value = serde_json::from_slice(&fs::read(summary).unwrap()).unwrap();
    assert_eq!(stdout, saved_summary);
    let saved_state: Value = serde_json::from_slice(&fs::read(state).unwrap()).unwrap();
    assert_eq!(saved_state["world"]["tick"], 20);
    assert_eq!(saved_state["world"]["agents"].as_array().unwrap().len(), 8);
    let records: Vec<Value> = fs::read_to_string(events)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records[0]["record_type"], "run_started");
    assert_eq!(records[0]["initial"]["config"]["seed"], 11);
    let events: Vec<_> = records
        .iter()
        .filter(|record| record["record_type"] == "event")
        .collect();
    assert_eq!(
        events.len() as u64,
        stdout["total_events"].as_u64().unwrap()
    );
    for (i, record) in events.iter().enumerate() {
        assert_eq!(record["sequence"], i as u64);
    }
    assert_eq!(events.last().unwrap()["event"]["type"], "tick_completed");
    assert_eq!(
        records
            .iter()
            .filter(|record| record["record_type"] == "tick_committed")
            .count(),
        20
    );
    assert_eq!(records.last().unwrap()["record_type"], "run_completed");
    assert_eq!(
        records.last().unwrap()["checkpoint"]["simulation"],
        saved_state
    );
    assert_eq!(fs::read_to_string(csv).unwrap().lines().count(), 21);
}

#[test]
fn independent_cli_runs_export_identical_deterministic_state() {
    let directory = tempfile::tempdir().unwrap();
    let one = directory.path().join("one.json");
    let two = directory.path().join("two.json");
    for path in [&one, &two] {
        let output = command()
            .args(["--agents", "10", "--ticks", "60", "--seed", "42", "--json"])
            .arg("--state-out")
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(fs::read(one).unwrap(), fs::read(two).unwrap());
}

#[test]
fn invalid_config_and_colliding_outputs_fail_explicitly() {
    let output = command()
        .args(["--agents", "100001", "--ticks", "0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid configuration"));
    let directory = tempfile::tempdir().unwrap();
    let same = directory.path().join("same.json");
    let output = command()
        .arg("--events")
        .arg(&same)
        .arg("--state-out")
        .arg(&same)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!same.exists());
    fs::write(&same, r#"{"agents":0}"#).unwrap();
    let output = command()
        .arg("--config")
        .arg(&same)
        .arg("--state-out")
        .arg(&same)
        .arg("--overwrite")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(same).unwrap(), r#"{"agents":0}"#);
}

#[test]
fn zero_tick_run_is_valid_and_existing_outputs_require_explicit_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("state.json");
    fs::write(&state, "keep").unwrap();
    let output = command()
        .args(["--agents", "0", "--ticks", "0", "--json"])
        .arg("--state-out")
        .arg(&state)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&state).unwrap(), "keep");
    let output = command()
        .args(["--agents", "0", "--ticks", "0", "--json", "--overwrite"])
        .arg("--state-out")
        .arg(&state)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["stats"]["gini"], 0.0);
    assert_eq!(summary["stats"]["tick"], 0);
}
