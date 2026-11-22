//! Fresh and resumed simulation execution with optional durable outputs.

use agentsociety_analytics::{EconomyStats, FlowTotals};
use agentsociety_core::*;
use agentsociety_simulation::{PolicyRuntime, RunCheckpoint, Simulation, StepTiming};
use agentsociety_storage::{JournalWriter, RunStore, read_run, read_scenario, write_checkpoint};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Initial conditions in JSON; explicit flags override the file.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Intervention schedule for a new run or branch; resume retains its saved schedule.
    #[arg(long)]
    scenario: Option<PathBuf>,
    /// Population (default: 100, or the config file value).
    #[arg(long)]
    agents: Option<usize>,
    /// Number of ticks to run; zero exports initial state.
    #[arg(long, default_value_t = 1000)]
    ticks: u64,
    /// Reproducible seed (default: 42, or the config file value).
    #[arg(long)]
    seed: Option<u64>,
    /// Decision policy: rule-based, llm, or hybrid (default: rule-based).
    #[arg(long)]
    policy: Option<PolicyKind>,
    /// Agents assigned the external policy; remaining agents use offline rules.
    #[arg(long)]
    llm_agents: Option<usize>,
    /// Provider settings JSON. Requires a build with --features llm.
    #[arg(long)]
    llm_config: Option<PathBuf>,
    /// Progress interval on stderr; zero disables progress.
    #[arg(long, default_value_t = 100)]
    report_every: u64,
    /// Stream a versioned header and structured events as JSONL.
    #[arg(long)]
    events: Option<PathBuf>,
    /// Export final engine state (rule-only exports also support resume).
    #[arg(long)]
    state_out: Option<PathBuf>,
    /// Save a checked checkpoint, including LLM budgets, periodically and at completion.
    #[arg(long)]
    checkpoint_out: Option<PathBuf>,
    /// Create a new SQLite run; resume an existing database with the resume subcommand.
    #[arg(long)]
    database: Option<PathBuf>,
    /// Tick interval for file and database checkpoints.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..=1_000_000))]
    checkpoint_every: u64,
    /// Export final statistics, configuration, and measured timings as JSON.
    #[arg(long)]
    summary_out: Option<PathBuf>,
    /// Export one numeric statistics row per tick.
    #[arg(long)]
    csv: Option<PathBuf>,
    /// Write the summary as JSON to stdout instead of a human-readable report.
    #[arg(long)]
    json: bool,
    /// Allow replacement of explicitly named output files.
    #[arg(long)]
    overwrite: bool,
}

#[derive(Serialize)]
struct Performance {
    build_profile: &'static str,
    elapsed_ms: f64,
    mean_tick_ms: f64,
    max_tick_ms: f64,
    mean_market_ms: f64,
    event_write_ms: f64,
    agent_ticks_per_second: f64,
    stages_mean_ms: std::collections::BTreeMap<&'static str, f64>,
}

#[derive(Serialize)]
struct Summary<'a> {
    format_version: u32,
    engine_version: &'static str,
    config: &'a SimulationConfig,
    stats: &'a EconomyStats,
    cumulative: &'a FlowTotals,
    total_events: u64,
    performance: Performance,
    llm_usage: LlmUsage,
}

pub(crate) fn execute(args: Args, resume: Option<PathBuf>, branch: bool) -> Result<()> {
    let (mut sim, mut policy_runtime, mut database) =
        prepare_run(&args, resume.as_deref(), branch)?;
    validate_output_paths(&args, resume.as_deref())?;
    let event_output = optional_output(args.events.as_deref(), args.overwrite)?;
    let mut state_writer = optional_output(args.state_out.as_deref(), args.overwrite)?;
    let mut summary_writer = optional_output(args.summary_out.as_deref(), args.overwrite)?;
    let mut csv_writer = optional_output(args.csv.as_deref(), args.overwrite)?;
    if let Some(store) = database.as_mut() {
        store.extend_run(args.ticks)?;
    } else if let Some(path) = &args.database {
        database = Some(RunStore::create(
            path,
            &sim,
            policy_runtime.snapshot(),
            args.ticks,
            args.checkpoint_every,
        )?);
    }
    let started = Instant::now();
    let writing = Instant::now();
    let mut journal = event_output
        .map(|writer| JournalWriter::new(writer, &sim, policy_runtime.snapshot(), args.ticks))
        .transpose()?;
    let mut event_write_time = if journal.is_some() {
        writing.elapsed()
    } else {
        Duration::ZERO
    };
    let mut checkpoint_written = false;
    if let Some(writer) = csv_writer.as_mut() {
        writeln!(
            writer,
            "tick,agents,total_money,average_wealth,gini,hungry_agents,food_price,wood_price,metal_price,goods_price,trades,trade_value,food_produced,wood_produced,metal_produced,goods_produced,food_consumed,goods_consumed,wood_inputs,metal_inputs,negotiations,negotiated_trades,cooperations,goal_changes,organizations,organization_cash,treasury,open_contracts,group_productions,contract_deliveries,contract_defaults,taxes,redistributed,food_destroyed,wood_destroyed,metal_destroyed,goods_destroyed"
        )?;
    }
    let mut tick_time = Duration::ZERO;
    let mut max_tick_time = Duration::ZERO;
    let mut market_time = Duration::ZERO;
    let mut stages = StepTiming::default();
    for _ in 0..args.ticks {
        let report = policy_runtime
            .step(&mut sim)
            .context("advance simulation tick")?;
        tick_time += report.timing.total;
        max_tick_time = max_tick_time.max(report.timing.total);
        market_time += report.timing.market;
        stages.accumulate(report.timing);
        if let Some(writer) = journal.as_mut() {
            let writing = Instant::now();
            writer.append(&sim, &report, policy_runtime.snapshot())?;
            event_write_time += writing.elapsed();
        }
        if let Some(store) = database.as_mut() {
            store.append(&sim, &report, policy_runtime.snapshot())?;
        }
        if let Some(writer) = csv_writer.as_mut() {
            csv_row(writer, &report.stats)?;
        }
        if let Some(path) = &args.checkpoint_out
            && sim.world().tick().is_multiple_of(args.checkpoint_every)
        {
            write_checkpoint(
                path,
                &RunCheckpoint::capture(&sim, &policy_runtime)?,
                args.overwrite || checkpoint_written,
            )?;
            checkpoint_written = true;
        }
        if !args.json
            && args.report_every > 0
            && report.stats.tick.is_multiple_of(args.report_every)
        {
            eprintln!(
                "tick={:>6} trades={:>5} cash={} gini={:.4} hungry={}",
                report.stats.tick,
                report.stats.flows.trade_count,
                report.stats.total_money,
                report.stats.gini,
                report.stats.hungry_agents
            );
        }
    }
    if journal.is_some() || database.is_some() || args.checkpoint_out.is_some() {
        let checkpoint = RunCheckpoint::capture(&sim, &policy_runtime)?;
        if let Some(writer) = journal.as_mut() {
            let writing = Instant::now();
            writer.finish(&checkpoint)?;
            event_write_time += writing.elapsed();
        }
        if let Some(store) = database.as_mut() {
            store.finish(&checkpoint)?;
        }
        if let Some(path) = &args.checkpoint_out {
            write_checkpoint(path, &checkpoint, args.overwrite || checkpoint_written)?;
        }
    }
    if let Some(writer) = csv_writer.as_mut() {
        writer.flush().context("flush statistics CSV")?;
    }
    let event_count = sim.next_event_sequence();
    let elapsed = started.elapsed();
    let denominator = args.ticks.max(1) as f64;
    let summary = Summary {
        format_version: FORMAT_VERSION,
        engine_version: ENGINE_VERSION,
        config: sim.config(),
        stats: sim.stats(),
        cumulative: sim.cumulative(),
        total_events: event_count,
        performance: Performance {
            build_profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            elapsed_ms: elapsed.as_secs_f64() * 1000.0,
            mean_tick_ms: tick_time.as_secs_f64() * 1000.0 / denominator,
            max_tick_ms: max_tick_time.as_secs_f64() * 1000.0,
            mean_market_ms: market_time.as_secs_f64() * 1000.0 / denominator,
            event_write_ms: event_write_time.as_secs_f64() * 1000.0,
            agent_ticks_per_second: if elapsed.is_zero() {
                0.0
            } else {
                sim.world().len() as f64 * args.ticks as f64 / elapsed.as_secs_f64()
            },
            stages_mean_ms: stages.mean_ms(args.ticks),
        },
        llm_usage: policy_runtime.usage(),
    };
    if let Some(writer) = state_writer.as_mut() {
        pretty_json(writer, &sim.snapshot()?)?;
    }
    if let Some(writer) = summary_writer.as_mut() {
        pretty_json(writer, &summary)?;
    }
    print_summary(&args, &sim, &summary)
}

fn prepare_run(
    args: &Args,
    resume: Option<&Path>,
    branch: bool,
) -> Result<(Simulation, PolicyRuntime, Option<RunStore>)> {
    ensure!(
        !branch || args.scenario.is_some(),
        "branch requires --scenario with future interventions"
    );
    if let Some(path) = resume {
        ensure!(
            branch || args.scenario.is_none(),
            "resume preserves its schedule; use branch to change future interventions"
        );
        ensure!(
            args.config.is_none()
                && args.agents.is_none()
                && args.seed.is_none()
                && args.policy.is_none()
                && args.llm_agents.is_none(),
            "resume preserves initial conditions; config, agents, seed and policy overrides are not allowed"
        );
        let (checkpoint, store) = if is_sqlite(path)? && !branch {
            ensure!(
                args.database.is_none(),
                "resume appends to its source database; --database cannot name another output"
            );
            let mut store = RunStore::open(path, true)?;
            (store.recover()?.checkpoint, Some(store))
        } else {
            (read_run(path)?, None)
        };
        if let Some(settings_path) = &args.llm_config {
            let settings: agentsociety_simulation::LlmSettings =
                serde_json::from_reader(File::open(settings_path)?)?;
            ensure!(
                checkpoint
                    .policy
                    .external
                    .as_ref()
                    .is_some_and(|external| external.settings == settings),
                "resume must use the recorded LLM settings and call limits"
            );
        }
        let (mut simulation, runtime) = checkpoint.restore()?;
        if branch {
            simulation = simulation.branch_scenario(read_scenario(
                args.scenario.as_deref().expect("required scenario"),
            )?)?;
        }
        simulation
            .world()
            .tick()
            .checked_add(args.ticks)
            .context("resumed tick range overflows")?;
        return Ok((simulation, runtime, store));
    }
    let mut config = if let Some(path) = &args.config {
        serde_json::from_reader(
            File::open(path).with_context(|| format!("open config {}", path.display()))?,
        )
        .with_context(|| format!("parse config {}", path.display()))?
    } else {
        SimulationConfig::default()
    };
    if let Some(agents) = args.agents {
        config.agents = agents;
    }
    if let Some(seed) = args.seed {
        config.seed = seed;
    }
    if let Some(policy) = args.policy {
        config.policy = policy;
    }
    if let Some(agents) = args.llm_agents {
        config.llm_agents = agents;
    }
    if let Some(path) = &args.scenario {
        config.scenario = read_scenario(path)?;
    }
    config.validate()?;
    let runtime = policy_runtime(args.llm_config.as_deref())?;
    runtime.validate(&config)?;
    Ok((Simulation::new(config)?, runtime, None))
}

fn print_summary(args: &Args, sim: &Simulation, summary: &Summary<'_>) -> Result<()> {
    let event_count = sim.next_event_sequence();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    if args.json {
        pretty_json(&mut output, &summary)?;
    } else {
        writeln!(
            output,
            "AgentSociety {} | agents={} ticks={} seed={}",
            ENGINE_VERSION,
            sim.world().len(),
            sim.world().tick(),
            sim.config().seed
        )?;
        writeln!(
            output,
            "Money supply: {} minor units (conservation checked every tick)",
            sim.stats().total_money
        )?;
        writeln!(
            output,
            "Trades: {} | turnover: {} | mean wealth: {:.2} | Gini: {:.4}",
            sim.cumulative().trade_count,
            sim.cumulative().trade_value,
            sim.stats().average_wealth,
            sim.stats().gini
        )?;
        writeln!(
            output,
            "Negotiations: {} | negotiated trades: {} | joint batches: {} | goal changes: {} | open proposals: {}",
            sim.cumulative().negotiations,
            sim.cumulative().negotiated_trades,
            sim.cumulative().cooperations,
            sim.cumulative().goal_changes,
            sim.proposals().proposals().count()
        )?;
        writeln!(
            output,
            "Organizations: {} | shared batches: {} | contract installments: {} | defaults: {} | treasury: {} | tax paid: {} | redistributed: {}",
            sim.stats().organizations,
            sim.cumulative().group_productions,
            sim.cumulative().contract_deliveries,
            sim.cumulative().contract_defaults,
            sim.stats().treasury_money,
            sim.cumulative().taxes,
            sim.cumulative().redistributed
        )?;
        writeln!(
            output,
            "Resource   Price    Stock    Produced  Household  Factory-in  Destroyed  Traded"
        )?;
        for resource in Resource::ALL {
            let i = resource.index();
            writeln!(
                output,
                "{:<10} {:>6} {:>9} {:>10} {:>10} {:>11} {:>10} {:>9}",
                resource.to_string(),
                sim.stats().prices[i],
                sim.stats().inventories[i],
                sim.cumulative().produced[i],
                sim.cumulative().household_consumed[i],
                sim.cumulative().production_inputs[i],
                sim.cumulative().destroyed[i],
                sim.cumulative().traded[i]
            )?;
        }
        writeln!(
            output,
            "Mean tick: {:.3} ms | max: {:.3} ms | market: {:.3} ms | elapsed: {:.1} ms ({})",
            summary.performance.mean_tick_ms,
            summary.performance.max_tick_ms,
            summary.performance.mean_market_ms,
            summary.performance.elapsed_ms,
            summary.performance.build_profile
        )?;
        writeln!(
            output,
            "Events: {} | rejected actions: {} | hungry agents: {}",
            event_count,
            sim.cumulative().rejected_actions,
            sim.stats().hungry_agents
        )?;
        if sim.config().policy != PolicyKind::RuleBased {
            writeln!(
                output,
                "LLM scheduled calls: {} | accepted: {} | rule fallbacks: {} | reported tokens: {} in / {} out ({} usage reports)",
                summary.llm_usage.attempts,
                summary.llm_usage.accepted,
                summary.llm_usage.fallbacks,
                summary.llm_usage.input_tokens,
                summary.llm_usage.output_tokens,
                summary.llm_usage.usage_reports
            )?;
        }
    }
    Ok(())
}

pub(crate) fn optional_output(
    path: Option<&Path>,
    overwrite: bool,
) -> Result<Option<BufWriter<File>>> {
    path.map(|path| {
        let file = if overwrite {
            let absolute = std::path::absolute(path)?;
            let temporary = tempfile::NamedTempFile::new_in(
                absolute.parent().context("output needs parent directory")?,
            )?;
            // Replace the entry instead of truncating through a possible hard link.
            temporary.persist(path).map_err(|error| error.error)
        } else {
            OpenOptions::new().write(true).create_new(true).open(path)
        }
        .with_context(|| {
            format!(
                "create output {} (use --overwrite to replace existing output)",
                path.display()
            )
        })?;
        Ok(BufWriter::new(file))
    })
    .transpose()
}

fn validate_output_paths(args: &Args, resume: Option<&Path>) -> Result<()> {
    let inputs: Vec<&Path> = [
        args.config.as_deref(),
        args.llm_config.as_deref(),
        args.scenario.as_deref(),
        resume,
    ]
    .into_iter()
    .flatten()
    .collect();
    let outputs: Vec<&Path> = [
        &args.events,
        &args.state_out,
        &args.summary_out,
        &args.csv,
        &args.checkpoint_out,
        &args.database,
    ]
    .into_iter()
    .flatten()
    .map(PathBuf::as_path)
    .collect();
    validate_paths(&inputs, &outputs, args.overwrite)?;
    if let Some(path) = &args.database {
        ensure!(
            !path.exists(),
            "database already exists; use resume instead of overwriting it"
        );
    }
    let database_path = args
        .database
        .as_deref()
        .or(resume.filter(|path| is_sqlite(path).unwrap_or(false)));
    if let Some(database) = database_path {
        let base = normalized_path(database)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = format!("{base}{suffix}");
            for output in &outputs {
                ensure!(
                    normalized_path(output)? != sidecar,
                    "output must not replace a SQLite sidecar file"
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_paths(
    input_paths: &[&Path],
    output_paths: &[&Path],
    overwrite: bool,
) -> Result<()> {
    let inputs = input_paths
        .iter()
        .map(|path| normalized_path(path))
        .collect::<Result<Vec<_>>>()?;
    let mut reserved = Vec::new();
    for path in input_paths {
        if is_sqlite(path)? {
            let base = normalized_path(path)?;
            for suffix in ["-wal", "-shm", "-journal"] {
                reserved.push(format!("{base}{suffix}"));
            }
        }
    }
    let mut outputs = BTreeSet::new();
    for path in output_paths {
        let absolute = std::path::absolute(path)?;
        let parent = absolute
            .parent()
            .context("output requires a parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
        let key = normalized_path(path)?;
        ensure!(
            !inputs.contains(&key),
            "an output must not overwrite an input config or saved run"
        );
        ensure!(
            !reserved.contains(&key),
            "output must not replace a SQLite sidecar file"
        );
        if !outputs.insert(key) {
            bail!("output paths must be distinct");
        }
        ensure!(
            overwrite || !path.exists(),
            "output {} already exists; use --overwrite",
            path.display()
        );
    }
    Ok(())
}

fn normalized_path(path: &Path) -> Result<String> {
    let absolute = std::path::absolute(path)?;
    let normalized = if path.exists() {
        fs::canonicalize(path)?
    } else {
        fs::canonicalize(absolute.parent().context("path needs a parent")?)?
            .join(absolute.file_name().context("path needs a filename")?)
    };
    Ok(if cfg!(windows) {
        normalized.to_string_lossy().to_lowercase()
    } else {
        normalized.to_string_lossy().into_owned()
    })
}

pub(crate) fn is_sqlite(path: &Path) -> Result<bool> {
    let mut bytes = [0_u8; 16];
    let mut file = File::open(path).with_context(|| format!("open input {}", path.display()))?;
    Ok(file.read(&mut bytes)? == bytes.len() && &bytes == b"SQLite format 3\0")
}

pub(crate) fn policy_runtime(path: Option<&Path>) -> Result<PolicyRuntime> {
    let Some(path) = path else {
        return Ok(PolicyRuntime::default());
    };
    #[cfg(feature = "llm")]
    {
        let settings = serde_json::from_reader(
            File::open(path).with_context(|| format!("open LLM settings {}", path.display()))?,
        )
        .context("parse LLM settings")?;
        Ok(PolicyRuntime::with_llm(settings)?)
    }
    #[cfg(not(feature = "llm"))]
    {
        let _ = path;
        bail!("--llm-config requires rebuilding with --features llm")
    }
}

pub(crate) fn pretty_json(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer_pretty(&mut *writer, value).context("serialize JSON output")?;
    writer.write_all(b"\n")?;
    writer.flush().context("flush JSON output")?;
    Ok(())
}

fn csv_row(writer: &mut impl Write, stats: &EconomyStats) -> Result<()> {
    let flows = &stats.flows;
    write!(
        writer,
        "{},{},{},{:.6},{:.9},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        stats.tick,
        stats.agent_count,
        stats.total_money,
        stats.average_wealth,
        stats.gini,
        stats.hungry_agents,
        stats.prices[0],
        stats.prices[1],
        stats.prices[2],
        stats.prices[3],
        flows.trade_count,
        flows.trade_value,
        flows.produced[0],
        flows.produced[1],
        flows.produced[2],
        flows.produced[3],
        flows.household_consumed[0],
        flows.household_consumed[3],
        flows.production_inputs[1],
        flows.production_inputs[2],
        flows.negotiations,
        flows.negotiated_trades,
        flows.cooperations,
        flows.goal_changes
    )?;
    writeln!(
        writer,
        ",{},{},{},{},{},{},{},{},{},{},{},{},{}",
        stats.organizations,
        stats.organization_cash,
        stats.treasury_money,
        stats.open_contracts,
        flows.group_productions,
        flows.contract_deliveries,
        flows.contract_defaults,
        flows.taxes,
        flows.redistributed,
        flows.destroyed[0],
        flows.destroyed[1],
        flows.destroyed[2],
        flows.destroyed[3]
    )?;
    Ok(())
}
