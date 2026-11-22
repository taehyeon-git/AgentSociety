//! UI commands cross a channel; only this worker owns mutable engine state.

use agentsociety_analytics::EconomyStats;
use agentsociety_core::{EventRecord, LlmUsage, SimulationConfig};
use agentsociety_simulation::{PolicyRuntime, RunCheckpoint, Simulation, SimulationSnapshot};
use agentsociety_storage::{read_run, read_scenario, write_checkpoint};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_HISTORY: usize = 600;
const MAX_EVENTS: usize = 1500;

/// Commands applied exclusively at completed tick boundaries.
pub enum Command {
    /// Start or pause continuous execution.
    Running(bool),
    /// Advance precisely one tick while paused.
    Step,
    /// Replace the run with fresh seeded initial conditions.
    Restart(SimulationConfig),
    /// Save a completed checkpoint, including external-policy budgets, and pause.
    Save {
        /// User-selected destination.
        path: std::path::PathBuf,
        /// Explicit replacement chosen in the file picker.
        overwrite: bool,
    },
    /// Validate and load a saved run while preserving the source file.
    Load(std::path::PathBuf),
    /// Create an independent continuation with a future intervention schedule.
    Branch(std::path::PathBuf),
    /// Set target ticks per wall-clock second (1 through 240).
    Speed(u32),
    /// Stop the worker and release resources.
    Shutdown,
}

/// Immutable published state; the UI never blocks on an engine tick.
pub struct View {
    /// Latest fully settled state.
    pub snapshot: SimulationSnapshot,
    /// Bounded recent economic samples.
    pub history: VecDeque<EconomyStats>,
    /// Bounded recent structured events, in sequence order.
    pub events: VecDeque<EventRecord>,
    /// Whether the worker is executing ticks continuously.
    pub running: bool,
    /// Requested tick rate.
    pub speed: u32,
    /// Last completed engine tick duration in milliseconds.
    pub tick_ms: f64,
    /// Explicit error/status message; errors pause the worker.
    pub status: String,
    /// Restart generation; lets the UI discard selection and plot history.
    pub generation: u64,
    /// Bounded request accounting for the current run.
    pub llm_usage: LlmUsage,
}

/// Handle to a worker; dropping it shuts down and joins the thread.
pub struct Worker {
    sender: Sender<Command>,
    latest: Arc<Mutex<Arc<View>>>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Validate and initialize before spawning, so startup failures are visible.
    pub fn new(config: SimulationConfig) -> anyhow::Result<Self> {
        Self::with_policy(config, PolicyRuntime::default())
    }

    /// Start a worker with an explicitly configured optional external policy runtime.
    pub fn with_policy(config: SimulationConfig, policy: PolicyRuntime) -> anyhow::Result<Self> {
        policy.validate(&config)?;
        let simulation = Simulation::new(config)?;
        let initial = View {
            snapshot: simulation.snapshot()?,
            history: VecDeque::from([simulation.stats().clone()]),
            events: initial_events(&simulation),
            running: false,
            speed: 30,
            tick_ms: 0.0,
            status: ready_status(&simulation),
            generation: 0,
            llm_usage: policy.usage(),
        };
        let latest = Arc::new(Mutex::new(Arc::new(initial)));
        let (sender, receiver) = mpsc::channel();
        let published = Arc::clone(&latest);
        let thread = thread::Builder::new()
            .name("agentsociety-engine".into())
            .spawn(move || run(simulation, receiver, published, policy))?;
        Ok(Self {
            sender,
            latest,
            thread: Some(thread),
        })
    }

    /// Send a nonblocking command. Channel failures are not silently ignored.
    pub fn send(&self, command: Command) -> anyhow::Result<()> {
        self.sender
            .send(command)
            .map_err(|_| anyhow::anyhow!("simulation worker stopped"))
    }

    /// Take a cheap shared reference to the latest published frame.
    pub fn view(&self) -> Arc<View> {
        Arc::clone(
            &self
                .latest
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn initial_events(simulation: &Simulation) -> VecDeque<EventRecord> {
    simulation
        .initial_events()
        .iter()
        .skip(simulation.initial_events().len().saturating_sub(MAX_EVENTS))
        .cloned()
        .collect()
}

fn ready_status(simulation: &Simulation) -> String {
    format!("Ready. {:?} policy.", simulation.config().policy)
}

fn run(
    mut simulation: Simulation,
    receiver: Receiver<Command>,
    published: Arc<Mutex<Arc<View>>>,
    mut policy: PolicyRuntime,
) {
    let mut history = VecDeque::from([simulation.stats().clone()]);
    let mut events = initial_events(&simulation);
    let mut running = false;
    let mut speed = 30;
    let mut tick_ms = 0.0;
    let mut status = ready_status(&simulation);
    let mut generation = 0;
    let mut next_tick = Instant::now();
    let mut last_publish = Instant::now();
    let mut engine_failed = false;
    loop {
        let wait = if running {
            next_tick
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(20))
        } else {
            Duration::from_millis(50)
        };
        let command = match receiver.recv_timeout(wait) {
            Ok(command) => Some(command),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let command_received = command.is_some();
        let mut single_step = false;
        match command {
            Some(Command::Shutdown) => break,
            Some(Command::Running(value)) => {
                running = value && !engine_failed;
                next_tick = Instant::now();
                if !engine_failed {
                    status = if running { "Running." } else { "Paused." }.into();
                }
            }
            Some(Command::Step) => {
                running = false;
                single_step = !engine_failed;
            }
            Some(Command::Speed(value)) => speed = value.clamp(1, 240),
            Some(Command::Save { path, overwrite }) => {
                running = false;
                status = match RunCheckpoint::capture(&simulation, &policy)
                    .map_err(anyhow::Error::from)
                    .and_then(|checkpoint| write_checkpoint(&path, &checkpoint, overwrite))
                {
                    Ok(()) => format!(
                        "Saved tick {} to {}",
                        simulation.world().tick(),
                        path.display()
                    ),
                    Err(error) => format!("Save failed: {error:#}"),
                };
            }
            Some(Command::Load(path)) => {
                running = false;
                match read_run(&path).and_then(|checkpoint| Ok(checkpoint.restore()?)) {
                    Ok((new, restored_policy)) => {
                        simulation = new;
                        policy = restored_policy;
                        engine_failed = false;
                        generation += 1;
                        tick_ms = 0.0;
                        history = VecDeque::from([simulation.stats().clone()]);
                        events = initial_events(&simulation);
                        status = format!(
                            "Loaded tick {} from {}",
                            simulation.world().tick(),
                            path.display()
                        );
                    }
                    Err(error) => status = format!("Load failed: {error:#}"),
                }
            }
            Some(Command::Branch(path)) => {
                running = false;
                match read_scenario(&path)
                    .and_then(|scenario| Ok(simulation.branch_scenario(scenario)?))
                {
                    Ok(new) => {
                        simulation = new;
                        engine_failed = false;
                        generation += 1;
                        tick_ms = 0.0;
                        history = VecDeque::from([simulation.stats().clone()]);
                        events.clear();
                        status = format!(
                            "Branched at tick {} into {}. Save to keep this branch.",
                            simulation.world().tick(),
                            simulation.config().scenario.name
                        );
                    }
                    Err(error) => status = format!("Branch failed: {error:#}"),
                }
            }
            Some(Command::Restart(config)) => match policy
                .validate(&config)
                .and_then(|()| Simulation::new(config))
            {
                Ok(new) => {
                    simulation = new;
                    policy.reset();
                    running = false;
                    engine_failed = false;
                    generation += 1;
                    tick_ms = 0.0;
                    history = VecDeque::from([simulation.stats().clone()]);
                    events = initial_events(&simulation);
                    status = "Restarted with the selected seed and population.".into();
                }
                Err(error) => {
                    running = false;
                    status = format!("Restart failed: {error}");
                }
            },
            None => {}
        }
        let stepped = single_step || (running && Instant::now() >= next_tick);
        let mut force_publish = command_received;
        if stepped {
            let tick_started = Instant::now();
            match policy.step(&mut simulation) {
                Ok(report) => {
                    tick_ms = report.timing.total.as_secs_f64() * 1000.0;
                    history.push_back(report.stats);
                    events.extend(report.events);
                    while history.len() > MAX_HISTORY {
                        history.pop_front();
                    }
                    while events.len() > MAX_EVENTS {
                        events.pop_front();
                    }
                    status = if running {
                        "Running."
                    } else {
                        "Paused after one tick."
                    }
                    .into();
                }
                Err(error) => {
                    running = false;
                    engine_failed = true;
                    status = format!("Engine stopped: {error}");
                    force_publish = true;
                }
            }
            next_tick = tick_started + Duration::from_secs_f64(1.0 / f64::from(speed));
        }
        if force_publish || (stepped && last_publish.elapsed() >= Duration::from_millis(33)) {
            match simulation.snapshot() {
                Ok(snapshot) => {
                    *published.lock().unwrap_or_else(|p| p.into_inner()) = Arc::new(View {
                        snapshot,
                        history: history.clone(),
                        events: events.clone(),
                        running,
                        speed,
                        tick_ms,
                        status: status.clone(),
                        generation,
                        llm_usage: policy.usage(),
                    });
                }
                Err(error) => {
                    // Publish the last valid state alongside the error, never a partial tick.
                    running = false;
                    if !engine_failed {
                        status = format!("Cannot publish failed tick: {error}");
                        engine_failed = true;
                    }
                    let mut guard = published.lock().unwrap_or_else(|p| p.into_inner());
                    *guard = Arc::new(View {
                        snapshot: guard.snapshot.clone(),
                        history: history.clone(),
                        events: events.clone(),
                        running: false,
                        speed,
                        tick_ms,
                        status: status.clone(),
                        generation,
                        llm_usage: policy.usage(),
                    });
                }
            }
            last_publish = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn until(worker: &Worker, condition: impl Fn(&View) -> bool) -> Arc<View> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let view = worker.view();
            if condition(&view) {
                return view;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not respond: {}",
                view.status
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn step_pause_restart_and_speed_cross_the_worker_boundary() {
        let config = SimulationConfig {
            agents: 10,
            ..Default::default()
        };
        let worker = Worker::new(config.clone()).unwrap();
        assert_eq!(worker.view().snapshot.world.tick, 0);
        worker.send(Command::Step).unwrap();
        let first = until(&worker, |v| v.snapshot.world.tick == 1);
        assert!(!first.running);
        let mut expected = Simulation::new(config.clone()).unwrap();
        expected.step().unwrap();
        assert_eq!(first.snapshot, expected.snapshot().unwrap());
        worker.send(Command::Speed(240)).unwrap();
        worker.send(Command::Running(true)).unwrap();
        until(&worker, |v| v.snapshot.world.tick >= 4);
        worker.send(Command::Running(false)).unwrap();
        let paused = until(&worker, |v| !v.running);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(
            worker.view().snapshot.world.tick,
            paused.snapshot.world.tick
        );
        worker.send(Command::Restart(config)).unwrap();
        let reset = until(&worker, |v| v.generation == 1);
        assert_eq!(reset.snapshot.world.tick, 0);
        assert_eq!(reset.history.len(), 1);
        assert_eq!(reset.snapshot.world.agents.len(), 10);
    }

    #[test]
    fn invalid_restart_preserves_completed_state_and_reports_error() {
        let worker = Worker::new(SimulationConfig {
            agents: 3,
            ..Default::default()
        })
        .unwrap();
        worker.send(Command::Step).unwrap();
        let before = until(&worker, |v| v.snapshot.world.tick == 1);
        worker
            .send(Command::Restart(SimulationConfig {
                map_width: 0,
                ..Default::default()
            }))
            .unwrap();
        let failed = until(&worker, |v| v.status.starts_with("Restart failed:"));
        assert_eq!(failed.snapshot, before.snapshot);
        assert!(!failed.running);
        worker.send(Command::Step).unwrap();
        until(&worker, |v| v.snapshot.world.tick == 2);
    }

    #[test]
    fn initial_timeline_is_bounded_for_large_populations() {
        let worker = Worker::new(SimulationConfig {
            agents: MAX_EVENTS + 1,
            ..Default::default()
        })
        .unwrap();
        let view = worker.view();
        assert_eq!(view.snapshot.world.agents.len(), MAX_EVENTS + 1);
        assert_eq!(view.events.len(), MAX_EVENTS);
        assert_eq!(view.events.front().unwrap().sequence, 1);
    }
}
