//! Real experiment grids, persisted artifacts and aggregate comparison checks.
use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn experiment_matrix_writes_reproducible_artifacts_and_correct_group_means() {
    let directory = tempfile::tempdir().unwrap();
    let mut reports = Vec::new();
    for name in ["first", "second"] {
        let output_dir = directory.path().join(name);
        let output = Command::new(env!("CARGO_BIN_EXE_agentsociety-cli"))
            .args([
                "experiment",
                "--populations",
                "0,8",
                "--seeds",
                "11,12",
                "--ticks",
                "20",
                "--record-events",
                "--json",
            ])
            .arg("--out-dir")
            .arg(&output_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["completed"], true);
        assert_eq!(report["results"].as_array().unwrap().len(), 4);
        assert_eq!(report["comparisons"].as_array().unwrap().len(), 2);
        assert_eq!(
            fs::read_to_string(output_dir.join("results.csv"))
                .unwrap()
                .lines()
                .count(),
            5
        );
        for row in report["results"].as_array().unwrap() {
            let checkpoint = output_dir.join(row["checkpoint_file"].as_str().unwrap());
            let saved = agentsociety_storage::read_checkpoint(&checkpoint).unwrap();
            assert_eq!(
                row["state_sha256"].as_str().unwrap(),
                agentsociety_storage::checksum(&saved.simulation).unwrap()
            );
            let events = output_dir.join(row["events_file"].as_str().unwrap());
            let replay = agentsociety_storage::replay_journal(std::io::BufReader::new(
                fs::File::open(events).unwrap(),
            ))
            .unwrap();
            assert_eq!(replay.checkpoint, saved);
        }
        for group in report["comparisons"].as_array().unwrap() {
            let rows: Vec<_> = report["results"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["config"]["agents"] == group["population"])
                .collect();
            let mean = rows
                .iter()
                .map(|row| row["cumulative"]["trade_count"].as_u64().unwrap() as f64)
                .sum::<f64>()
                / rows.len() as f64;
            assert_eq!(group["mean_trades"].as_f64().unwrap(), mean);
        }
        let before = fs::read(output_dir.join("report.json")).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_agentsociety-cli"))
            .args([
                "experiment",
                "--populations",
                "0,8",
                "--seeds",
                "11,12",
                "--ticks",
                "1",
            ])
            .arg("--out-dir")
            .arg(&output_dir)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read(output_dir.join("report.json")).unwrap(), before);
        reports.push(report);
    }
    for (first, second) in reports[0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .zip(reports[1]["results"].as_array().unwrap())
    {
        assert_eq!(first["state_sha256"], second["state_sha256"]);
        assert_eq!(first["wealth_distribution"], second["wealth_distribution"]);
        assert_eq!(first["social"], second["social"]);
    }
}

#[test]
fn invalid_experiment_matrix_fails_before_creating_results() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid");
    let output = Command::new(env!("CARGO_BIN_EXE_agentsociety-cli"))
        .args([
            "experiment",
            "--populations",
            "8,100001",
            "--seeds",
            "1",
            "--ticks",
            "1",
        ])
        .arg("--out-dir")
        .arg(&path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!path.exists());
}
