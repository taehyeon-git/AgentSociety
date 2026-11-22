//! Heap allocation tracing in a separate binary; production allocators are unchanged.
use agentsociety_core::{ENGINE_VERSION, SimulationConfig};
use agentsociety_simulation::Simulation;
use anyhow::{Context, Result, ensure};
use clap::Parser;
use std::{fs, path::PathBuf, time::Instant};

#[global_allocator]
static ALLOCATOR: dhat::Alloc = dhat::Alloc;

#[derive(Parser)]
#[command(about = "Trace rule-based engine heap allocations; use --profile profiling")]
struct Args {
    #[arg(long, default_value_t = 100)]
    agents: usize,
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u64).range(0..=1_000_000))]
    ticks: u64,
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// New DHAT trace file. Existing files are never replaced.
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(u8).range(4..=64))]
    frames: u8,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = SimulationConfig {
        agents: args.agents,
        seed: args.seed,
        ..Default::default()
    };
    config.validate()?;
    ensure!(!args.out.exists(), "profile output already exists");
    let parent = args
        .out
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?.into_temp_path();

    // Include initialization, all engine stages and world teardown. Argument
    // parsing, trace serialization and result reporting are outside the sample.
    let profiler = dhat::Profiler::builder()
        .file_name(&temporary)
        .trim_backtraces(Some(usize::from(args.frames)))
        .build();
    let started = Instant::now();
    let mut simulation = Simulation::new(config)?;
    for _ in 0..args.ticks {
        simulation.step()?;
    }
    let stats = simulation.stats().clone();
    let events = simulation.next_event_sequence();
    let retained = dhat::HeapStats::get();
    drop(simulation);
    let final_heap = dhat::HeapStats::get();
    let profiled_elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    drop(profiler);
    ensure!(
        fs::metadata(&temporary)?.len() > 0,
        "profiler did not write a trace"
    );
    temporary
        .persist_noclobber(&args.out)
        .context("save allocation profile without replacing existing files")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "engine_version": ENGINE_VERSION,
            "agents": args.agents,
            "ticks": args.ticks,
            "seed": args.seed,
            "trace_file": args.out,
            "profiled_elapsed_ms": profiled_elapsed_ms,
            "total_allocations": final_heap.total_blocks,
            "total_allocated_bytes": final_heap.total_bytes,
            "peak_live_allocations": final_heap.max_blocks,
            "peak_live_bytes": final_heap.max_bytes,
            "retained_world_allocations": retained.curr_blocks,
            "retained_world_bytes": retained.curr_bytes,
            "after_world_drop_allocations": final_heap.curr_blocks,
            "after_world_drop_bytes": final_heap.curr_bytes,
            "total_events": events,
            "stats": stats,
        }))?
    );
    Ok(())
}
