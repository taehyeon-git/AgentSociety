//! AgentSociety native desktop entry point.

use agentsociety_core::{PolicyKind, SimulationConfig};
use agentsociety_desktop::Dashboard;
use agentsociety_simulation::PolicyRuntime;
use clap::Parser;

#[derive(Parser)]
#[command(version, about = "AgentSociety live social/economic dashboard")]
struct Args {
    /// Population; overrides the config file (default: 100).
    #[arg(long)]
    agents: Option<usize>,
    /// Random seed; overrides the config file (default: 42).
    #[arg(long)]
    seed: Option<u64>,
    /// Decision policy: rule-based, llm, or hybrid.
    #[arg(long)]
    policy: Option<PolicyKind>,
    /// Maximum agents assigned the external policy.
    #[arg(long)]
    llm_agents: Option<usize>,
    /// Provider settings JSON; requires --features llm.
    #[arg(long)]
    llm_config: Option<std::path::PathBuf>,
    #[arg(long)]
    config: Option<std::path::PathBuf>,
    #[arg(long)]
    auto_start: bool,
    /// Close after a completed tick; useful for native-renderer smoke tests.
    #[arg(long)]
    exit_after_ticks: Option<u64>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    let mut config = if let Some(path) = args.config {
        serde_json::from_reader(std::fs::File::open(path)?)?
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
    let policy = if let Some(path) = args.llm_config {
        #[cfg(feature = "llm")]
        {
            let settings = serde_json::from_reader(std::fs::File::open(path)?)?;
            PolicyRuntime::with_llm(settings)?
        }
        #[cfg(not(feature = "llm"))]
        {
            let _ = path;
            anyhow::bail!("--llm-config requires rebuilding with --features llm")
        }
    } else {
        PolicyRuntime::default()
    };
    let dashboard = Dashboard::with_policy(config, args.auto_start, args.exit_after_ticks, policy)?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 950.0])
            .with_min_inner_size([900.0, 650.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "AgentSociety",
        options,
        Box::new(|context| {
            context.egui_ctx.set_visuals(eframe::egui::Visuals::dark());
            Ok(Box::new(dashboard))
        }),
    )
    .map_err(|error| anyhow::anyhow!("desktop initialization failed: {error}"))
}
