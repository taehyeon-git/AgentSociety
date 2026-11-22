use crate::run::{is_sqlite, optional_output, pretty_json, validate_paths};
use agentsociety_core::*;
use agentsociety_storage::{EventQuery, RunStore, checksum, replay_journal, write_checkpoint};
use anyhow::Result;
use serde_json::json;
use std::{
    fs::File,
    io::{BufReader, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, clap::Args)]
pub(crate) struct ReplayArgs {
    input: PathBuf,
    #[arg(long)]
    state_out: Option<PathBuf>,
    #[arg(long)]
    checkpoint_out: Option<PathBuf>,
    #[arg(long)]
    summary_out: Option<PathBuf>,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    overwrite: bool,
}

pub(crate) fn replay(args: ReplayArgs) -> Result<()> {
    let outputs: Vec<&Path> = [&args.state_out, &args.checkpoint_out, &args.summary_out]
        .into_iter()
        .flatten()
        .map(PathBuf::as_path)
        .collect();
    validate_paths(&[&args.input], &outputs, args.overwrite)?;
    let result = if is_sqlite(&args.input)? {
        RunStore::open(&args.input, false)?.replay()?
    } else {
        replay_journal(BufReader::new(File::open(&args.input)?))?
    };
    let summary = json!({
        "format_version": FORMAT_VERSION, "engine_version": ENGINE_VERSION,
        "replayed_ticks": result.replayed_ticks, "verified_events": result.verified_events,
        "final_tick": result.checkpoint.simulation.world.tick,
        "state_sha256": checksum(&result.checkpoint.simulation)?,
        "stats": result.checkpoint.simulation.stats,
        "llm_usage": result.checkpoint.policy.usage(),
    });
    if let Some(mut writer) = optional_output(args.state_out.as_deref(), args.overwrite)? {
        pretty_json(&mut writer, &result.checkpoint.simulation)?;
    }
    if let Some(path) = &args.checkpoint_out {
        write_checkpoint(path, &result.checkpoint, args.overwrite)?;
    }
    if let Some(mut writer) = optional_output(args.summary_out.as_deref(), args.overwrite)? {
        pretty_json(&mut writer, &summary)?;
    }
    let mut stdout = std::io::stdout().lock();
    if args.json {
        pretty_json(&mut stdout, &summary)?;
    } else {
        writeln!(
            stdout,
            "Replay verified: {} ticks, {} events, final tick {}; state SHA-256 {}",
            result.replayed_ticks,
            result.verified_events,
            result.checkpoint.simulation.world.tick,
            checksum(&result.checkpoint.simulation)?
        )?;
    }
    Ok(())
}

#[derive(Debug, clap::Args)]
pub(crate) struct InspectArgs {
    database: PathBuf,
    #[arg(long)]
    agent: Option<u64>,
    #[arg(long, default_value_t = 0)]
    from_tick: u64,
    #[arg(long, default_value_t = i64::MAX as u64)]
    to_tick: u64,
    #[arg(long, default_value_t = 100)]
    limit: usize,
    #[arg(long)]
    json: bool,
}

pub(crate) fn inspect(args: InspectArgs) -> Result<()> {
    let records = RunStore::open(&args.database, false)?.query_events(&EventQuery {
        agent: args.agent.map(AgentId),
        from_tick: args.from_tick,
        to_tick: args.to_tick,
        limit: args.limit,
    })?;
    let mut stdout = std::io::stdout().lock();
    if args.json {
        pretty_json(&mut stdout, &records)?;
    } else {
        for record in records {
            writeln!(
                stdout,
                "tick={} sequence={} {}",
                record.tick,
                record.sequence,
                serde_json::to_string(&record.event)?
            )?;
        }
    }
    Ok(())
}
