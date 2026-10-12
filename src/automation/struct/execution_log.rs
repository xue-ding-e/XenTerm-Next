//! Optional personal history for remote CLI/MCP operations.
//! Disabled profiles do not create files. Outputs and saved credentials are excluded.
//! This history reports attempts and results; it never authorizes an operation.

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Frontend;

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_COMMAND_BYTES: usize = 8 * 1024;
// Bound work even when every line is malformed or every daily file is full.
const MAX_READ_FILES: usize = 32;
const MAX_READ_BYTES: u64 = 16 * 1024 * 1024;
const MAX_READ_ENTRIES: usize = 4096;
const MAX_READ_RECORDS: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ExecutionRecord {
    pub id: String,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub operation: String,
    pub source: String,
    pub session_id: String,
    pub session: String,
    pub command: String,
    pub command_truncated: bool,
    pub outcome: String,
    pub exit_code: Option<u32>,
}

pub(crate) fn log_dir() -> PathBuf {
    crate::config::data_dir().join("mcp-executions")
}

pub(crate) struct ExecutionGuard {
    record: Option<ExecutionRecord>,
}

impl ExecutionGuard {
    pub(crate) fn begin(name: &str, arguments: &Value, frontend: Frontend) -> Option<Self> {
        if !matches!(
            name,
            "run_command"
                | "list_remote_files"
                | "read_remote_text_file"
                | "upload_file"
                | "download_file"
        ) {
            return None;
        }
        let store = crate::config::ConfigStore::load().ok()?;
        if !store.mcp_execution_log_enabled() || (frontend.is_mcp() && !store.mcp_enabled()) {
            return None;
        }
        let id = arguments
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let session = store
            .get(id)
            .map(|s| format!("{} ({})", s.name, s.host))
            .unwrap_or_else(|| id.to_string());
        let details = match name {
            "run_command" => text_argument(arguments, "command"),
            "upload_file" => format!(
                "{} → {}",
                text_argument(arguments, "local_path"),
                text_argument(arguments, "remote_directory")
            ),
            "download_file" => format!(
                "{} → {}",
                text_argument(arguments, "remote_path"),
                text_argument(arguments, "local_directory")
            ),
            _ => arguments
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or(".")
                .to_string(),
        };
        Some(Self {
            record: Some(ExecutionRecord {
                id: uuid::Uuid::new_v4().to_string(),
                started_at_ms: now_ms(),
                finished_at_ms: 0,
                operation: name.to_string(),
                source: if frontend.is_mcp() { "mcp" } else { "cli" }.to_string(),
                session_id: bounded(id, 256),
                session: bounded(&session, 1024),
                command_truncated: details.len() > MAX_COMMAND_BYTES,
                command: bounded(&details, MAX_COMMAND_BYTES),
                outcome: "interrupted".to_string(),
                exit_code: None,
            }),
        })
    }

    pub(crate) fn finish(&mut self, result: &anyhow::Result<Value>) {
        if let Some(record) = &mut self.record {
            record.outcome = result_outcome(&record.operation, result).to_string();
            record.exit_code = result
                .as_ref()
                .ok()
                .and_then(|v| v.get("exit_code"))
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok());
        }
        self.publish();
    }

    fn publish(&mut self) {
        let Some(mut record) = self.record.take() else {
            return;
        };
        // Re-read the opt-in flag: turning history off also stops in-flight writes.
        let Ok(store) = crate::config::ConfigStore::load() else {
            return;
        };
        if !store.mcp_execution_log_enabled() {
            return;
        }
        record.finished_at_ms = now_ms();
        if let Err(error) = append_in(
            &log_dir(),
            &record,
            store.mcp_execution_log_retention_days(),
        ) {
            // History failures must not change the remote operation's result.
            tracing::warn!("could not save execution history: {error}");
        }
    }
}

impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        self.publish();
    }
}

fn text_argument(arguments: &Value, key: &str) -> String {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn bounded(value: &str, maximum: usize) -> String {
    let mut end = value.len().min(maximum);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn result_outcome(name: &str, result: &anyhow::Result<Value>) -> &'static str {
    match result {
        Err(_) => "failed",
        Ok(value) if value.get("timed_out").and_then(Value::as_bool) == Some(true) => "timed_out",
        Ok(value)
            if name == "run_command"
                && value.get("exit_code").and_then(Value::as_u64) != Some(0) =>
        {
            "failed"
        }
        Ok(_) => "succeeded",
    }
}

fn reject_link(path: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "linked execution log path",
        ));
    }
    Ok(())
}

fn append_in(dir: &Path, record: &ExecutionRecord, retention_days: u64) -> io::Result<()> {
    reject_link(dir)?;
    std::fs::create_dir_all(dir)?;
    let lock_path = dir.join(".lock");
    reject_link(&lock_path)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.lock_exclusive()?;
    prune_in(dir, retention_days, record.finished_at_ms);
    let millis = i64::try_from(record.finished_at_ms).map_err(io::Error::other)?;
    let at = chrono::DateTime::from_timestamp_millis(millis)
        .ok_or_else(|| io::Error::other("invalid execution timestamp"))?;
    let path = dir.join(format!("executions-{}.jsonl", at.format("%Y-%m-%d")));
    reject_link(&path)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)?;
    let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
    line.push(b'\n');
    let length = file.metadata()?.len();
    let needs_newline = if length > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut tail = [0];
        file.read_exact(&mut tail)?;
        tail[0] != b'\n'
    } else { false };
    // Personal history is bounded. At the daily cap, recording resumes tomorrow.
    if length.saturating_add(line.len() as u64 + u64::from(needs_newline)) > MAX_FILE_BYTES {
        return Ok(());
    }
    // A crashed writer's partial tail must not swallow the next complete record.
    if needs_newline { file.write_all(b"\n")?; }
    file.write_all(&line)?;
    file.sync_data()
}

fn prune_in(dir: &Path, retention_days: u64, timestamp_ms: u64) {
    let Some(today) = i64::try_from(timestamp_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|t| t.date_naive())
    else {
        return;
    };
    let Some(cutoff) = today.checked_sub_signed(chrono::Duration::days(
        retention_days.clamp(1, 365).saturating_sub(1) as i64,
    )) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let date = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("executions-"))
            .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
        if date.is_some_and(|day| day < cutoff) && reject_link(&path).is_ok() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Read a bounded number of newest records without creating or pruning files.
pub(crate) fn read_records() -> Vec<ExecutionRecord> {
    read_records_in(&log_dir())
}

fn read_records_in(dir: &Path) -> Vec<ExecutionRecord> {
    read_records_with_budget(dir, MAX_READ_FILES, MAX_READ_BYTES)
}

fn read_records_with_budget(dir: &Path, max_files: usize, max_bytes: u64) -> Vec<ExecutionRecord> {
    if max_files == 0 || max_bytes == 0 || reject_link(dir).is_err() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // Normal retention has at most 365 daily files. Cap discovery as well for
    // externally modified directories; this viewer is not an archival search.
    let mut paths: Vec<_> = entries
        .take(MAX_READ_ENTRIES)
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix("executions-"))
                .and_then(|s| s.strip_suffix(".jsonl"))
                .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
                    .filter(|date| date.format("%Y-%m-%d").to_string() == s))
                .is_some()
        })
        .collect();
    paths.sort_by(|a, b| b.cmp(a));
    let mut records = Vec::new();
    let mut bytes_left = max_bytes;
    for path in paths.into_iter().take(max_files) {
        if bytes_left == 0 || records.len() >= MAX_READ_RECORDS {
            break;
        }
        if !std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            continue;
        }
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        // Charge attempted bytes even if a read fails. Take enforces the bound
        // on the opened file even if an external writer grows it after metadata.
        // Stop before a file that cannot fit: never parse a budget-truncated line.
        if metadata.len() > bytes_left {
            break;
        }
        let allowance = metadata.len();
        let mut body = Vec::new();
        let result = file.take(allowance).read_to_end(&mut body);
        bytes_left = bytes_left.saturating_sub(body.len() as u64);
        if result.is_err() {
            // An I/O error must not allow repeated full-budget reads.
            bytes_left = bytes_left.saturating_sub(allowance.saturating_sub(body.len() as u64));
            continue;
        }
        let remaining = MAX_READ_RECORDS.saturating_sub(records.len());
        records.extend(
            body.split(|byte| *byte == b'\n')
                .rev()
                .filter_map(|line| serde_json::from_slice(line).ok())
                .take(remaining),
        );
    }
    records.sort_by_key(|record: &ExecutionRecord| std::cmp::Reverse(record.finished_at_ms));
    records
}

#[cfg(test)]
#[path = "execution_log_tests.rs"]
mod tests;
