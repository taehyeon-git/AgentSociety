//! Durable run boundaries and deterministic replay. Reading never contacts a model.

mod checkpoint;
pub use checkpoint::{checksum, read_checkpoint, write_checkpoint};
mod journal;
pub use journal::{JournalHeader, JournalRecord, JournalWriter, ReplayResult, replay_journal};
mod sqlite;
pub use sqlite::{EventQuery, RunStore};

/// Upper bound for a single checkpoint, journal header or footer (256 MiB).
pub const MAX_DOCUMENT_BYTES: usize = 256 * 1024 * 1024;

/// Detect a database by its file signature instead of its extension.
pub fn is_sqlite(path: &std::path::Path) -> anyhow::Result<bool> {
    use std::io::Read;
    let mut bytes = [0_u8; 16];
    let mut file = std::fs::File::open(path)?;
    Ok(file.read(&mut bytes)? == bytes.len() && &bytes == b"SQLite format 3\0")
}

/// Read a checked state, checkpoint, JSONL journal or SQLite run without modifying its source.
/// Restoring a live external policy is a separate step and preserves recorded call budgets.
pub fn read_run(path: &std::path::Path) -> anyhow::Result<agentsociety_simulation::RunCheckpoint> {
    if is_sqlite(path)? {
        return Ok(RunStore::open(path, false)?.recover()?.checkpoint);
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("jsonl") || e.eq_ignore_ascii_case("ndjson"))
    {
        return Ok(replay_journal(std::io::BufReader::new(std::fs::File::open(path)?))?.checkpoint);
    }
    read_checkpoint(path)
}

/// Parse and validate a bounded intervention schedule before changing a run.
pub fn read_scenario(path: &std::path::Path) -> anyhow::Result<agentsociety_core::ScenarioConfig> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(1_048_577)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 1_048_576, "scenario exceeds 1 MiB");
    let scenario: agentsociety_core::ScenarioConfig = serde_json::from_slice(&bytes)?;
    scenario.validate()?;
    Ok(scenario)
}
