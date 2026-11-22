//! Reproducible experiment matrices with per-run artifacts and group comparisons.
use crate::run::{optional_output, pretty_json, validate_paths};
use agentsociety_analytics::{EconomyStats, FlowTotals};
use agentsociety_core::*;
use agentsociety_simulation::{LlmSettings, PolicyRuntime, RunCheckpoint, Simulation, StepTiming};
use agentsociety_social::{SocialStats, analyze};
use agentsociety_storage::{JournalWriter, checksum, read_scenario, write_checkpoint};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Base initial conditions; matrix dimensions override the corresponding fields.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Named intervention JSON files, crossed with every seed/population/policy.
    #[arg(long, value_delimiter = ',')]
    scenarios: Vec<PathBuf>,
    /// Also run an otherwise identical economy with no scheduled interventions.
    #[arg(long)]
    include_baseline: bool,
    #[arg(long, value_delimiter = ',', default_value = "42,43,44")]
    seeds: Vec<u64>,
    #[arg(long, value_delimiter = ',', default_value = "100")]
    populations: Vec<usize>,
    #[arg(long, value_delimiter = ',', default_value = "rule-based")]
    policies: Vec<PolicyKind>,
    #[arg(long, default_value_t = 1000)]
    ticks: u64,
    #[arg(long, default_value_t = 4)]
    llm_agents: usize,
    #[arg(long)]
    llm_config: Option<PathBuf>,
    /// Results, per-run summaries and checked checkpoints are written here.
    #[arg(long, default_value = "runs/experiments")]
    out_dir: PathBuf,
    /// Record rule-only runs too; external-policy runs always record decisions for replay.
    #[arg(long)]
    record_events: bool,
    #[arg(long)]
    overwrite: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Serialize)]
struct Distribution {
    minimum: u128,
    p10: f64,
    median: f64,
    p90: f64,
    maximum: u128,
    top_decile_share: f64,
}

fn distribution(mut values: Vec<u128>) -> Distribution {
    values.sort_unstable();
    let quantile = |fraction: f64| {
        if values.is_empty() {
            return 0.0;
        }
        let rank = fraction * (values.len() - 1) as f64;
        let lower = rank.floor() as usize;
        let upper = rank.ceil() as usize;
        values[lower] as f64 * (1.0 - rank.fract()) + values[upper] as f64 * rank.fract()
    };
    let total: f64 = values.iter().map(|&value| value as f64).sum();
    let upper: f64 = values
        .iter()
        .rev()
        .take(values.len().div_ceil(10))
        .map(|&value| value as f64)
        .sum();
    Distribution {
        minimum: values.first().copied().unwrap_or(0),
        p10: quantile(0.1),
        median: quantile(0.5),
        p90: quantile(0.9),
        maximum: values.last().copied().unwrap_or(0),
        top_decile_share: if total == 0.0 { 0.0 } else { upper / total },
    }
}

#[derive(Serialize)]
struct RunResult {
    run_id: usize,
    config: SimulationConfig,
    stats: EconomyStats,
    cumulative: FlowTotals,
    wealth_distribution: Distribution,
    social: SocialStats,
    memory_records: usize,
    llm_usage: LlmUsage,
    elapsed_ms: f64,
    mean_tick_ms: f64,
    mean_market_ms: f64,
    max_tick_ms: f64,
    event_write_ms: f64,
    stages_mean_ms: BTreeMap<&'static str, f64>,
    state_sha256: String,
    checkpoint_file: String,
    events_file: Option<String>,
}

#[derive(Debug, Serialize)]
struct Comparison {
    scenario: String,
    population: usize,
    policy: String,
    runs: usize,
    mean_gini: f64,
    min_gini: f64,
    max_gini: f64,
    mean_trades: f64,
    mean_negotiated_trades: f64,
    mean_cooperations: f64,
    mean_goal_changes: f64,
    mean_group_productions: f64,
    mean_contract_deliveries: f64,
    mean_contract_defaults: f64,
    mean_taxes: f64,
    mean_redistributed: f64,
    mean_food_price: f64,
    mean_hungry_agents: f64,
    mean_wealth: f64,
    mean_top_decile_share: f64,
    mean_communities: f64,
    mean_elapsed_ms: f64,
    mean_llm_attempts: f64,
    mean_input_tokens: f64,
    mean_output_tokens: f64,
}

fn comparisons(results: &[RunResult]) -> Vec<Comparison> {
    let mut groups: BTreeMap<(String, usize, String), Vec<&RunResult>> = BTreeMap::new();
    for result in results {
        groups
            .entry((
                result.config.scenario.name.clone(),
                result.config.agents,
                label(result.config.policy).into(),
            ))
            .or_default()
            .push(result);
    }
    groups
        .into_iter()
        .map(|((scenario, population, policy), runs)| {
            let mean = |value: fn(&RunResult) -> f64| {
                runs.iter().map(|run| value(run)).sum::<f64>() / runs.len() as f64
            };
            Comparison {
                scenario,
                population,
                policy,
                runs: runs.len(),
                mean_gini: mean(|run| run.stats.gini),
                min_gini: runs
                    .iter()
                    .map(|run| run.stats.gini)
                    .fold(f64::INFINITY, f64::min),
                max_gini: runs
                    .iter()
                    .map(|run| run.stats.gini)
                    .fold(f64::NEG_INFINITY, f64::max),
                mean_trades: mean(|run| run.cumulative.trade_count as f64),
                mean_negotiated_trades: mean(|run| run.cumulative.negotiated_trades as f64),
                mean_cooperations: mean(|run| run.cumulative.cooperations as f64),
                mean_goal_changes: mean(|run| run.cumulative.goal_changes as f64),
                mean_group_productions: mean(|run| run.cumulative.group_productions as f64),
                mean_contract_deliveries: mean(|run| run.cumulative.contract_deliveries as f64),
                mean_contract_defaults: mean(|run| run.cumulative.contract_defaults as f64),
                mean_taxes: mean(|run| run.cumulative.taxes as f64),
                mean_redistributed: mean(|run| run.cumulative.redistributed as f64),
                mean_food_price: mean(|run| run.stats.prices[Resource::Food.index()] as f64),
                mean_hungry_agents: mean(|run| run.stats.hungry_agents as f64),
                mean_wealth: mean(|run| run.stats.average_wealth),
                mean_top_decile_share: mean(|run| run.wealth_distribution.top_decile_share),
                mean_communities: mean(|run| run.social.communities as f64),
                mean_elapsed_ms: mean(|run| run.elapsed_ms),
                mean_llm_attempts: mean(|run| run.llm_usage.attempts as f64),
                mean_input_tokens: mean(|run| run.llm_usage.input_tokens as f64),
                mean_output_tokens: mean(|run| run.llm_usage.output_tokens as f64),
            }
        })
        .collect()
}

fn label(policy: PolicyKind) -> &'static str {
    match policy {
        PolicyKind::RuleBased => "rule_based",
        PolicyKind::Llm => "llm",
        PolicyKind::Hybrid => "hybrid",
    }
}

fn runtime(config: &SimulationConfig, settings: Option<&LlmSettings>) -> Result<PolicyRuntime> {
    if config.policy == PolicyKind::RuleBased {
        return Ok(PolicyRuntime::default());
    }
    let settings =
        settings.context("external experiment policies require --llm-config and --features llm")?;
    #[cfg(feature = "llm")]
    {
        Ok(PolicyRuntime::with_llm(settings.clone())?)
    }
    #[cfg(not(feature = "llm"))]
    {
        let _ = settings;
        anyhow::bail!("external experiment policies require --features llm")
    }
}

#[derive(Serialize)]
struct Report<'a> {
    format_version: u32,
    engine_version: &'static str,
    ticks_per_run: Tick,
    completed: bool,
    requested_runs: usize,
    provider_settings: &'a Option<LlmSettings>,
    results: &'a [RunResult],
    comparisons: Vec<Comparison>,
}

pub(crate) fn execute(args: Args) -> Result<()> {
    let base: SimulationConfig = if let Some(path) = &args.config {
        serde_json::from_reader(File::open(path)?)?
    } else {
        SimulationConfig::default()
    };
    let mut scenarios = if args.scenarios.is_empty() {
        vec![base.scenario.clone()]
    } else {
        args.scenarios
            .iter()
            .map(|path| read_scenario(path))
            .collect::<Result<Vec<_>>>()?
    };
    if args.include_baseline && !scenarios.iter().any(|s| s == &ScenarioConfig::default()) {
        scenarios.insert(0, ScenarioConfig::default());
    }
    let mut names = std::collections::BTreeSet::new();
    for scenario in &scenarios {
        scenario.validate()?;
        ensure!(
            names.insert(scenario.name.clone()),
            "scenario names must be unique"
        );
    }
    let count = args
        .seeds
        .len()
        .checked_mul(args.populations.len())
        .and_then(|count| count.checked_mul(args.policies.len()))
        .and_then(|count| count.checked_mul(scenarios.len()))
        .context("experiment matrix size overflow")?;
    ensure!(
        (1..=1000).contains(&count),
        "experiment matrix must contain 1..=1000 runs"
    );
    let settings: Option<LlmSettings> = args
        .llm_config
        .as_ref()
        .map(|path| -> Result<_> { Ok(serde_json::from_reader(File::open(path)?)?) })
        .transpose()?;
    if let Some(settings) = &settings {
        settings.validate()?;
    }
    let mut configs = Vec::with_capacity(count);
    for scenario in scenarios {
        for &population in &args.populations {
            for &policy in &args.policies {
                for &seed in &args.seeds {
                    let config = SimulationConfig {
                        agents: population,
                        seed,
                        policy,
                        llm_agents: args.llm_agents,
                        scenario: scenario.clone(),
                        ..base.clone()
                    };
                    config.validate()?;
                    runtime(&config, settings.as_ref())?.validate(&config)?;
                    configs.push(config);
                }
            }
        }
    }
    let report_path = args.out_dir.join("report.json");
    let csv_path = args.out_dir.join("results.csv");
    let mut paths = vec![report_path.clone(), csv_path.clone()];
    for (index, config) in configs.iter().enumerate() {
        paths.push(
            args.out_dir
                .join(format!("run-{:04}.checkpoint.json", index + 1)),
        );
        paths.push(
            args.out_dir
                .join(format!("run-{:04}.summary.json", index + 1)),
        );
        if args.record_events || config.policy != PolicyKind::RuleBased {
            paths.push(args.out_dir.join(format!("run-{:04}.jsonl", index + 1)));
        }
    }
    let inputs: Vec<&Path> = [args.config.as_deref(), args.llm_config.as_deref()]
        .into_iter()
        .flatten()
        .chain(args.scenarios.iter().map(PathBuf::as_path))
        .collect();
    validate_paths(
        &inputs,
        &paths.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
        args.overwrite,
    )?;
    let mut csv = optional_output(Some(&csv_path), args.overwrite)?.expect("requested CSV");
    writeln!(
        csv,
        "run_id,seed,population,policy,ticks,total_money,trades,trade_value,gini,hungry_agents,average_wealth,top_decile_share,communities,elapsed_ms,mean_tick_ms,mean_market_ms,llm_attempts,input_tokens,output_tokens,state_sha256,negotiations,negotiated_trades,cooperations,goal_changes,scenario,organizations,group_productions,contract_deliveries,contract_defaults,taxes,redistributed,treasury"
    )?;
    let mut results = Vec::with_capacity(count);
    for (index, config) in configs.into_iter().enumerate() {
        let mut runtime = runtime(&config, settings.as_ref())?;
        let mut sim = Simulation::new(config.clone())?;
        let events_file = (args.record_events || config.policy != PolicyKind::RuleBased)
            .then(|| format!("run-{:04}.jsonl", index + 1));
        let events_path = events_file.as_ref().map(|name| args.out_dir.join(name));
        let mut journal = optional_output(events_path.as_deref(), args.overwrite)?
            .map(|writer| JournalWriter::new(writer, &sim, runtime.snapshot(), args.ticks))
            .transpose()?;
        let started = Instant::now();
        let mut tick_time = Duration::ZERO;
        let mut market_time = Duration::ZERO;
        let mut max_tick = Duration::ZERO;
        let mut event_write_time = Duration::ZERO;
        let mut stages = StepTiming::default();
        for _ in 0..args.ticks {
            let report = runtime.step(&mut sim)?;
            tick_time += report.timing.total;
            market_time += report.timing.market;
            max_tick = max_tick.max(report.timing.total);
            stages.accumulate(report.timing);
            if let Some(writer) = journal.as_mut() {
                let writing = Instant::now();
                writer.append(&sim, &report, runtime.snapshot())?;
                event_write_time += writing.elapsed();
            }
        }
        let elapsed = started.elapsed();
        let checkpoint = RunCheckpoint::capture(&sim, &runtime)?;
        if let Some(writer) = journal.as_mut() {
            writer.finish(&checkpoint)?;
        }
        let checkpoint_file = format!("run-{:04}.checkpoint.json", index + 1);
        write_checkpoint(
            &args.out_dir.join(&checkpoint_file),
            &checkpoint,
            args.overwrite,
        )?;
        let equity = sim.institutions().equity(&sim.stats().prices);
        let result = RunResult {
            run_id: index + 1,
            config,
            stats: sim.stats().clone(),
            cumulative: sim.cumulative().clone(),
            wealth_distribution: distribution(
                sim.world()
                    .agents()
                    .map(|agent| {
                        agent.account.wealth(&sim.stats().prices)
                            + equity.get(&agent.id).copied().unwrap_or(0)
                    })
                    .collect(),
            ),
            social: analyze(sim.world().agents()),
            memory_records: sim.world().agents().map(|agent| agent.memories.len()).sum(),
            llm_usage: runtime.usage(),
            elapsed_ms: elapsed.as_secs_f64() * 1000.0,
            mean_tick_ms: tick_time.as_secs_f64() * 1000.0 / args.ticks.max(1) as f64,
            mean_market_ms: market_time.as_secs_f64() * 1000.0 / args.ticks.max(1) as f64,
            max_tick_ms: max_tick.as_secs_f64() * 1000.0,
            event_write_ms: event_write_time.as_secs_f64() * 1000.0,
            stages_mean_ms: stages.mean_ms(args.ticks),
            state_sha256: checksum(&checkpoint.simulation)?,
            checkpoint_file,
            events_file,
        };
        let summary_path = args
            .out_dir
            .join(format!("run-{:04}.summary.json", index + 1));
        pretty_json(
            &mut optional_output(Some(&summary_path), args.overwrite)?
                .expect("requested run summary"),
            &result,
        )?;
        writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{:.9},{},{:.6},{:.9},{},{:.6},{:.6},{:.6},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            result.run_id,
            result.config.seed,
            result.config.agents,
            label(result.config.policy),
            result.stats.tick,
            result.stats.total_money,
            result.cumulative.trade_count,
            result.cumulative.trade_value,
            result.stats.gini,
            result.stats.hungry_agents,
            result.stats.average_wealth,
            result.wealth_distribution.top_decile_share,
            result.social.communities,
            result.elapsed_ms,
            result.mean_tick_ms,
            result.mean_market_ms,
            result.llm_usage.attempts,
            result.llm_usage.input_tokens,
            result.llm_usage.output_tokens,
            result.state_sha256,
            result.cumulative.negotiations,
            result.cumulative.negotiated_trades,
            result.cumulative.cooperations,
            result.cumulative.goal_changes,
            result.config.scenario.name,
            result.stats.organizations,
            result.cumulative.group_productions,
            result.cumulative.contract_deliveries,
            result.cumulative.contract_defaults,
            result.cumulative.taxes,
            result.cumulative.redistributed,
            result.stats.treasury_money
        )?;
        csv.flush()?;
        if !args.json {
            eprintln!(
                "experiment {}/{}: agents={} seed={} policy={} trades={} gini={:.4}",
                index + 1,
                count,
                result.config.agents,
                result.config.seed,
                label(result.config.policy),
                result.cumulative.trade_count,
                result.stats.gini
            );
        }
        results.push(result);
        let report = Report {
            format_version: FORMAT_VERSION,
            engine_version: ENGINE_VERSION,
            ticks_per_run: args.ticks,
            completed: results.len() == count,
            requested_runs: count,
            provider_settings: &settings,
            comparisons: comparisons(&results),
            results: &results,
        };
        pretty_json(
            &mut optional_output(Some(&report_path), args.overwrite || index > 0)?
                .expect("requested report"),
            &report,
        )?;
    }
    let groups = comparisons(&results);
    let mut stdout = std::io::stdout().lock();
    if args.json {
        pretty_json(
            &mut stdout,
            &Report {
                format_version: FORMAT_VERSION,
                engine_version: ENGINE_VERSION,
                ticks_per_run: args.ticks,
                completed: true,
                requested_runs: count,
                provider_settings: &settings,
                results: &results,
                comparisons: groups,
            },
        )?;
    } else {
        writeln!(
            stdout,
            "Scenario Population Policy     Runs Mean Gini Mean trades Mean hungry Mean ms"
        )?;
        for group in groups {
            writeln!(
                stdout,
                "{:<20} {:>10} {:<10} {:>4} {:>9.4} {:>11.1} {:>11.1} {:>7.2}",
                group.scenario,
                group.population,
                group.policy,
                group.runs,
                group.mean_gini,
                group.mean_trades,
                group.mean_hungry_agents,
                group.mean_elapsed_ms
            )?;
        }
        writeln!(stdout, "Report: {}", report_path.display())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::distribution;
    #[test]
    fn wealth_comparison_handles_known_distribution_and_empty_population() {
        let values = distribution(vec![30, 0, 20, 10]);
        assert_eq!(values.minimum, 0);
        assert_eq!(values.maximum, 30);
        assert_eq!(values.median, 15.0);
        assert!((values.p10 - 3.0).abs() < 1e-10);
        assert!((values.p90 - 27.0).abs() < 1e-10);
        assert_eq!(values.top_decile_share, 0.5);
        assert_eq!(distribution(vec![]).median, 0.0);
        assert_eq!(distribution(vec![0; 10]).top_decile_share, 0.0);
    }
}
