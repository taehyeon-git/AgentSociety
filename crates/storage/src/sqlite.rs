//! Transactional SQLite persistence; implemented independently of the GUI.
use crate::{JournalHeader, ReplayResult, checksum};
use agentsociety_core::*;
use agentsociety_simulation::{PolicyCheckpoint, RunCheckpoint, Simulation, TickReport};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{fs::OpenOptions, path::Path, time::Duration};

const APPLICATION_ID: i64 = 0x41534734;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    tick: Tick,
    next_sequence: u64,
    chain_hash: String,
}

/// Bounded indexed lookup of an agent's past facts, including both sides of trades.
#[derive(Debug, Clone)]
pub struct EventQuery {
    /// Optional participant identity.
    pub agent: Option<AgentId>,
    /// Inclusive first tick.
    pub from_tick: Tick,
    /// Inclusive last tick.
    pub to_tick: Tick,
    /// Maximum returned records, from 1 to 10,000.
    pub limit: usize,
}

impl Default for EventQuery {
    fn default() -> Self {
        Self {
            agent: None,
            from_tick: 0,
            to_tick: i64::MAX as u64,
            limit: 100,
        }
    }
}

/// One simulation run per database. Every tick commits events, policy accounting and
/// the new sequence together; optional checkpoints participate in the same transaction.
pub struct RunStore {
    connection: Connection,
    header: JournalHeader,
    position: Position,
    policy: PolicyCheckpoint,
    target_tick: Tick,
    checkpoint_every: Tick,
    writable: bool,
}

impl RunStore {
    /// Create a new database. Existing files are never truncated, even on CLI overwrite.
    pub fn create(
        path: &Path,
        simulation: &Simulation,
        policy: PolicyCheckpoint,
        ticks: Tick,
        checkpoint_every: Tick,
    ) -> Result<Self> {
        ensure!(
            (1..=1_000_000).contains(&checkpoint_every),
            "checkpoint interval must be 1..=1000000"
        );
        let header = JournalHeader::new(simulation, policy.clone(), ticks)?;
        let target_tick = simulation
            .world()
            .tick()
            .checked_add(ticks)
            .context("tick range overflow")?;
        sql_integer(target_tick)?;
        sql_integer(simulation.next_event_sequence())?;
        if let Some(parent) = std::path::absolute(path)?.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Reserve this specific path before SQLite opens it, preventing accidental clobber.
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| {
                format!(
                    "create database {}; use resume for an existing run",
                    path.display()
                )
            })?;
        let mut connection = Connection::open(path)?;
        configure(&connection)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
            CREATE TABLE events (sequence INTEGER PRIMARY KEY CHECK(sequence >= 0), tick INTEGER NOT NULL CHECK(tick >= 0), body TEXT NOT NULL) STRICT;
            CREATE INDEX events_tick ON events(tick, sequence);
            CREATE TABLE event_agents (sequence INTEGER NOT NULL REFERENCES events(sequence), agent_id INTEGER NOT NULL,
                PRIMARY KEY(agent_id, sequence)) STRICT;
            CREATE TABLE tick_commits (tick INTEGER PRIMARY KEY, event_count INTEGER NOT NULL CHECK(event_count > 0),
                last_sequence INTEGER NOT NULL, policy TEXT NOT NULL, chain_hash TEXT NOT NULL) STRICT;
            CREATE TABLE checkpoints (tick INTEGER PRIMARY KEY, body TEXT NOT NULL, checksum TEXT NOT NULL, chain_hash TEXT NOT NULL) STRICT;")?;
        connection.pragma_update(None, "application_id", APPLICATION_ID)?;
        connection.pragma_update(None, "user_version", 1)?;
        let position = Position {
            tick: simulation.world().tick(),
            next_sequence: simulation.next_event_sequence(),
            chain_hash: checksum(&header)?,
        };
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        put_meta(&tx, "header", &header)?;
        put_meta(&tx, "header_checksum", &checksum(&header)?)?;
        put_meta(&tx, "position", &position)?;
        put_meta(&tx, "target_tick", &target_tick)?;
        put_meta(&tx, "checkpoint_every", &checkpoint_every)?;
        insert_events(&tx, simulation.initial_events())?;
        save_checkpoint(&tx, &header.checkpoint(), &position.chain_hash)?;
        tx.commit()?;
        Ok(Self {
            connection,
            header,
            position,
            policy,
            target_tick,
            checkpoint_every,
            writable: true,
        })
    }

    /// Open an existing database without CREATE. Replay and inspection use read-only mode.
    pub fn open(path: &Path, writable: bool) -> Result<Self> {
        let flags = if writable {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let connection = Connection::open_with_flags(path, flags)
            .with_context(|| format!("open database {}", path.display()))?;
        configure(&connection)?;
        let application: i64 =
            connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            application == APPLICATION_ID && version == 1,
            "not a supported AgentSociety database"
        );
        let check: String = connection.pragma_query_value(None, "quick_check", |row| row.get(0))?;
        ensure!(check == "ok", "SQLite integrity check failed: {check}");
        let header: JournalHeader = get_meta(&connection, "header")?;
        ensure!(
            get_meta::<String>(&connection, "header_checksum")? == checksum(&header)?,
            "database header checksum mismatch"
        );
        header.restore()?;
        let position: Position = get_meta(&connection, "position")?;
        let target_tick = get_meta(&connection, "target_tick")?;
        let checkpoint_every = get_meta(&connection, "checkpoint_every")?;
        ensure!(
            position.tick >= header.initial.world.tick
                && target_tick >= position.tick
                && (1..=1_000_000).contains(&checkpoint_every),
            "invalid database run boundaries"
        );
        sql_integer(target_tick)?;
        let policy = if position.tick == header.initial.world.tick {
            header.policy.clone()
        } else {
            parse_json(&connection.query_row(
                "SELECT policy FROM tick_commits WHERE tick=?1",
                [sql_integer(position.tick)?],
                |row| row.get::<_, String>(0),
            )?)?
        };
        Ok(Self {
            connection,
            header,
            position,
            policy,
            target_tick,
            checkpoint_every,
            writable,
        })
    }

    /// Additional target ticks for an explicitly resumed run, preserving all past rows.
    pub fn extend_run(&mut self, ticks: Tick) -> Result<()> {
        ensure!(self.writable, "database is read-only");
        let target = self
            .position
            .tick
            .checked_add(ticks)
            .context("resumed tick range overflows")?;
        sql_integer(target)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(
            get_meta::<Position>(&tx, "position")? == self.position,
            "database changed in another writer"
        );
        put_meta(&tx, "target_tick", &target)?;
        tx.commit()?;
        self.target_tick = target;
        Ok(())
    }

    /// Commit a successful tick atomically; any SQLite failure leaves the prior tick durable.
    pub fn append(
        &mut self,
        simulation: &Simulation,
        report: &TickReport,
        policy: PolicyCheckpoint,
    ) -> Result<()> {
        ensure!(
            self.writable
                && simulation.world().tick() == self.position.tick + 1
                && simulation.world().tick() <= self.target_tick,
            "database tick order mismatch"
        );
        ensure!(
            report.stats == *simulation.stats(),
            "database report differs from simulation"
        );
        for (index, record) in report.events.iter().enumerate() {
            ensure!(
                record.tick == simulation.world().tick()
                    && self.position.next_sequence.checked_add(index as u64)
                        == Some(record.sequence),
                "database event sequence mismatch"
            );
        }
        ensure!(
            matches!(
                report.events.last().map(|r| &r.event),
                Some(Event::TickCompleted { .. })
            ),
            "database requires a completed tick"
        );
        policy.validate_transition(&self.policy, simulation, &report.events)?;
        let bodies = report
            .events
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let policy_json = serde_json::to_string(&policy)?;
        let chain_hash = tick_hash(
            &self.position.chain_hash,
            report.stats.tick,
            &bodies,
            &policy_json,
        );
        let position = Position {
            tick: report.stats.tick,
            next_sequence: simulation.next_event_sequence(),
            chain_hash,
        };
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(
            get_meta::<Position>(&tx, "position")? == self.position,
            "database changed in another writer"
        );
        insert_event_bodies(&tx, &report.events, &bodies)?;
        tx.execute("INSERT INTO tick_commits(tick,event_count,last_sequence,policy,chain_hash) VALUES(?1,?2,?3,?4,?5)",
            params![sql_integer(position.tick)?, report.events.len() as i64, sql_integer(position.next_sequence - 1)?, policy_json, position.chain_hash])?;
        if position.tick.is_multiple_of(self.checkpoint_every) {
            save_checkpoint(
                &tx,
                &RunCheckpoint {
                    checkpoint_version: 1,
                    simulation: simulation.snapshot()?,
                    policy: policy.clone(),
                },
                &position.chain_hash,
            )?;
        }
        put_meta(&tx, "position", &position)?;
        tx.commit()?;
        self.position = position;
        self.policy = policy;
        Ok(())
    }

    /// Persist the final checkpoint only at the declared target boundary.
    pub fn finish(&mut self, checkpoint: &RunCheckpoint) -> Result<()> {
        ensure!(
            self.writable
                && self.position.tick == self.target_tick
                && checkpoint.simulation.world.tick == self.position.tick
                && checkpoint.simulation.next_event_sequence == self.position.next_sequence
                && checkpoint.policy == self.policy,
            "cannot finish database with a different or incomplete run"
        );
        checkpoint.restore_engine()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(
            get_meta::<Position>(&tx, "position")? == self.position,
            "database changed in another writer"
        );
        save_checkpoint(&tx, checkpoint, &self.position.chain_hash)?;
        tx.commit()?;
        Ok(())
    }

    /// Recover the latest committed tick from a checked checkpoint plus its event tail.
    /// This also works after process termination with an unfinished requested run.
    pub fn recover(&mut self) -> Result<ReplayResult> {
        self.read_run(false)
    }

    /// Re-execute the entire persisted segment and compare all stored checkpoints.
    pub fn replay(&mut self) -> Result<ReplayResult> {
        self.read_run(true)
    }

    fn read_run(&mut self, full: bool) -> Result<ReplayResult> {
        let tx = self.connection.transaction()?;
        let position: Position = get_meta(&tx, "position")?;
        verify_chain(&tx, &self.header, &position)?;
        let start = if full {
            self.header.checkpoint()
        } else {
            let (body, digest, hash): (String, String, String) = tx.query_row(
                "SELECT body,checksum,chain_hash FROM checkpoints ORDER BY tick DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let checkpoint: RunCheckpoint = parse_json(&body)?;
            ensure!(
                checksum(&checkpoint)? == digest
                    && checkpoint.simulation.world.tick <= position.tick,
                "checkpoint checksum or tick mismatch"
            );
            let expected_hash = boundary_hash(&tx, &self.header, checkpoint.simulation.world.tick)?;
            ensure!(
                hash == expected_hash,
                "checkpoint is not attached to its committed event boundary"
            );
            checkpoint
        };
        let start_tick = start.simulation.world.tick;
        let mut simulation = start.restore_engine()?;
        let mut policy = start.policy;
        let mut commits = tx.prepare(
            "SELECT tick,event_count,policy FROM tick_commits WHERE tick>?1 ORDER BY tick",
        )?;
        let rows = commits.query_map([sql_integer(start_tick)?], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, usize>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (tick, count, policy_json) = row?;
            let records = event_records(&tx, tick)?;
            ensure!(
                records.len() == count,
                "database committed event count mismatch"
            );
            simulation
                .replay_tick(&records)
                .with_context(|| format!("replay database tick {tick}"))?;
            let next_policy: PolicyCheckpoint = parse_json(&policy_json)?;
            next_policy.validate_transition(&policy, &simulation, &records)?;
            policy = next_policy;
            if full {
                let mut statement =
                    tx.prepare("SELECT body,checksum FROM checkpoints WHERE tick=?1")?;
                let mut checkpoint_rows = statement.query([sql_integer(tick)?])?;
                if let Some(row) = checkpoint_rows.next()? {
                    let saved: RunCheckpoint = parse_json(&row.get::<_, String>(0)?)?;
                    let actual = RunCheckpoint {
                        checkpoint_version: 1,
                        simulation: simulation.snapshot()?,
                        policy: policy.clone(),
                    };
                    ensure!(
                        saved == actual && row.get::<_, String>(1)? == checksum(&actual)?,
                        "database checkpoint differs from replay at tick {tick}"
                    );
                }
            }
        }
        drop(commits);
        ensure!(
            simulation.world().tick() == position.tick
                && simulation.next_event_sequence() == position.next_sequence,
            "recovered run differs from committed position"
        );
        let checkpoint = RunCheckpoint {
            checkpoint_version: 1,
            simulation: simulation.snapshot()?,
            policy: policy.clone(),
        };
        let target_tick = get_meta(&tx, "target_tick")?;
        tx.commit()?;
        let first_sequence =
            self.header.initial.next_event_sequence - self.header.initial_events_count as u64;
        let result = ReplayResult {
            checkpoint,
            replayed_ticks: position.tick - start_tick,
            verified_events: position.next_sequence - first_sequence,
        };
        self.position = position;
        self.policy = policy;
        self.target_tick = target_tick;
        Ok(result)
    }

    /// Verify event checksums and query a consistent read snapshot of matching facts.
    pub fn query_events(&mut self, query: &EventQuery) -> Result<Vec<EventRecord>> {
        ensure!(
            (1..=10_000).contains(&query.limit) && query.from_tick <= query.to_tick,
            "invalid event query range or limit"
        );
        let tx = self.connection.transaction()?;
        let position: Position = get_meta(&tx, "position")?;
        verify_chain(&tx, &self.header, &position)?;
        let mut statement = tx.prepare("SELECT e.body FROM events e WHERE e.tick>=?1 AND e.tick<=?2
            AND (?3 IS NULL OR EXISTS(SELECT 1 FROM event_agents a WHERE a.sequence=e.sequence AND a.agent_id=?3))
            ORDER BY e.sequence LIMIT ?4")?;
        let rows = statement.query_map(
            params![
                sql_integer(query.from_tick)?,
                sql_integer(query.to_tick)?,
                query.agent.map(|id| sql_integer(id.0)).transpose()?,
                query.limit as i64
            ],
            |row| row.get::<_, String>(0),
        )?;
        let result = rows
            .map(|row| parse_json(&row?))
            .collect::<Result<Vec<_>>>()?;
        drop(statement);
        tx.commit()?;
        Ok(result)
    }
}

fn configure(connection: &Connection) -> Result<()> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;")?;
    Ok(())
}

fn sql_integer(value: u64) -> Result<i64> {
    i64::try_from(value).context("SQLite IDs and ticks must fit a signed 64-bit integer")
}

fn parse_json<T: DeserializeOwned>(text: &str) -> Result<T> {
    ensure!(
        text.len() <= crate::MAX_DOCUMENT_BYTES,
        "database document exceeds size limit"
    );
    serde_json::from_str(text).context("invalid database JSON")
}

fn get_meta<T: DeserializeOwned>(connection: &Connection, key: &str) -> Result<T> {
    parse_json(
        &connection.query_row("SELECT value FROM metadata WHERE key=?1", [key], |row| {
            row.get::<_, String>(0)
        })?,
    )
}

fn put_meta(connection: &Connection, key: &str, value: &impl Serialize) -> Result<()> {
    connection.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, serde_json::to_string(value)?])?;
    Ok(())
}

fn insert_events(connection: &Connection, records: &[EventRecord]) -> Result<()> {
    let bodies = records
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    insert_event_bodies(connection, records, &bodies)
}

fn insert_event_bodies(
    connection: &Connection,
    records: &[EventRecord],
    bodies: &[String],
) -> Result<()> {
    let mut events =
        connection.prepare("INSERT INTO events(sequence,tick,body) VALUES(?1,?2,?3)")?;
    let mut agents =
        connection.prepare("INSERT INTO event_agents(sequence,agent_id) VALUES(?1,?2)")?;
    for (record, body) in records.iter().zip(bodies) {
        events.execute(params![
            sql_integer(record.sequence)?,
            sql_integer(record.tick)?,
            body
        ])?;
        for agent in record.event.participants() {
            agents.execute(params![
                sql_integer(record.sequence)?,
                sql_integer(agent.0)?
            ])?;
        }
    }
    Ok(())
}

fn save_checkpoint(
    connection: &Connection,
    checkpoint: &RunCheckpoint,
    chain_hash: &str,
) -> Result<()> {
    let body = serde_json::to_string(checkpoint)?;
    ensure!(
        body.len() <= crate::MAX_DOCUMENT_BYTES,
        "checkpoint exceeds document size limit"
    );
    connection.execute("INSERT INTO checkpoints(tick,body,checksum,chain_hash) VALUES(?1,?2,?3,?4)
        ON CONFLICT(tick) DO UPDATE SET body=excluded.body,checksum=excluded.checksum,chain_hash=excluded.chain_hash",
        params![sql_integer(checkpoint.simulation.world.tick)?, body, checksum(checkpoint)?, chain_hash])?;
    Ok(())
}

fn tick_hash(previous: &str, tick: Tick, bodies: &[String], policy: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(previous.as_bytes());
    hash.update(tick.to_le_bytes());
    hash.update((bodies.len() as u64).to_le_bytes());
    for body in bodies {
        hash.update(body.as_bytes());
        hash.update(b"\n");
    }
    hash.update(policy.as_bytes());
    format!("{:x}", hash.finalize())
}

fn event_records(connection: &Connection, tick: Tick) -> Result<Vec<EventRecord>> {
    let mut statement =
        connection.prepare("SELECT body FROM events WHERE tick=?1 ORDER BY sequence")?;
    let rows = statement.query_map([sql_integer(tick)?], |row| row.get::<_, String>(0))?;
    rows.map(|row| parse_json(&row?)).collect()
}

fn boundary_hash(connection: &Connection, header: &JournalHeader, tick: Tick) -> Result<String> {
    if tick == header.initial.world.tick {
        checksum(header)
    } else {
        Ok(connection.query_row(
            "SELECT chain_hash FROM tick_commits WHERE tick=?1",
            [sql_integer(tick)?],
            |row| row.get(0),
        )?)
    }
}

fn verify_chain(
    connection: &Connection,
    header: &JournalHeader,
    position: &Position,
) -> Result<()> {
    ensure!(
        get_meta::<JournalHeader>(connection, "header")? == *header
            && get_meta::<String>(connection, "header_checksum")? == checksum(header)?,
        "database header changed"
    );
    ensure!(
        get_meta::<Tick>(connection, "target_tick")? >= position.tick,
        "database target precedes committed ticks"
    );
    let first = header
        .initial
        .next_event_sequence
        .checked_sub(header.initial_events_count as u64)
        .context("invalid initial sequence")?;
    let (count, min, max): (u64, Option<u64>, Option<u64>) = connection.query_row(
        "SELECT count(*),min(sequence),max(sequence) FROM events",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    ensure!(
        position.next_sequence >= first
            && count == position.next_sequence - first
            && (count == 0 || (min == Some(first) && max == position.next_sequence.checked_sub(1))),
        "database has missing or extra event rows"
    );
    let initialization = event_records(connection, header.initial.world.tick)?;
    if header.initial_events_count > 0 {
        ensure!(
            initialization == Simulation::new(header.initial.config.clone())?.initial_events(),
            "database initialization mismatch"
        );
    } else {
        ensure!(
            initialization.is_empty(),
            "unexpected events at the initial boundary"
        );
    }
    let mut previous = Position {
        tick: header.initial.world.tick,
        next_sequence: header.initial.next_event_sequence,
        chain_hash: checksum(header)?,
    };
    let mut commits = connection.prepare(
        "SELECT tick,event_count,last_sequence,policy,chain_hash FROM tick_commits ORDER BY tick",
    )?;
    let rows = commits.query_map([], |row| {
        Ok((
            row.get::<_, u64>(0)?,
            row.get::<_, usize>(1)?,
            row.get::<_, u64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut statement =
        connection.prepare("SELECT sequence,body FROM events WHERE tick=?1 ORDER BY sequence")?;
    for row in rows {
        let (tick, expected_count, last, policy, hash) = row?;
        ensure!(
            expected_count
                <= header
                    .initial
                    .config
                    .agents
                    .saturating_mul(MAX_EVENTS_PER_AGENT)
                    .saturating_add(1024),
            "database tick exceeds event limit"
        );
        ensure!(
            tick == previous.tick + 1,
            "missing or out-of-order database tick commit"
        );
        let records = statement
            .query_map([sql_integer(tick)?], |row| {
                Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ensure!(
            records.len() == expected_count
                && records.iter().enumerate().all(|(i, (sequence, _))| previous
                    .next_sequence
                    .checked_add(i as u64)
                    == Some(*sequence))
                && records
                    .last()
                    .is_some_and(|(sequence, _)| *sequence == last),
            "database tick event sequence mismatch"
        );
        let bodies: Vec<_> = records.into_iter().map(|(_, body)| body).collect();
        ensure!(
            tick_hash(&previous.chain_hash, tick, &bodies, &policy) == hash,
            "database event checksum mismatch at tick {tick}"
        );
        previous = Position {
            tick,
            next_sequence: last.checked_add(1).context("sequence overflow")?,
            chain_hash: hash,
        };
    }
    ensure!(
        previous == *position,
        "database commit chain differs from saved position"
    );
    let mut checkpoints = connection
        .prepare("SELECT tick,body,checksum,chain_hash FROM checkpoints ORDER BY tick")?;
    let rows = checkpoints.query_map([], |row| {
        Ok((
            row.get::<_, u64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut has_initial = false;
    for row in rows {
        let (tick, body, digest, chain_hash) = row?;
        let checkpoint: RunCheckpoint = parse_json(&body)?;
        ensure!(
            tick >= header.initial.world.tick
                && tick <= position.tick
                && checkpoint.simulation.world.tick == tick
                && checkpoint.checkpoint_version == 1
                && checkpoint.simulation.config == header.initial.config
                && checksum(&checkpoint)? == digest
                && boundary_hash(connection, header, tick)? == chain_hash,
            "database checkpoint checksum or boundary mismatch"
        );
        if tick == header.initial.world.tick {
            ensure!(
                checkpoint == header.checkpoint(),
                "initial database checkpoint differs from header"
            );
            has_initial = true;
        } else {
            let (last, policy): (u64, String) = connection.query_row(
                "SELECT last_sequence,policy FROM tick_commits WHERE tick=?1",
                [sql_integer(tick)?],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            ensure!(
                last.checked_add(1) == Some(checkpoint.simulation.next_event_sequence)
                    && parse_json::<PolicyCheckpoint>(&policy)? == checkpoint.policy,
                "checkpoint resets counters or policy budgets"
            );
        }
    }
    ensure!(has_initial, "initial database checkpoint is missing");
    Ok(())
}
