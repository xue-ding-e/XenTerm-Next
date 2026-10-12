use super::*;
use serde_json::json;

fn record(command: &str) -> ExecutionRecord {
    let now = now_ms();
    ExecutionRecord {
        id: uuid::Uuid::new_v4().to_string(),
        started_at_ms: now,
        finished_at_ms: now,
        operation: "run_command".into(),
        source: "mcp".into(),
        session_id: "fixture".into(),
        session: "Loopback fixture".into(),
        command: command.into(),
        command_truncated: false,
        outcome: "succeeded".into(),
        exit_code: Some(0),
    }
}

#[test]
fn reading_disabled_or_missing_history_does_not_create_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("mcp-executions");
    assert!(read_records_in(&path).is_empty());
    assert!(!path.exists());
}

#[test]
fn concurrent_operations_keep_complete_independent_records() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_path_buf();
    let threads: Vec<_> = (0..20)
        .map(|n| {
            let path = path.clone();
            std::thread::spawn(move || {
                append_in(&path, &record(&format!("printf '中文 {n}'")), 7).unwrap()
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let records = read_records_in(&path);
    assert_eq!(records.len(), 20);
    let unique: std::collections::HashSet<_> = records.iter().map(|r| &r.id).collect();
    assert_eq!(unique.len(), 20);
}

#[test]
fn timeout_and_failed_exit_are_not_reported_as_success() {
    assert_eq!(
        result_outcome(
            "run_command",
            &Ok(json!({"exit_code": 0, "timed_out": true}))
        ),
        "timed_out"
    );
    assert_eq!(
        result_outcome("run_command", &Ok(json!({"exit_code": 3}))),
        "failed"
    );
    assert_eq!(
        result_outcome("run_command", &Ok(json!({"exit_code": null}))),
        "failed"
    );
    assert_eq!(
        result_outcome("run_command", &Ok(json!({"exit_code": 0}))),
        "succeeded"
    );
    assert_eq!(
        result_outcome("read_remote_text_file", &Ok(json!({"text": "fixture"}))),
        "succeeded"
    );
    assert_eq!(
        result_outcome(
            "run_command",
            &Err(anyhow::anyhow!("connection unavailable"))
        ),
        "failed"
    );
}

#[test]
fn retention_only_removes_older_execution_days() {
    let root = tempfile::tempdir().unwrap();
    let yesterday = chrono::Utc::now().date_naive() - chrono::Duration::days(1);
    let old = root.path().join(format!("executions-{yesterday}.jsonl"));
    let unrelated = root.path().join("personal-note.jsonl");
    std::fs::write(&old, "old fixture").unwrap();
    std::fs::write(&unrelated, "unrelated fixture").unwrap();
    let current = record("printf today");
    append_in(root.path(), &current, 1).unwrap();
    assert!(!old.exists());
    assert!(unrelated.exists());
    assert_eq!(read_records_in(root.path()), [current]);
}

#[test]
fn daily_cap_does_not_expand_the_log_or_change_a_result() {
    let root = tempfile::tempdir().unwrap();
    let value = record("printf capped");
    let at = chrono::DateTime::from_timestamp_millis(value.finished_at_ms as i64).unwrap();
    let path = root
        .path()
        .join(format!("executions-{}.jsonl", at.format("%Y-%m-%d")));
    std::fs::File::create(&path)
        .unwrap()
        .set_len(MAX_FILE_BYTES)
        .unwrap();
    assert!(append_in(root.path(), &value, 7).is_ok());
    assert_eq!(std::fs::metadata(path).unwrap().len(), MAX_FILE_BYTES);
}

#[test]
fn malformed_tail_does_not_hide_prior_records() {
    let root = tempfile::tempdir().unwrap();
    let value = record("printf retained");
    append_in(root.path(), &value, 7).unwrap();
    let path = std::fs::read_dir(root.path())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .unwrap();
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(b"{unfinished").unwrap();
    drop(file);
    assert_eq!(read_records_in(root.path()), [value.clone()]);
    let next = record("printf next");
    append_in(root.path(), &next, 7).unwrap();
    let recovered = read_records_in(root.path());
    assert_eq!(recovered.len(), 2);
    assert!(recovered.contains(&value) && recovered.contains(&next));
}

#[test]
fn long_multibyte_commands_are_bounded_without_invalid_utf8() {
    let original = "中文".repeat(6000);
    let clipped = bounded(&original, MAX_COMMAND_BYTES);
    assert!(clipped.len() <= MAX_COMMAND_BYTES);
    assert!(original.starts_with(&clipped));
    assert!(original.len() > clipped.len());
}

fn write_fixture_day(dir: &Path, day: usize, body: &[u8]) {
    std::fs::write(dir.join(format!("executions-2026-01-{day:02}.jsonl")), body).unwrap();
}

#[test]
fn read_file_budget_counts_malformed_files_and_prefers_newest_days() {
    let root = tempfile::tempdir().unwrap();
    let value = record("older valid record");
    let body = serde_json::to_vec(&value).unwrap();
    write_fixture_day(root.path(), 1, &body);
    write_fixture_day(root.path(), 2, b"invalid JSON\n");
    write_fixture_day(root.path(), 3, b"also invalid\n");
    assert!(read_records_with_budget(root.path(), 2, MAX_READ_BYTES).is_empty());
    assert_eq!(read_records_with_budget(root.path(), 3, MAX_READ_BYTES), [value]);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 3);
}

#[test]
fn read_byte_budget_counts_corruption_and_accepts_exact_boundary() {
    let root = tempfile::tempdir().unwrap();
    let value = record("older valid record");
    let body = serde_json::to_vec(&value).unwrap();
    write_fixture_day(root.path(), 1, &body);
    let corrupt = vec![b'!'; body.len()];
    write_fixture_day(root.path(), 2, &corrupt);
    assert!(read_records_with_budget(root.path(), 2, body.len() as u64).is_empty());
    assert!(read_records_with_budget(root.path(), 2, (body.len() * 2 - 1) as u64).is_empty());
    assert_eq!(
        read_records_with_budget(root.path(), 2, (body.len() * 2) as u64),
        [value]
    );
    assert_eq!(std::fs::read(root.path().join("executions-2026-01-02.jsonl")).unwrap(), corrupt);
}

#[test]
fn zero_budgets_do_not_read_and_invalid_names_or_oversized_files_are_ignored() {
    let root = tempfile::tempdir().unwrap();
    let value = record("valid record");
    let body = serde_json::to_vec(&value).unwrap();
    write_fixture_day(root.path(), 1, &body);
    for name in ["executions-not-a-date.jsonl", "executions-2026-1-02.jsonl", "personal-note.jsonl"] {
        std::fs::write(root.path().join(name), &body).unwrap();
    }
    let oversized = root.path().join("executions-2026-01-03.jsonl");
    std::fs::File::create(&oversized).unwrap().set_len(MAX_FILE_BYTES + 1).unwrap();
    std::fs::create_dir(root.path().join("executions-2026-01-04.jsonl")).unwrap();
    assert!(read_records_with_budget(root.path(), 0, MAX_READ_BYTES).is_empty());
    assert!(read_records_with_budget(root.path(), MAX_READ_FILES, 0).is_empty());
    assert_eq!(read_records_in(root.path()), [value]);
    assert_eq!(std::fs::metadata(oversized).unwrap().len(), MAX_FILE_BYTES + 1);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 6);
}

#[test]
fn read_record_limit_keeps_latest_lines_and_tolerates_invalid_utf8() {
    let root = tempfile::tempdir().unwrap();
    let mut value = record("");
    let mut body = Vec::new();
    for index in 0..=MAX_READ_RECORDS {
        value.id = index.to_string();
        value.finished_at_ms = index as u64;
        serde_json::to_writer(&mut body, &value).unwrap();
        body.push(b'\n');
    }
    body.extend_from_slice(b"\xffinvalid\n");
    assert!(body.len() as u64 <= MAX_FILE_BYTES);
    write_fixture_day(root.path(), 1, &body);
    let result = read_records_in(root.path());
    assert_eq!(result.len(), MAX_READ_RECORDS);
    assert_eq!(result.first().unwrap().id, MAX_READ_RECORDS.to_string());
    assert_eq!(result.last().unwrap().id, "1");
}

#[cfg(unix)]
#[test]
fn reads_never_follow_log_or_directory_symlinks() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let logs = root.path().join("logs");
    std::fs::create_dir(&logs).unwrap();
    let target = root.path().join("private.jsonl");
    let body = serde_json::to_vec(&record("not a log")).unwrap();
    std::fs::write(&target, &body).unwrap();
    symlink(&target, logs.join("executions-2026-01-01.jsonl")).unwrap();
    assert!(read_records_in(&logs).is_empty());
    let alias = root.path().join("alias");
    symlink(&logs, &alias).unwrap();
    assert!(read_records_in(&alias).is_empty());
    assert_eq!(std::fs::read(target).unwrap(), body);
}
