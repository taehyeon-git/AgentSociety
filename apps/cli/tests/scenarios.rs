//! End-to-end scenario matrices and independent policy branches use actual CLI processes.
use agentsociety_core::*;
use agentsociety_storage::{read_run, read_scenario};
use std::{
    fs,
    process::{Command, Output},
};

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_agentsociety-cli"))
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
fn fiscal(name: &str, tick: Tick) -> ScenarioConfig {
    ScenarioConfig {
        name: name.into(),
        events: vec![ScheduledIntervention {
            tick,
            label: "Funded redistribution".into(),
            intervention: Intervention::Fiscal {
                policy: FiscalPolicy {
                    cash_levy_bps: 100,
                    redistribution_bps: 5000,
                    period: 1,
                    ..Default::default()
                },
            },
        }],
    }
}

#[test]
fn scenario_axis_crosses_identical_seeds_and_keeps_group_means_and_csv_aligned() {
    let directory = tempfile::tempdir().unwrap();
    let tax = directory.path().join("tax.json");
    let crisis = directory.path().join("crisis.json");
    fs::write(&tax, serde_json::to_vec(&fiscal("tax", 2)).unwrap()).unwrap();
    let scenario = ScenarioConfig {
        name: "crisis".into(),
        events: vec![ScheduledIntervention {
            tick: 2,
            label: "Food crisis".into(),
            intervention: Intervention::InventoryLoss {
                resource: Resource::Food,
                fraction_bps: 5000,
            },
        }],
    };
    fs::write(&crisis, serde_json::to_vec(&scenario).unwrap()).unwrap();
    let output = success(
        command()
            .args([
                "experiment",
                "--seeds",
                "11,12",
                "--populations",
                "8",
                "--ticks",
                "12",
                "--include-baseline",
                "--record-events",
                "--json",
            ])
            .arg("--scenarios")
            .arg(format!("{},{}", tax.display(), crisis.display()))
            .arg("--out-dir")
            .arg(directory.path().join("matrix")),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let results = report["results"].as_array().unwrap();
    assert_eq!(results.len(), 6);
    assert_eq!(report["comparisons"].as_array().unwrap().len(), 3);
    for group in report["comparisons"].as_array().unwrap() {
        let rows: Vec<_> = results
            .iter()
            .filter(|r| r["config"]["scenario"]["name"] == group["scenario"])
            .collect();
        assert_eq!(rows.len(), 2);
        let mean = rows
            .iter()
            .map(|r| r["cumulative"]["taxes"].as_u64().unwrap() as f64)
            .sum::<f64>()
            / 2.0;
        assert_eq!(group["mean_taxes"].as_f64().unwrap(), mean);
        if group["scenario"] == "tax" {
            assert!(mean > 0.0);
        } else {
            assert_eq!(mean, 0.0);
        }
    }
    for seed in [11, 12] {
        let mut supplies = results
            .iter()
            .filter(|r| r["config"]["seed"] == seed)
            .map(|r| r["stats"]["total_money"].as_u64().unwrap());
        let first = supplies.next().unwrap();
        assert!(supplies.all(|supply| supply == first));
    }
    let csv = fs::read_to_string(directory.path().join("matrix/results.csv")).unwrap();
    let header: Vec<_> = csv.lines().next().unwrap().split(',').collect();
    let scenario_index = header.iter().position(|s| *s == "scenario").unwrap();
    for (row, expected) in csv.lines().skip(1).zip(results) {
        let cells: Vec<_> = row.split(',').collect();
        assert_eq!(cells.len(), header.len());
        assert_eq!(
            cells[scenario_index],
            expected["config"]["scenario"]["name"].as_str().unwrap()
        );
        let checkpoint = read_run(
            &directory
                .path()
                .join("matrix")
                .join(expected["checkpoint_file"].as_str().unwrap()),
        )
        .unwrap();
        let replayed = read_run(
            &directory
                .path()
                .join("matrix")
                .join(expected["events_file"].as_str().unwrap()),
        )
        .unwrap();
        assert_eq!(checkpoint, replayed);
    }
}

#[test]
fn sqlite_branch_preserves_source_and_replays_the_exact_forked_future() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.sqlite");
    success(
        command()
            .args(["run", "--agents", "12", "--ticks", "4", "--json"])
            .arg("--database")
            .arg(&source),
    );
    let saved = read_run(&source).unwrap();
    let source_bytes = fs::read(&source).unwrap();
    let scenario = directory.path().join("future.json");
    fs::write(&scenario, serde_json::to_vec(&fiscal("fork", 5)).unwrap()).unwrap();
    let output_db = directory.path().join("branch.sqlite");
    let events = directory.path().join("branch.jsonl");
    let checkpoint = directory.path().join("branch.json");
    let csv = directory.path().join("branch.csv");
    success(
        command()
            .arg("branch")
            .arg(&source)
            .args(["--ticks", "6", "--json"])
            .arg("--scenario")
            .arg(&scenario)
            .arg("--database")
            .arg(&output_db)
            .arg("--events")
            .arg(&events)
            .arg("--checkpoint-out")
            .arg(&checkpoint)
            .arg("--csv")
            .arg(&csv),
    );
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    let actual = read_run(&checkpoint).unwrap();
    let mut expected = saved
        .restore_engine()
        .unwrap()
        .branch_scenario(read_scenario(&scenario).unwrap())
        .unwrap();
    for _ in 0..6 {
        expected.step().unwrap();
    }
    assert_eq!(actual.simulation, expected.snapshot().unwrap());
    assert_eq!(read_run(&events).unwrap(), actual);
    assert_eq!(read_run(&output_db).unwrap(), actual);
    let csv = fs::read_to_string(csv).unwrap();
    let columns = csv.lines().next().unwrap().split(',').count();
    assert!(csv.lines().all(|line| line.split(',').count() == columns));
    let before = fs::read(&scenario).unwrap();
    let invalid = command()
        .arg("branch")
        .arg(&source)
        .args(["--ticks", "0", "--overwrite"])
        .arg("--scenario")
        .arg(&scenario)
        .arg("--summary-out")
        .arg(&scenario)
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert_eq!(fs::read(&scenario).unwrap(), before);
    let invalid = command()
        .arg("resume")
        .arg(&source)
        .args(["--ticks", "1"])
        .arg("--scenario")
        .arg(&scenario)
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert_eq!(read_run(&source).unwrap(), saved);
    fs::write(&scenario, serde_json::to_vec(&fiscal("past", 4)).unwrap()).unwrap();
    let destination = directory.path().join("invalid.json");
    let invalid = command()
        .arg("branch")
        .arg(&source)
        .args(["--ticks", "1"])
        .arg("--scenario")
        .arg(&scenario)
        .arg("--state-out")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(!destination.exists());
}
