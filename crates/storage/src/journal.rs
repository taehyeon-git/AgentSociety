use crate::checksum;
use agentsociety_core::*;
use agentsociety_simulation::{
    PolicyCheckpoint, RunCheckpoint, Simulation, SimulationSnapshot, TickReport,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

/// Versioned description of one recording segment, including resumed segments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalHeader {
    /// Serialization format used for all records.
    pub format_version: u32,
    /// Exact simulation rules version.
    pub engine_version: String,
    /// Additional ticks requested from this initial state.
    pub requested_ticks: Tick,
    /// Settled engine state before the segment.
    pub initial: SimulationSnapshot,
    /// Request budgets and cooldowns before the segment.
    pub policy: PolicyCheckpoint,
    /// Number of original tick-zero facts following the header; zero on resume.
    pub initial_events_count: usize,
}

impl JournalHeader {
    /// Describe a fresh or restored run without contacting a provider.
    pub fn new(
        simulation: &Simulation,
        policy: PolicyCheckpoint,
        requested_ticks: Tick,
    ) -> Result<Self> {
        policy.validate(simulation)?;
        simulation
            .world()
            .tick()
            .checked_add(requested_ticks)
            .context("requested tick range overflows")?;
        Ok(Self {
            format_version: FORMAT_VERSION,
            engine_version: ENGINE_VERSION.into(),
            requested_ticks,
            initial: simulation.snapshot()?,
            policy,
            initial_events_count: simulation.initial_events().len(),
        })
    }

    /// Restore and check framing metadata without reading credentials.
    pub fn restore(&self) -> Result<Simulation> {
        ensure!(
            self.format_version == FORMAT_VERSION && self.engine_version == ENGINE_VERSION,
            "unsupported journal format or engine version"
        );
        self.initial
            .world
            .tick
            .checked_add(self.requested_ticks)
            .context("journal tick range overflows")?;
        let simulation = Simulation::from_snapshot(self.initial.clone())?;
        self.policy.validate(&simulation)?;
        ensure!(
            self.initial_events_count == 0
                || (self.initial.world.tick == 0
                    && self.initial_events_count == self.initial.config.agents),
            "invalid initialization event count"
        );
        Ok(simulation)
    }

    pub(crate) fn checkpoint(&self) -> RunCheckpoint {
        RunCheckpoint {
            checkpoint_version: 1,
            simulation: self.initial.clone(),
            policy: self.policy.clone(),
        }
    }
}

/// Persisted framing and facts. Tick markers commit policy state only after all events.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "record_type", rename_all = "snake_case")]
pub enum JournalRecord {
    /// First record, carrying the exact initial conditions.
    RunStarted {
        /// Header fields remain at the top level for simple log inspection.
        #[serde(flatten)]
        header: Box<JournalHeader>,
    },
    /// One immutable engine fact.
    Event {
        /// Global sequence, tick and typed payload.
        #[serde(flatten)]
        record: EventRecord,
    },
    /// A completely recorded tick and its corresponding application policy state.
    TickCommitted {
        /// Completed tick.
        tick: Tick,
        /// Expected number of facts in this tick.
        event_count: usize,
        /// Request accounting after this tick.
        policy: PolicyCheckpoint,
    },
    /// Required completion marker; a log without it is explicitly incomplete.
    RunCompleted {
        /// Full state expected after replay.
        checkpoint: Box<RunCheckpoint>,
        /// SHA-256 of the compact checkpoint JSON.
        checksum: String,
    },
}

/// Streaming journal writer, retaining only counters and policy state between ticks.
pub struct JournalWriter<W: Write> {
    writer: W,
    tick: Tick,
    target_tick: Tick,
    next_sequence: u64,
    policy: PolicyCheckpoint,
    finished: bool,
}

impl<W: Write> JournalWriter<W> {
    /// Write a header and any original initialization events.
    pub fn new(
        writer: W,
        simulation: &Simulation,
        policy: PolicyCheckpoint,
        ticks: Tick,
    ) -> Result<Self> {
        let header = JournalHeader::new(simulation, policy.clone(), ticks)?;
        let mut journal = Self {
            writer,
            tick: simulation.world().tick(),
            target_tick: simulation.world().tick() + ticks,
            next_sequence: simulation.next_event_sequence(),
            policy,
            finished: false,
        };
        journal.line(&JournalRecord::RunStarted {
            header: Box::new(header),
        })?;
        for record in simulation.initial_events() {
            journal.line(&JournalRecord::Event {
                record: record.clone(),
            })?;
        }
        journal.writer.flush()?;
        Ok(journal)
    }

    fn line(&mut self, record: &JournalRecord) -> Result<()> {
        serde_json::to_writer(&mut self.writer, record)?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }

    /// Record a successful engine tick followed by the corresponding policy commit marker.
    pub fn append(
        &mut self,
        simulation: &Simulation,
        report: &TickReport,
        policy: PolicyCheckpoint,
    ) -> Result<()> {
        ensure!(
            !self.finished
                && report.stats.tick == self.tick + 1
                && report.stats.tick <= self.target_tick,
            "journal tick order mismatch"
        );
        ensure!(
            report.stats == *simulation.stats(),
            "journal report differs from simulation"
        );
        ensure!(
            report
                .events
                .first()
                .is_some_and(|record| record.sequence == self.next_sequence),
            "journal event sequence mismatch"
        );
        policy.validate_transition(&self.policy, simulation, &report.events)?;
        for record in &report.events {
            self.line(&JournalRecord::Event {
                record: record.clone(),
            })?;
        }
        self.line(&JournalRecord::TickCommitted {
            tick: report.stats.tick,
            event_count: report.events.len(),
            policy: policy.clone(),
        })?;
        self.writer.flush()?;
        self.tick = report.stats.tick;
        self.next_sequence = simulation.next_event_sequence();
        self.policy = policy;
        Ok(())
    }

    /// Write the final comparison checkpoint; premature completion is rejected.
    pub fn finish(&mut self, checkpoint: &RunCheckpoint) -> Result<()> {
        ensure!(
            !self.finished
                && self.tick == self.target_tick
                && checkpoint.simulation.world.tick == self.tick
                && checkpoint.simulation.next_event_sequence == self.next_sequence
                && checkpoint.policy == self.policy,
            "cannot finish journal with a different or incomplete run"
        );
        self.line(&JournalRecord::RunCompleted {
            checkpoint: Box::new(checkpoint.clone()),
            checksum: checksum(checkpoint)?,
        })?;
        self.writer.flush()?;
        self.finished = true;
        Ok(())
    }

    /// Recover the underlying output after a successful finish.
    pub fn into_inner(self) -> W {
        self.writer
    }
}

/// Verified replay result, with no performance values mixed into deterministic state.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayResult {
    /// Restorable final state and recorded request budgets.
    pub checkpoint: RunCheckpoint,
    /// Ticks actually re-executed during verification.
    pub replayed_ticks: Tick,
    /// Facts validated in this recording segment, including initialization facts.
    pub verified_events: u64,
}

fn read_record(reader: &mut impl BufRead, limit: usize) -> Result<Option<JournalRecord>> {
    let mut bytes = Vec::new();
    let count = std::io::Read::take(reader, limit as u64 + 1).read_until(b'\n', &mut bytes)?;
    if count == 0 {
        return Ok(None);
    }
    ensure!(count <= limit, "journal record exceeds size limit");
    ensure!(bytes.last() == Some(&b'\n'), "truncated journal line");
    Ok(Some(
        decode_record(&bytes).context("invalid journal record")?,
    ))
}

// Decode the discriminant, then deserialize the original bytes directly into a
// typed payload. Serde's buffered internally tagged/flattened content does not
// support u128, including RNG positions and cumulative resource flows.
fn decode_record(bytes: &[u8]) -> Result<JournalRecord> {
    #[derive(Deserialize)]
    struct Tag {
        record_type: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Start {
        #[serde(rename = "record_type")]
        _kind: String,
        format_version: u32,
        engine_version: String,
        requested_ticks: Tick,
        initial: SimulationSnapshot,
        policy: PolicyCheckpoint,
        initial_events_count: usize,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Fact {
        #[serde(rename = "record_type")]
        _kind: String,
        sequence: u64,
        tick: Tick,
        event: Event,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Commit {
        #[serde(rename = "record_type")]
        _kind: String,
        tick: Tick,
        event_count: usize,
        policy: PolicyCheckpoint,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct End {
        #[serde(rename = "record_type")]
        _kind: String,
        checkpoint: RunCheckpoint,
        checksum: String,
    }
    match serde_json::from_slice::<Tag>(bytes)?.record_type.as_str() {
        "run_started" => {
            let start: Start = serde_json::from_slice(bytes)?;
            Ok(JournalRecord::RunStarted {
                header: Box::new(JournalHeader {
                    format_version: start.format_version,
                    engine_version: start.engine_version,
                    requested_ticks: start.requested_ticks,
                    initial: start.initial,
                    policy: start.policy,
                    initial_events_count: start.initial_events_count,
                }),
            })
        }
        "event" => {
            let fact: Fact = serde_json::from_slice(bytes)?;
            Ok(JournalRecord::Event {
                record: EventRecord {
                    sequence: fact.sequence,
                    tick: fact.tick,
                    event: fact.event,
                },
            })
        }
        "tick_committed" => {
            let commit: Commit = serde_json::from_slice(bytes)?;
            Ok(JournalRecord::TickCommitted {
                tick: commit.tick,
                event_count: commit.event_count,
                policy: commit.policy,
            })
        }
        "run_completed" => {
            let end: End = serde_json::from_slice(bytes)?;
            Ok(JournalRecord::RunCompleted {
                checkpoint: Box::new(end.checkpoint),
                checksum: end.checksum,
            })
        }
        _ => bail!("unknown journal record type"),
    }
}

/// Replay a complete JSONL stream using recorded decisions and compare every event.
/// Missing footers, partial ticks, changed accounting and trailing records are errors.
pub fn replay_journal(mut reader: impl BufRead) -> Result<ReplayResult> {
    let Some(JournalRecord::RunStarted { header }) =
        read_record(&mut reader, crate::MAX_DOCUMENT_BYTES)?
    else {
        bail!("journal must begin with a run_started header");
    };
    let mut simulation = header.restore()?;
    let mut policy = header.policy.clone();
    let initialization = if header.initial_events_count > 0 {
        Simulation::new(header.initial.config.clone())?
            .initial_events()
            .to_vec()
    } else {
        Vec::new()
    };
    for expected in &initialization {
        ensure!(
            matches!(read_record(&mut reader, 1024 * 1024)?, Some(JournalRecord::Event { record }) if record == *expected),
            "initialization event mismatch"
        );
    }
    let mut verified_events = initialization.len() as u64;
    let mut ticks = 0;
    let mut pending = Vec::new();
    let max_tick_events = header
        .initial
        .config
        .agents
        .saturating_mul(MAX_EVENTS_PER_AGENT)
        .saturating_add(1024);
    loop {
        // The final checkpoint can be larger than a single event; all reads remain bounded.
        let record = read_record(&mut reader, crate::MAX_DOCUMENT_BYTES)?
            .context("incomplete journal: missing run_completed footer")?;
        match record {
            JournalRecord::Event { record } => {
                ensure!(
                    pending.len() < max_tick_events,
                    "too many events in a single tick"
                );
                pending.push(record);
            }
            JournalRecord::TickCommitted {
                tick,
                event_count,
                policy: next_policy,
            } => {
                ensure!(
                    ticks < header.requested_ticks
                        && tick == simulation.world().tick() + 1
                        && event_count == pending.len(),
                    "journal tick commit mismatch"
                );
                simulation
                    .replay_tick(&pending)
                    .with_context(|| format!("replay tick {tick}"))?;
                next_policy.validate_transition(&policy, &simulation, &pending)?;
                policy = next_policy;
                verified_events = verified_events
                    .checked_add(pending.len() as u64)
                    .context("event count overflow")?;
                pending.clear();
                ticks += 1;
            }
            JournalRecord::RunCompleted {
                checkpoint,
                checksum: expected,
            } => {
                ensure!(
                    pending.is_empty() && ticks == header.requested_ticks,
                    "incomplete journal: final tick count mismatch"
                );
                let actual = RunCheckpoint {
                    checkpoint_version: 1,
                    simulation: simulation.snapshot()?,
                    policy,
                };
                ensure!(
                    actual == *checkpoint && checksum(&actual)? == expected,
                    "replay final checkpoint mismatch"
                );
                ensure!(
                    read_record(&mut reader, crate::MAX_DOCUMENT_BYTES)?.is_none(),
                    "unexpected data after journal footer"
                );
                return Ok(ReplayResult {
                    checkpoint: actual,
                    replayed_ticks: ticks,
                    verified_events,
                });
            }
            JournalRecord::RunStarted { .. } => bail!("duplicate journal header"),
        }
    }
}
