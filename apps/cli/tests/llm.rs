//! Optional HTTP mode exercised through the actual executable and child-only credentials.
#![cfg(feature = "llm")]

#[path = "../../../tests/support/mock_llm.rs"]
mod mock_llm;

use agentsociety_simulation::{LlmSettings, Provider};
use mock_llm::{Response, Server};
use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn separate_process_resume_preserves_budget_and_replay_needs_no_credentials() {
    let server = Server::new(|_| {
        Response::openai(r#"{"action":{"action":"rest"},"reasoning_summary":"Rest."}"#)
    });
    let directory = tempfile::tempdir().unwrap();
    let settings_path = directory.path().join("llm.json");
    let checkpoint = directory.path().join("saved.json");
    let journal = directory.path().join("resumed.jsonl");
    let state = directory.path().join("state.json");
    let replayed = directory.path().join("replayed.json");
    fs::write(
        &settings_path,
        serde_json::to_vec(&LlmSettings {
            provider: Provider::OpenaiCompatible,
            base_url: format!("{}/v1", server.base_url),
            api_key_env: Some("AGENTSOCIETY_RESUME_TEST_KEY".into()),
            min_interval_ticks: 4,
            max_total_calls: 2,
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    let first = Command::new(env!("CARGO_BIN_EXE_agentsociety-cli"))
        .env("AGENTSOCIETY_RESUME_TEST_KEY", "synthetic-resume-token")
        .args([
            "--agents",
            "4",
            "--ticks",
            "2",
            "--policy",
            "llm",
            "--llm-agents",
            "1",
            "--json",
        ])
        .arg("--llm-config")
        .arg(&settings_path)
        .arg("--checkpoint-out")
        .arg(&checkpoint)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(server.count(), 1);
    let resumed = Command::new(env!("CARGO_BIN_EXE_agentsociety-cli"))
        .env("AGENTSOCIETY_RESUME_TEST_KEY", "synthetic-resume-token")
        .arg("resume")
        .arg(&checkpoint)
        .args(["--ticks", "10", "--json"])
        .arg("--events")
        .arg(&journal)
        .arg("--state-out")
        .arg(&state)
        .output()
        .unwrap();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(server.count(), 2);
    let summary: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(summary["llm_usage"]["attempts"], 2);
    assert!(
        !fs::read_to_string(&checkpoint)
            .unwrap()
            .contains("synthetic-resume-token")
    );
    assert!(
        !fs::read_to_string(&journal)
            .unwrap()
            .contains("synthetic-resume-token")
    );
    drop(server);
    let replay = Command::new(env!("CARGO_BIN_EXE_agentsociety-cli"))
        .env_remove("AGENTSOCIETY_RESUME_TEST_KEY")
        .arg("replay")
        .arg(&journal)
        .arg("--state-out")
        .arg(&replayed)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    assert_eq!(fs::read(state).unwrap(), fs::read(replayed).unwrap());
}

#[test]
fn cli_uses_environment_auth_and_persists_decisions_without_credentials() {
    let server = Server::new(|_| {
        Response::openai(r#"{"action":{"action":"rest"},"reasoning_summary":"Recover energy."}"#)
    });
    let directory = tempfile::tempdir().unwrap();
    let settings = directory.path().join("llm.json");
    let events = directory.path().join("events.jsonl");
    let state = directory.path().join("state.json");
    let config = LlmSettings {
        provider: Provider::OpenaiCompatible,
        base_url: format!("{}/v1", server.base_url),
        api_key_env: Some("AGENTSOCIETY_TEST_API_KEY".into()),
        max_total_calls: 2,
        min_interval_ticks: 1,
        ..Default::default()
    };
    fs::write(&settings, serde_json::to_vec(&config).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agentsociety-cli"))
        .env("AGENTSOCIETY_TEST_API_KEY", "synthetic-test-token")
        .env("RUST_LOG", "warn")
        .args([
            "--agents",
            "8",
            "--ticks",
            "5",
            "--policy",
            "llm",
            "--llm-agents",
            "2",
            "--json",
        ])
        .arg("--llm-config")
        .arg(&settings)
        .arg("--events")
        .arg(&events)
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
    assert_eq!(summary["stats"]["tick"], 5);
    assert_eq!(summary["llm_usage"]["attempts"], 2);
    assert_eq!(summary["llm_usage"]["accepted"], 2);
    assert_eq!(summary["llm_usage"]["input_tokens"], 200);
    assert_eq!(server.count(), 2);
    for request in server.requests.lock().unwrap().iter() {
        assert_eq!(request.path, "/v1/chat/completions");
        assert_eq!(
            request.authorization.as_deref(),
            Some("Bearer synthetic-test-token")
        );
    }
    let journal = fs::read_to_string(&events).unwrap();
    assert!(journal.contains("external_decision_recorded"));
    assert!(journal.contains("budget_exhausted"));
    for text in [
        journal,
        fs::read_to_string(state).unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    ] {
        assert!(!text.contains("synthetic-test-token"));
    }
}

#[test]
fn llm_config_cannot_be_overwritten_by_any_output() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("llm.json");
    let bytes = serde_json::to_vec(&LlmSettings::default()).unwrap();
    fs::write(&path, &bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agentsociety-cli"))
        .args(["--ticks", "0", "--policy", "llm", "--overwrite"])
        .arg("--llm-config")
        .arg(&path)
        .arg("--state-out")
        .arg(&path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(String::from_utf8_lossy(&output.stderr).contains("must not overwrite"));
}
