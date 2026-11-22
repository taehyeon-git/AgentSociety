//! Actual CLI processes exercise file formats, command routing and crash recovery.
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader},
    process::{Command, Output, Stdio},
    time::Duration,
};

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsociety-cli"));
    command.env("RUST_LOG", "warn");
    command
}

fn success(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn checkpoint_resume_jsonl_replay_and_sqlite_replay_match_a_continuous_process() {
    let directory = tempfile::tempdir().unwrap();
    let checkpoint = directory.path().join("half checkpoint.json");
    let state = directory.path().join("continuous.json");
    let journal = directory.path().join("events.jsonl");
    let resumed = directory.path().join("resumed.json");
    let replayed = directory.path().join("replayed.json");
    let database = directory.path().join("run.sqlite");
    success(
        command()
            .args(["--agents", "12", "--ticks", "60", "--json"])
            .arg("--state-out")
            .arg(&state)
            .arg("--events")
            .arg(&journal),
    );
    success(
        command()
            .args([
                "--agents",
                "12",
                "--ticks",
                "25",
                "--json",
                "--checkpoint-every",
                "7",
            ])
            .arg("--checkpoint-out")
            .arg(&checkpoint)
            .arg("--database")
            .arg(&database),
    );
    let original_checkpoint = fs::read(&checkpoint).unwrap();
    success(
        command()
            .arg("resume")
            .arg(&checkpoint)
            .args(["--ticks", "35", "--json"])
            .arg("--state-out")
            .arg(&resumed),
    );
    assert_eq!(fs::read(&state).unwrap(), fs::read(&resumed).unwrap());
    assert_eq!(fs::read(&checkpoint).unwrap(), original_checkpoint);
    success(
        command()
            .arg("replay")
            .arg(&journal)
            .arg("--state-out")
            .arg(&replayed)
            .arg("--json"),
    );
    assert_eq!(fs::read(&state).unwrap(), fs::read(&replayed).unwrap());
    success(
        command()
            .arg("resume")
            .arg(&database)
            .args(["--ticks", "35", "--json"]),
    );
    success(
        command()
            .arg("replay")
            .arg(&database)
            .arg("--state-out")
            .arg(&replayed)
            .args(["--overwrite", "--json"]),
    );
    assert_eq!(fs::read(&state).unwrap(), fs::read(&replayed).unwrap());
    let inspected = success(command().arg("inspect").arg(&database).args([
        "--agent",
        "0",
        "--from-tick",
        "30",
        "--limit",
        "5",
        "--json",
    ]));
    let records: Vec<Value> = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(records.len(), 5);
    assert!(
        records
            .iter()
            .all(|record| record["tick"].as_u64().unwrap() >= 30)
    );
}

#[test]
fn resume_rejects_changed_conditions_and_outputs_cannot_clobber_saved_input() {
    let directory = tempfile::tempdir().unwrap();
    let checkpoint = directory.path().join("saved.json");
    success(
        command()
            .args(["--agents", "4", "--ticks", "3", "--json"])
            .arg("--checkpoint-out")
            .arg(&checkpoint),
    );
    let before = fs::read(&checkpoint).unwrap();
    let output = command()
        .arg("resume")
        .arg(&checkpoint)
        .args(["--agents", "5", "--ticks", "1"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let output = command()
        .arg("resume")
        .arg(&checkpoint)
        .args(["--ticks", "1", "--overwrite"])
        .arg("--events")
        .arg(&checkpoint)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(&checkpoint).unwrap(), before);
}

#[test]
fn actual_terminated_sqlite_process_recovers_a_complete_reproducible_tick() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("interrupted.sqlite");
    let mut child = command()
        .args([
            "--agents",
            "8",
            "--ticks",
            "100000",
            "--report-every",
            "1",
            "--checkpoint-every",
            "100",
        ])
        .arg("--database")
        .arg(&database)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut ticks = 0;
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if line.starts_with("tick=") {
                ticks += 1;
            }
            if ticks == 8 {
                let _ = sender.send(());
                ticks += 1;
            }
        }
    });
    let ready = receiver.recv_timeout(Duration::from_secs(10));
    let _ = child.kill();
    let _ = child.wait();
    reader.join().unwrap();
    assert!(
        ready.is_ok(),
        "simulation did not reach eight committed ticks"
    );
    let recovered = directory.path().join("recovered.json");
    let output = success(
        command()
            .arg("resume")
            .arg(&database)
            .args(["--ticks", "0", "--json"])
            .arg("--state-out")
            .arg(&recovered),
    );
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    let tick = summary["stats"]["tick"].as_u64().unwrap();
    assert!((8..100000).contains(&tick));
    let expected = directory.path().join("expected.json");
    success(
        command()
            .args(["--agents", "8", "--ticks", &tick.to_string(), "--json"])
            .arg("--state-out")
            .arg(&expected),
    );
    assert_eq!(fs::read(recovered).unwrap(), fs::read(expected).unwrap());
}

#[cfg(not(feature = "llm"))]
#[test]
fn offline_binary_replays_external_policy_records_without_http_support() {
    use agentsociety_core::*;
    use agentsociety_simulation::{
        ExternalPolicyCheckpoint, LlmSettings, PolicyCheckpoint, RunCheckpoint, Simulation,
    };
    use agentsociety_storage::JournalWriter;
    let directory = tempfile::tempdir().unwrap();
    let journal_path = directory.path().join("llm.jsonl");
    let mut sim = Simulation::new(SimulationConfig {
        agents: 2,
        llm_agents: 1,
        policy: PolicyKind::Llm,
        ..Default::default()
    })
    .unwrap();
    let mut policy = PolicyCheckpoint {
        external: Some(ExternalPolicyCheckpoint {
            settings: LlmSettings {
                max_total_calls: 0,
                ..Default::default()
            },
            state: LlmState::default(),
        }),
    };
    let mut writer = JournalWriter::new(
        fs::File::create(&journal_path).unwrap(),
        &sim,
        policy.clone(),
        3,
    )
    .unwrap();
    for tick in 1..=3 {
        let batch = DecisionBatch {
            target_tick: tick,
            decisions: std::collections::BTreeMap::from([(
                AgentId(0),
                ExternalDecision {
                    agent_id: AgentId(0),
                    target_tick: tick,
                    provider: "ollama".into(),
                    model: "llama3.2:3b".into(),
                    outcome: ExternalOutcome::BudgetExhausted,
                    decision: None,
                    summary: "budget exhausted".into(),
                },
            )]),
        };
        let report = sim.step_with_decision_batch(&batch).unwrap();
        policy.external.as_mut().unwrap().state.usage.fallbacks += 1;
        writer.append(&sim, &report, policy.clone()).unwrap();
    }
    writer
        .finish(&RunCheckpoint {
            checkpoint_version: 1,
            simulation: sim.snapshot().unwrap(),
            policy,
        })
        .unwrap();
    let output = success(command().arg("replay").arg(&journal_path).arg("--json"));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["final_tick"], 3);
    assert_eq!(result["llm_usage"]["fallbacks"], 3);
}
