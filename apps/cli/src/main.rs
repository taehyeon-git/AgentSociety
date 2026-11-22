//! CLI entry point; simulation and persistence stay in independently testable crates.
mod experiment;
mod persistence;
mod run;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "AgentSociety simulation, resume, replay and experiments",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    args: run::Args,
}

#[derive(Subcommand)]
enum Command {
    /// Start a new simulation (also the default without a subcommand).
    Run(run::Args),
    /// Resume a checkpoint, rule-only state export, or committed SQLite run.
    Resume {
        input: PathBuf,
        #[command(flatten)]
        args: run::Args,
    },
    /// Start an independent run at a checkpoint with --scenario containing only future interventions.
    Branch {
        input: PathBuf,
        #[command(flatten)]
        args: run::Args,
    },
    /// Verify JSONL or SQLite history without model calls or credentials.
    Replay(persistence::ReplayArgs),
    /// Query an agent's historical events from SQLite.
    Inspect(persistence::InspectArgs),
    /// Compare a matrix of seeds, populations and policies, saving every run's configuration.
    Experiment(experiment::Args),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let filter = match std::env::var("RUST_LOG") {
        Ok(value) => {
            tracing_subscriber::EnvFilter::try_new(value).context("invalid RUST_LOG filter")?
        }
        Err(_) => tracing_subscriber::EnvFilter::new("warn"),
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    match cli.command {
        None => run::execute(cli.args, None, false),
        Some(Command::Run(args)) => run::execute(args, None, false),
        Some(Command::Resume { input, args }) => run::execute(args, Some(input), false),
        Some(Command::Branch { input, args }) => run::execute(args, Some(input), true),
        Some(Command::Replay(args)) => persistence::replay(args),
        Some(Command::Inspect(args)) => persistence::inspect(args),
        Some(Command::Experiment(args)) => experiment::execute(args),
    }
}
