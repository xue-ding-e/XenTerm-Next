use super::*;
use serde_json::json;

thread_local! {
    static LOG_LOCK_REACHED: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> = const {
        std::cell::RefCell::new(None)
    };
    static LOG_SYNC_REACHED: std::cell::RefCell<Option<(
        std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>
    )>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn before_log_lock() {
    LOG_LOCK_REACHED.with(|signal| {
        if let Some(signal) = signal.borrow_mut().take() {
            signal.send(()).unwrap();
        }
    });
}

pub(super) fn after_log_sync() {
    LOG_SYNC_REACHED.with(|signal| {
        if let Some((reached, resume)) = signal.borrow_mut().take() {
            reached.send(()).unwrap();
            resume
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
        }
    });
}

fn history_store(root: &Path) -> crate::config::ConfigStore {
    use crate::config::{ConfigFile, ConfigStore, SavedState};
    let cache = ConfigFile::default();
    let saved_state = std::sync::Mutex::new(SavedState::of_cache(&cache)).into();
    let mut store = ConfigStore {
        path: root.join("sessions.db"),
        backup_dir: None,
        cache,
        key: [7; 32],
        keyring_enabled: false,
        saved_state,
    };
    store.set_mcp_execution_log_enabled(true);
    store.save().unwrap();
    store
}

fn guard(store: &crate::config::ConfigStore, command: &str) -> ExecutionGuard {
    ExecutionGuard::begin_with_store(
        "run_command",
        &json!({"session_id": "fixture", "command": command}),
        Frontend::Cli,
        store,
    )
    .unwrap()
}

#[test]
fn opt_out_suppresses_finish_and_drop_without_creating_history() {
    let root = tempfile::tempdir().unwrap();
    let mut store = history_store(root.path());
    let mut finished = guard(&store, "finish after opt-out");
    let dropped = guard(&store, "drop after opt-out");
    assert_eq!(
        finished.epoch, None,
        "legacy enabled profiles need no token migration"
    );
    store.set_mcp_execution_log_enabled(false);
    store.save().unwrap();
    finished.finish(&Ok(json!({"exit_code": 0})));
    drop(dropped);
    assert!(!root.path().join("mcp-executions").exists());
}

#[test]
fn opt_out_then_reenable_does_not_revive_old_guards() {
    let root = tempfile::tempdir().unwrap();
    let mut store = history_store(root.path());
    let logs = root.path().join("mcp-executions");
    guard(&store, "retained history").finish(&Ok(json!({"exit_code": 0})));
    let saved = read_records_in(&logs);
    let mut finished = guard(&store, "revoked finish");
    let dropped = guard(&store, "revoked drop");
    store.set_mcp_execution_log_enabled(false);
    store.save().unwrap();
    store.set_mcp_execution_log_enabled(true);
    store.save().unwrap();
    finished.finish(&Ok(json!({"exit_code": 0})));
    drop(dropped);
    assert_eq!(read_records_in(&logs), saved);
    guard(&store, "new generation").finish(&Ok(json!({"exit_code": 0})));
    let records = read_records_in(&logs);
    assert_eq!(records.len(), saved.len() + 1);
    assert!(records
        .iter()
        .any(|record| record.command == "new generation"));
    assert!(records.contains(&saved[0]));
}

#[test]
fn opt_out_commits_while_publisher_waits_for_log_lock() {
    for reenable in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut store = history_store(root.path());
        let logs = root.path().join("mcp-executions");
        guard(&store, "retained history").finish(&Ok(json!({"exit_code": 0})));
        let saved = read_records_in(&logs);
        let mut pending = guard(&store, "queued before opt-out");
        let held = lock_log_in(&logs).unwrap();
        let (reached, waiting) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            LOG_LOCK_REACHED.with(|signal| *signal.borrow_mut() = Some(reached));
            pending.finish(&Ok(json!({"exit_code": 0})));
        });
        // This signal is after the enabled pre-check and immediately before
        // the contended OS lock. No sleep or scheduler timing establishes it.
        waiting
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let (saved_tx, saved_rx) = std::sync::mpsc::channel();
        let saver = std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                store.set_mcp_execution_log_enabled(false);
                store.save()?;
                if reenable {
                    store.set_mcp_execution_log_enabled(true);
                    store.save()?;
                }
                Ok(())
            })();
            saved_tx.send(result).unwrap();
        });
        let save_result = saved_rx.recv_timeout(std::time::Duration::from_secs(5));
        assert_eq!(read_records_in(&logs), saved);
        // Release even on timeout, so an inverted-lock regression fails the
        // assertion rather than hanging the whole test binary.
        drop(held);
        writer.join().unwrap();
        saver.join().unwrap();
        save_result.unwrap().unwrap();
        assert_eq!(read_records_in(&logs), saved);
    }
}

#[test]
fn subprocess_opt_out_fixture() {
    let Some(root) = std::env::var_os("XENTERM_LOG_OPT_OUT_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    let mut args = vec![
        "xenterm".into(),
        "--data-dir".into(),
        root.display().to_string(),
        "cli".into(),
    ];
    crate::config::configure_profile(&mut args).unwrap();
    let mut store = crate::config::ConfigStore::load().unwrap();
    println!("log-fixture:ready");
    std::io::stdout().flush().unwrap();
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
    assert_eq!(input.trim(), "save");
    let profile_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("sessions.db.profile-lock"))
        .unwrap();
    // This is an actual second process, not just another file handle.
    assert!(fs2::FileExt::try_lock_exclusive(&profile_lock).is_err());
    println!("log-fixture:blocked");
    std::io::stdout().flush().unwrap();
    store.set_mcp_execution_log_enabled(false);
    store.save().unwrap();
    println!("log-fixture:saved");
}

#[test]
fn admitted_append_finishes_before_another_process_can_disable() {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    let root = tempfile::tempdir().unwrap();
    let store = history_store(root.path());
    let module = module_path!().split_once("::").unwrap().1;
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("{module}::subprocess_opt_out_fixture"),
            "--nocapture",
        ])
        .env("XENTERM_LOG_OPT_OUT_FIXTURE", root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let (output_tx, output_rx) = mpsc::channel();
    let output = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(output).lines() {
            if output_tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let wait_for = |expected: &str| loop {
        let line = output_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        if line.ends_with(expected) {
            break;
        }
    };
    wait_for("log-fixture:ready");
    let mut pending = guard(&store, "admitted before opt-out");
    let (reached, synced) = mpsc::channel();
    let (resume, paused) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        LOG_SYNC_REACHED.with(|signal| *signal.borrow_mut() = Some((reached, paused)));
        pending.finish(&Ok(json!({"exit_code": 0})));
    });
    synced.recv_timeout(Duration::from_secs(5)).unwrap();
    child.stdin.take().unwrap().write_all(b"save\n").unwrap();
    wait_for("log-fixture:blocked");
    resume.send(()).unwrap();
    writer.join().unwrap();
    wait_for("log-fixture:saved");
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();
    let saved = read_records_in(&root.path().join("mcp-executions"));
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].command, "admitted before opt-out");
    assert!(
        !crate::config::ConfigStore::with_execution_log_state(&store.path, |state| Ok(
            state.enabled
        ))
        .unwrap()
    );
}

#[test]
fn failed_history_io_never_changes_the_operation_result() {
    for corrupt_profile in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = history_store(root.path());
        let mut finished = guard(&store, "finish with unavailable history");
        let dropped = guard(&store, "drop with unavailable history");
        if corrupt_profile {
            let conn = rusqlite::Connection::open(&store.path).unwrap();
            conn.execute(
                "UPDATE meta SET value='invalid JSON' WHERE key='settings'",
                [],
            )
            .unwrap();
        } else {
            std::fs::write(root.path().join("mcp-executions"), "not a directory").unwrap();
        }
        let result = Ok(json!({"exit_code": 17, "stdout": "synthetic result"}));
        finished.finish(&result);
        drop(dropped);
        assert_eq!(
            result.unwrap(),
            json!({"exit_code": 17, "stdout": "synthetic result"})
        );
        assert!(read_records_in(&root.path().join("mcp-executions")).is_empty());
    }
}

#[test]
fn records_exclude_saved_credentials_unknown_arguments_and_outputs() {
    let root = tempfile::tempdir().unwrap();
    let mut store = history_store(root.path());
    let mut session = crate::config::Session::new_empty();
    session.id = "fixture".into();
    session.name = "Synthetic session".into();
    session.host = "fixture.invalid".into();
    session.password = crate::config::Secret::new("synthetic-saved-secret");
    store.upsert(session);
    store.save().unwrap();
    let mut pending = ExecutionGuard::begin_with_store(
        "run_command",
        &json!({
            "session_id": "fixture", "command": "printf fixture",
            "password": "synthetic-argument-secret", "token": "synthetic-token"
        }),
        Frontend::Cli,
        &store,
    )
    .unwrap();
    pending.finish(&Ok(json!({
        "exit_code": 0, "stdout": "synthetic-stdout", "stderr": "synthetic-stderr"
    })));
    let records = read_records_in(&root.path().join("mcp-executions"));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].command, "printf fixture");
    let raw = serde_json::to_string(&records).unwrap();
    for excluded in [
        "synthetic-saved-secret",
        "synthetic-argument-secret",
        "synthetic-token",
        "synthetic-stdout",
        "synthetic-stderr",
    ] {
        assert!(
            !raw.contains(excluded),
            "unexpected data in history: {excluded}"
        );
    }
}

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
