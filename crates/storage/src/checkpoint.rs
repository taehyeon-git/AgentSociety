use agentsociety_core::PolicyKind;
use agentsociety_simulation::{PolicyCheckpoint, RunCheckpoint, SimulationSnapshot};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedCheckpoint {
    checksum_algorithm: String,
    checksum: String,
    checkpoint: RunCheckpoint,
}

/// SHA-256 of canonical compact JSON, independent of file indentation and whitespace.
pub fn checksum(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

/// Validate, flush and atomically install a restartable checkpoint in the target directory.
/// An unsuccessful serialization or no-clobber write preserves an existing destination.
pub fn write_checkpoint(path: &Path, checkpoint: &RunCheckpoint, overwrite: bool) -> Result<()> {
    checkpoint
        .restore_engine()
        .context("validate checkpoint before saving")?;
    let document = SavedCheckpoint {
        checksum_algorithm: "sha256".into(),
        checksum: checksum(checkpoint)?,
        checkpoint: checkpoint.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&document)?;
    ensure!(
        bytes.len() < crate::MAX_DOCUMENT_BYTES,
        "checkpoint exceeds document size limit"
    );
    let absolute = std::path::absolute(path)?;
    let parent = absolute
        .parent()
        .context("checkpoint needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.write_all(b"\n")?;
    temporary.as_file_mut().sync_all()?;
    let result = if overwrite {
        temporary.persist(path)
    } else {
        temporary.persist_noclobber(path)
    };
    result.map_err(|error| error.error).with_context(|| {
        format!(
            "save checkpoint {} (use --overwrite to replace)",
            path.display()
        )
    })?;
    Ok(())
}

/// Read a checked checkpoint, a raw RunCheckpoint, or a rule-only engine state export.
/// All variants undergo the same domain checks before a caller can resume them.
pub fn read_checkpoint(path: &Path) -> Result<RunCheckpoint> {
    let file = File::open(path).with_context(|| format!("open checkpoint {}", path.display()))?;
    ensure!(
        file.metadata()?.len() <= crate::MAX_DOCUMENT_BYTES as u64,
        "checkpoint exceeds document size limit"
    );
    let mut bytes = Vec::new();
    file.take(crate::MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= crate::MAX_DOCUMENT_BYTES,
        "checkpoint exceeds document size limit"
    );
    let shape: serde_json::Value =
        serde_json::from_slice(&bytes).context("parse checkpoint JSON")?;
    let checkpoint = if shape.get("checkpoint").is_some() {
        let saved: SavedCheckpoint = serde_json::from_slice(&bytes)?;
        ensure!(
            saved.checksum_algorithm == "sha256" && saved.checksum == checksum(&saved.checkpoint)?,
            "checkpoint checksum mismatch"
        );
        saved.checkpoint
    } else if shape.get("checkpoint_version").is_some() {
        serde_json::from_slice(&bytes)?
    } else {
        let simulation: SimulationSnapshot = serde_json::from_slice(&bytes)?;
        if simulation.config.policy != PolicyKind::RuleBased {
            bail!("external-policy state requires a full checkpoint with saved request budgets");
        }
        RunCheckpoint {
            checkpoint_version: 1,
            simulation,
            policy: PolicyCheckpoint::default(),
        }
    };
    checkpoint
        .restore_engine()
        .context("validate loaded checkpoint")?;
    Ok(checkpoint)
}
