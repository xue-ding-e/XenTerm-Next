//! The human-approval queue for risky automation commands.
//!
//! The MCP server is its own stdio process with no window, and the person who
//! can approve a risky command is sitting at the main XenTerm window — a
//! different process, possibly not running at all. The bridge is a directory
//! under the data dir: a risky request lands there as a JSON file in state
//! `pending`, the main window's poll finds it and asks the human, and the
//! answer is written back into the same file in place. The MCP side polls its
//! one file until the status moves or its deadline passes. Windows headless
//! builds provide their own visible native confirmation window, without
//! requiring a separately installed or running GPUI desktop application.
//!
//! Fail-closed on every path: no window running, an expired request, a
//! timeout, a file that will not parse — all of it resolves to "not
//! approved", and the command does not run.
//!
//! Every finished request — approved or denied, by a human or by a timer —
//! lands in an append-only audit journal, one JSONL file per day. The files
//! are never edited or deleted one by one; a retention sweep drops whole days
//! past the configured age, and that is the only deletion there is.

use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use serde::{Deserialize, Serialize};

use super::risk::RiskVerdict;

/// Where the request files live; the audit journal lives in `audit/` beneath.
pub(crate) fn queue_dir() -> PathBuf {
    crate::config::data_dir().join("mcp-approvals")
}

/// How many requests may sit pending at once. A request arriving past this is
/// auto-denied (and audited as such): a queue that grows without bound is a
/// wall of dialogs waiting for someone who has probably left.
pub(crate) const MAX_PENDING: usize = 8;

/// The grace the MCP waiter adds on top of the window's decision deadline, so
/// the window always finishes writing the answer before the asker gives up.
const WAITER_MARGIN: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ApprovalRequest {
    pub(crate) id: String,
    /// The session the command would run on, described for the human (id and
    /// host — enough to judge, not enough to leak secrets into the file).
    pub(crate) session: String,
    pub(crate) command: String,
    pub(crate) reasons: Vec<String>,
    /// Unix epoch millis, from the requesting side.
    pub(crate) created_at_ms: u64,
    /// How long the asker is willing to wait for a human, in seconds. The
    /// window times its dialog by this — the countdown on the deny button
    /// reads it — and the asker waits it plus a margin.
    #[serde(default = "crate::config::default_approval_timeout_value")]
    pub(crate) wait_timeout_secs: u64,
    /// "pending" | "approved" | "denied"
    pub(crate) status: String,
    /// Who decided, written when the status leaves `pending`: "manual" when
    /// a human answered, "auto-timeout" when the window's deadline did.
    #[serde(default)]
    pub(crate) decided_by: String,
    /// The file this request was read from, filled by the scanning side.
    #[serde(skip)]
    pub(crate) path: Option<PathBuf>,
}

const PENDING: &str = "pending";
const APPROVED: &str = "approved";
const DENIED: &str = "denied";

pub(crate) fn past_deadline(request: &ApprovalRequest) -> bool {
    let Some(deadline_ms) = request.created_at_ms.checked_add(
        request
            .wait_timeout_secs
            .checked_mul(1000)
            .unwrap_or(u64::MAX),
    ) else {
        return true;
    };
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|age| {
            age.as_millis() < u128::from(request.created_at_ms)
                || age.as_millis() >= u128::from(deadline_ms)
        })
        .unwrap_or(true)
}

/// Seconds left on a request's decision deadline, for the deny button's
/// countdown. Never negative.
pub(crate) fn seconds_left(request: &ApprovalRequest) -> u64 {
    let deadline = request
        .created_at_ms
        .saturating_add(request.wait_timeout_secs.saturating_mul(1000));
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|age| {
            if age.as_millis() < u128::from(request.created_at_ms) {
                0
            } else {
                deadline
                    .saturating_sub(age.as_millis() as u64)
                    .div_ceil(1000)
            }
        })
        .unwrap_or(0)
}

/// One finished decision in the audit journal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AuditRecord {
    pub(crate) id: String,
    pub(crate) decided_at_ms: u64,
    pub(crate) session: String,
    pub(crate) command: String,
    pub(crate) reasons: Vec<String>,
    /// "approved" | "denied"
    pub(crate) outcome: String,
    /// "manual" | "auto-timeout" | "auto-expired" | "auto-limit" | "auto-unreachable"
    pub(crate) by: String,
}

/// What the human (or the timer) decided, returned to the automation layer
/// so the caller's error can say *who* said no — a user's denial and an
/// unattended timeout are different messages, and a client that conflates
/// them will retry the first as if it were the second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApprovalOutcome {
    /// A human pressed 批准 at the window.
    Approved,
    /// A human pressed 拒绝 at the window.
    DeniedByUser,
    /// The decision deadline passed with nobody at the window.
    DeniedByTimeout,
    /// The queue was already at its cap when the request arrived.
    DeniedByLimit,
    /// The window pruned the request as stale, or it could not be written.
    DeniedUnreachable,
}

impl ApprovalOutcome {
    pub(crate) fn is_approved(self) -> bool {
        matches!(self, Self::Approved)
    }
}

fn audit_dir(queue: &Path) -> PathBuf {
    queue.join("audit")
}

fn audit_path_for(queue: &Path, decided_at_ms: u64) -> PathBuf {
    // Day files in UTC: the smallest unit the retention sweep reasons about,
    // and a name that sorts the same way every filesystem lists it.
    let day = chrono::DateTime::from_timestamp_millis(decided_at_ms as i64)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown-day".to_string());
    audit_dir(queue).join(format!("audit-{day}.jsonl"))
}

/// Append one decision to the day file. Idempotent per request id: the same
/// decision arriving from two paths (the window timing out while the human
/// was mid-click) is recorded once, because an audit that double-counts is
/// an audit nobody trusts.
fn append_audit_in(queue: &Path, record: &AuditRecord) -> io::Result<()> {
    std::fs::create_dir_all(audit_dir(queue))?;
    if symlinked(&audit_dir(queue)) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "symlinked approval audit directory",
        ));
    }
    let _lock = lock_file(&audit_dir(queue).join(".lock"))?;
    let path = audit_path_for(queue, record.decided_at_ms);
    if symlinked(&path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "symlinked approval audit file",
        ));
    }
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if existing.lines().any(|line| {
            serde_json::from_str::<AuditRecord>(line)
                .map(|r| r.id == record.id)
                .unwrap_or(false)
        }) {
            return Ok(());
        }
    }
    let mut line = serde_json::to_string(record).map_err(io::Error::other)?;
    line.push('\n');
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(line.as_bytes())?;
    file.sync_data()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn lock_file(path: &Path) -> io::Result<std::fs::File> {
    if symlinked(path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "symlinked approval lock",
        ));
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)?;
    file.lock_exclusive()?;
    Ok(file)
}

fn request_path(dir: &Path, id: &str) -> io::Result<PathBuf> {
    if symlinked(dir) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "symlinked approval directory",
        ));
    }
    if !valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid approval id",
        ));
    }
    Ok(dir.join(format!("{id}.json")))
}

fn with_request_lock<T>(
    dir: &Path,
    id: &str,
    f: impl FnOnce(&Path) -> io::Result<T>,
) -> io::Result<T> {
    let path = request_path(dir, id)?;
    let _lock = lock_file(&dir.join(format!("{id}.lock")))?;
    if symlinked(&path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "symlinked approval request",
        ));
    }
    f(&path)
}

fn read_request(path: &Path, id: &str) -> io::Result<ApprovalRequest> {
    let request: ApprovalRequest =
        serde_json::from_slice(&std::fs::read(path)?).map_err(io::Error::other)?;
    if request.id != id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "approval id mismatch",
        ));
    }
    Ok(request)
}

fn write_request_atomic(path: &Path, request: &ApprovalRequest) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing approval directory"))?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(&mut temp, request).map_err(io::Error::other)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Read a request without changing its decision. A missing or invalid file is not pending.
pub(crate) fn request_state(id: &str) -> Option<ApprovalRequest> {
    request_state_in(&queue_dir(), id)
}

pub(crate) fn request_state_in(dir: &Path, id: &str) -> Option<ApprovalRequest> {
    let path = request_path(dir, id).ok()?;
    if symlinked(&path) {
        return None;
    }
    read_request(&path, id).ok()
}

/// Drop audit day files older than `retention_days`. Whole days only — there
/// is no single-record deletion, by design: an audit that can be edited is
/// an audit that can be rewritten.
pub(crate) fn prune_audit_in(queue: &Path, retention_days: u64) {
    let Ok(entries) = std::fs::read_dir(audit_dir(queue)) else {
        return;
    };
    let cutoff_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
        .saturating_sub(retention_days.saturating_mul(86_400_000));
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(day) = stem.strip_prefix("audit-") else {
            continue;
        };
        let Ok(day_start) = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") else {
            continue;
        };
        let day_start_ms = day_start
            .and_hms_opt(0, 0, 0)
            .and_then(|dt| dt.and_utc().timestamp_millis().try_into().ok())
            .unwrap_or(u64::MAX);
        if day_start_ms < cutoff_ms {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn now_ms_of() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Submit a request into `dir` and poll until a human answers through the
/// same directory, the request expires, or `timeout` elapses. Returns whether
/// the command may run; every abnormal path returns `false` (fail-closed).
///
/// A queue already at `MAX_PENDING` denies the new request immediately and
/// audits it — pressure on the queue is pressure on the human, and the safe
/// answer to that is no.
pub(crate) async fn request_approval_in(
    dir: &Path,
    session: String,
    command: String,
    verdict: &RiskVerdict,
    timeout: std::time::Duration,
) -> ApprovalOutcome {
    if std::fs::create_dir_all(dir).is_err() {
        return ApprovalOutcome::DeniedUnreachable;
    }
    if pending_requests_in(dir).len() >= MAX_PENDING {
        // Audited with the command that was refused, so the journal shows the
        // pressure and not just its consequences.
        let _ = append_audit_in(
            dir,
            &AuditRecord {
                id: uuid::Uuid::new_v4().to_string(),
                decided_at_ms: now_ms_of(),
                session: session.clone(),
                command: command.clone(),
                reasons: verdict.reasons.clone(),
                outcome: DENIED.to_string(),
                by: "auto-limit".to_string(),
            },
        );
        return ApprovalOutcome::DeniedByLimit;
    }

    let request = ApprovalRequest {
        id: uuid::Uuid::new_v4().to_string(),
        session,
        command: command.clone(),
        reasons: verdict.reasons.clone(),
        created_at_ms: now_ms_of(),
        wait_timeout_secs: timeout.as_secs().max(1),
        status: PENDING.to_string(),
        decided_by: String::new(),
        path: None,
    };
    if let Err(error) = with_request_lock(dir, &request.id, |path| {
        write_request_atomic(path, &request)
    }) {
        tracing::warn!(
            "approval request {} could not be written: {error}",
            request.id
        );
        return ApprovalOutcome::DeniedUnreachable;
    }

    // Poll our own file: only the requester consumes a terminal answer. The
    // margin lets a window finish a decision at the deadline before we deny.
    let begun = std::time::Instant::now();
    while begun.elapsed() < timeout + WAITER_MARGIN {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let observed = with_request_lock(dir, &request.id, |path| {
            let current = read_request(path, &request.id)?;
            let outcome = match current.status.as_str() {
                APPROVED => Some(ApprovalOutcome::Approved),
                DENIED => Some(match current.decided_by.as_str() {
                    "manual" => ApprovalOutcome::DeniedByUser,
                    "auto-unreachable" => ApprovalOutcome::DeniedUnreachable,
                    _ => ApprovalOutcome::DeniedByTimeout,
                }),
                PENDING => None,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid approval state",
                    ))
                }
            };
            if outcome.is_some() {
                std::fs::remove_file(path)?;
            }
            Ok(outcome)
        });
        match observed {
            Ok(Some(outcome)) => return outcome,
            Ok(None) => {
                if begun.elapsed() >= timeout || past_deadline(&request) {
                    let _ = resolve_timeout_in(dir, &request.id);
                }
            }
            Err(_) => return ApprovalOutcome::DeniedUnreachable,
        }
    }
    // A competing human decision may have landed on the last tick. Consume it
    // before deciding timeout, still under the same request lock.
    let _ = resolve_timeout_in(dir, &request.id);
    with_request_lock(dir, &request.id, |path| {
        let current = read_request(path, &request.id)?;
        let outcome = match current.status.as_str() {
            APPROVED => ApprovalOutcome::Approved,
            DENIED => match current.decided_by.as_str() {
                "manual" => ApprovalOutcome::DeniedByUser,
                "auto-unreachable" => ApprovalOutcome::DeniedUnreachable,
                _ => ApprovalOutcome::DeniedByTimeout,
            },
            _ => ApprovalOutcome::DeniedUnreachable,
        };
        if current.status != PENDING {
            std::fs::remove_file(path)?;
        }
        Ok(outcome)
    })
    .unwrap_or(ApprovalOutcome::DeniedUnreachable)
}

/// The real queue's wrapper — what the automation layer calls.
pub(crate) async fn request_approval(
    session: String,
    command: String,
    verdict: &RiskVerdict,
    timeout: std::time::Duration,
) -> ApprovalOutcome {
    #[cfg(all(windows, not(feature = "desktop")))]
    super::native_approval::ensure_worker();
    request_approval_in(&queue_dir(), session, command, verdict, timeout).await
}

/// Every live pending request in `dir`, oldest first — what the window's poll
/// shows. Expired pendings are finalized as denied, never removed ahead of
/// the requester that must consume their terminal result.
pub(crate) fn pending_requests_in(dir: &Path) -> Vec<ApprovalRequest> {
    scan_in(dir)
        .into_iter()
        .filter(|request| {
            if request.status != PENDING {
                return false;
            }
            if past_deadline(request) {
                let _ = resolve_timeout_in(dir, &request.id);
                return false;
            }
            true
        })
        .collect()
}

/// Every request file in `dir` regardless of status — what the window's
/// feedback pass reads: a file that has left `pending` is a decision the
/// human has made or a timeout the window itself recorded, and the asking
/// process is waiting for that fact to be seen.
pub(crate) fn scan_in(dir: &Path) -> Vec<ApprovalRequest> {
    if symlinked(dir) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut requests: Vec<ApprovalRequest> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && !symlinked(path))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|path| {
            let body = std::fs::read_to_string(&path).ok()?;
            let mut request: ApprovalRequest = serde_json::from_str(&body).ok()?;
            if !valid_id(&request.id)
                || path.file_stem().and_then(|s| s.to_str()) != Some(request.id.as_str())
            {
                return None;
            }
            request.path = Some(path);
            Some(request)
        })
        .collect();
    requests.sort_by_key(|request| request.created_at_ms);
    requests
}

/// Write the human's answer into the request file and audit it as a manual
/// decision. Called from the dialog's buttons. Only a live pending request
/// can change; a later close or click cannot reverse a terminal answer.
pub(crate) fn resolve_manual(id: &str, approved: bool) -> bool {
    resolve_manual_in(&queue_dir(), id, approved)
}

pub(crate) fn resolve_manual_in(dir: &Path, id: &str, approved: bool) -> bool {
    resolve_in(dir, id, Some(approved)).unwrap_or(false)
}

/// Record a presenter failure distinctly from an unanswered, visible prompt.
#[cfg(any(test, all(windows, not(feature = "desktop"))))]
pub(crate) fn resolve_unreachable_in(dir: &Path, id: &str) -> bool {
    with_request_lock(dir, id, |path| {
        let mut request = read_request(path, id)?;
        if request.status != PENDING {
            return Ok(false);
        }
        append_audit_in(
            dir,
            &AuditRecord {
                id: request.id.clone(),
                decided_at_ms: now_ms_of(),
                session: request.session.clone(),
                command: request.command.clone(),
                reasons: request.reasons.clone(),
                outcome: DENIED.to_string(),
                by: "auto-unreachable".to_string(),
            },
        )?;
        request.status = DENIED.to_string();
        request.decided_by = "auto-unreachable".to_string();
        write_request_atomic(path, &request)?;
        Ok(true)
    })
    .unwrap_or(false)
}

/// Finalize an unanswered request without removing the asker's answer.
pub(crate) fn resolve_timeout_in(dir: &Path, id: &str) -> bool {
    resolve_in(dir, id, None).unwrap_or(false)
}

fn resolve_in(dir: &Path, id: &str, manual: Option<bool>) -> io::Result<bool> {
    with_request_lock(dir, id, |path| {
        let mut request = read_request(path, id)?;
        if request.status != PENDING {
            return Ok(false);
        }
        let expired = past_deadline(&request);
        if manual.is_none() && !expired {
            return Ok(false);
        }
        let (outcome, by) = match manual {
            Some(true) if !expired => (APPROVED, "manual"),
            Some(false) if !expired => (DENIED, "manual"),
            _ => (DENIED, "auto-timeout"),
        };
        let record = AuditRecord {
            id: request.id.clone(),
            decided_at_ms: now_ms_of(),
            session: request.session.clone(),
            command: request.command.clone(),
            reasons: request.reasons.clone(),
            outcome: outcome.to_string(),
            by: by.to_string(),
        };
        // A failed journal write must never make an unaudited approval visible.
        append_audit_in(dir, &record)?;
        request.status = outcome.to_string();
        request.decided_by = by.to_string();
        write_request_atomic(path, &request)?;
        Ok(match manual {
            Some(approved) => !expired && (outcome == APPROVED) == approved,
            None => true,
        })
    })
}

/// Whether the path is a symlink — a queue file replaced by one must not be
/// read (and its verdict rewritten through it).
fn symlinked(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(reasons: &[&str]) -> RiskVerdict {
        RiskVerdict {
            reasons: reasons.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xenterm-approval-test-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn audit_lines(dir: &Path) -> Vec<AuditRecord> {
        std::fs::read_dir(audit_dir(dir))
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                    .flat_map(|body| {
                        body.lines()
                            .filter_map(|line| serde_json::from_str::<AuditRecord>(line).ok())
                            .collect::<Vec<_>>()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn pending_fixture(dir: &Path, id: &str, age_secs: u64, timeout_secs: u64) {
        let request = ApprovalRequest {
            id: id.into(),
            session: "s".into(),
            command: "c".into(),
            reasons: vec![],
            created_at_ms: now_ms_of().saturating_sub(age_secs * 1000),
            wait_timeout_secs: timeout_secs,
            status: PENDING.into(),
            decided_by: String::new(),
            path: None,
        };
        std::fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn approval_cannot_be_reversed_by_close_or_timeout() {
        let dir = fresh_dir("approve-once");
        pending_fixture(&dir, "approve-once", 0, 60);
        assert!(resolve_manual_in(&dir, "approve-once", true));
        assert!(!resolve_manual_in(&dir, "approve-once", false));
        assert!(!resolve_manual_in(&dir, "approve-once", true));
        assert!(!resolve_timeout_in(&dir, "approve-once"));
        assert_eq!(
            request_state_in(&dir, "approve-once").unwrap().status,
            APPROVED
        );
        let audits = audit_lines(&dir);
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].outcome, APPROVED);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expired_request_cannot_be_approved() {
        let dir = fresh_dir("expired-manual");
        pending_fixture(&dir, "expired-manual", 3, 1);
        assert!(!resolve_manual_in(&dir, "expired-manual", true));
        assert!(!resolve_timeout_in(&dir, "expired-manual"));
        assert!(!resolve_manual_in(&dir, "expired-manual", true));
        assert_eq!(
            request_state_in(&dir, "expired-manual").unwrap().status,
            DENIED
        );
        assert_eq!(audit_lines(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn future_created_at_fails_closed() {
        let dir = fresh_dir("future-clock");
        pending_fixture(&dir, "future-clock", 0, 60);
        let path = dir.join("future-clock.json");
        let mut request = request_state_in(&dir, "future-clock").unwrap();
        request.created_at_ms = now_ms_of().saturating_add(60_000);
        std::fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        assert!(past_deadline(&request));
        assert!(!resolve_manual_in(&dir, "future-clock", true));
        assert_eq!(
            request_state_in(&dir, "future-clock").unwrap().status,
            DENIED
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_conflicting_decisions_have_one_terminal_result() {
        let dir = fresh_dir("race");
        pending_fixture(&dir, "race", 0, 60);
        let first_dir = dir.clone();
        let second_dir = dir.clone();
        let first = std::thread::spawn(move || resolve_manual_in(&first_dir, "race", true));
        let second = std::thread::spawn(move || resolve_manual_in(&second_dir, "race", false));
        let changed = u8::from(first.join().unwrap()) + u8::from(second.join().unwrap());
        assert_eq!(changed, 1);
        let request = request_state_in(&dir, "race").unwrap();
        assert!(request.status == APPROVED || request.status == DENIED);
        let audits = audit_lines(&dir);
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].outcome, request.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn terminal_requests_do_not_count_as_pending_or_expire_again() {
        let dir = fresh_dir("terminal-count");
        pending_fixture(&dir, "terminal", 0, 60);
        assert!(resolve_manual_in(&dir, "terminal", true));
        pending_fixture(&dir, "still-pending", 0, 60);
        assert_eq!(pending_requests_in(&dir).len(), 1);
        assert_eq!(audit_lines(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unavailable_presenter_is_audited_without_reversing_a_decision() {
        let dir = fresh_dir("unreachable");
        pending_fixture(&dir, "unreachable", 0, 60);
        assert!(resolve_unreachable_in(&dir, "unreachable"));
        assert!(!resolve_manual_in(&dir, "unreachable", true));
        assert!(!resolve_unreachable_in(&dir, "unreachable"));
        let request = request_state_in(&dir, "unreachable").unwrap();
        assert_eq!(request.status, DENIED);
        assert_eq!(request.decided_by, "auto-unreachable");
        let audits = audit_lines(&dir);
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].by, "auto-unreachable");
        pending_fixture(&dir, "approved", 0, 60);
        assert!(resolve_manual_in(&dir, "approved", true));
        assert!(!resolve_unreachable_in(&dir, "approved"));
        assert_eq!(request_state_in(&dir, "approved").unwrap().status, APPROVED);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_unsafe_id_fails_closed() {
        let dir = fresh_dir("missing");
        assert!(request_state_in(&dir, "gone").is_none());
        assert!(!resolve_manual_in(&dir, "gone", true));
        assert!(!resolve_manual_in(&dir, "../outside", true));
        assert!(request_state_in(&dir, "../outside").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A request with no window answering it resolves to denied once the
    /// (short, test-sized) timeout passes, and leaves no file behind — but it
    /// does leave an audit record saying the timeout decided.
    #[tokio::test]
    async fn an_unanswered_request_fails_closed_and_is_audited() {
        let dir = fresh_dir("unanswered");
        let outcome = request_approval_in(
            &dir,
            "sess-1".into(),
            "rm -rf /tmp/build".into(),
            &verdict(&["matches: rm -rf"]),
            std::time::Duration::from_millis(400),
        )
        .await;
        assert_eq!(
            outcome,
            ApprovalOutcome::DeniedByTimeout,
            "nobody answered; the timeout is what said no"
        );
        // Only request files count — the audit journal lives in a subdirectory
        // of the same queue and is meant to survive the request.
        assert_eq!(
            std::fs::read_dir(&dir)
                .map(|entries| entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
                    .count())
                .unwrap_or(99),
            0,
            "the timed-out request is cleaned up"
        );
        let audits = audit_lines(&dir);
        assert_eq!(audits.len(), 1, "exactly one audit line");
        assert_eq!(audits[0].outcome, DENIED);
        assert_eq!(audits[0].by, "auto-timeout");
        assert_eq!(audits[0].command, "rm -rf /tmp/build");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The window's side of the protocol: submit on a task that mimics the
    /// asking process, see the request in `pending_requests_in`, resolve it,
    /// and watch the asker observe the answer — with a manual audit line.
    #[tokio::test]
    async fn the_window_sees_and_resolves_a_pending_request() {
        let dir = fresh_dir("resolve");
        let dir_for_asker = dir.clone();
        let asker = tokio::spawn(async move {
            request_approval_in(
                &dir_for_asker,
                "sess-2".into(),
                "shutdown -h now".into(),
                &verdict(&["matches: shutdown"]),
                std::time::Duration::from_secs(5),
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let pending = pending_requests_in(&dir);
        assert_eq!(pending.len(), 1, "the window's poll finds the request");
        assert_eq!(pending[0].command, "shutdown -h now");
        assert_eq!(pending[0].session, "sess-2");

        resolve_manual_in(&dir, &pending[0].id, true);
        assert!(
            asker.await.unwrap().is_approved(),
            "an approved request runs"
        );

        assert!(
            pending_requests_in(&dir).is_empty(),
            "a resolved request leaves the queue"
        );
        let audits = audit_lines(&dir);
        let record = audits
            .iter()
            .find(|r| r.id == pending[0].id)
            .expect("the manual decision is in the journal");
        assert_eq!(record.by, "manual");
        assert_eq!(record.outcome, APPROVED);
        assert_eq!(record.session, "sess-2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The deny path, end to end: a denied request makes the asker fail.
    #[tokio::test]
    async fn a_denied_answer_fails_closed() {
        let dir = fresh_dir("denied");
        let dir_for_asker = dir.clone();
        let asker = tokio::spawn(async move {
            request_approval_in(
                &dir_for_asker,
                "s".into(),
                "mkfs /dev/sda".into(),
                &verdict(&["matches: mkfs"]),
                std::time::Duration::from_secs(5),
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let pending = pending_requests_in(&dir);
        assert_eq!(pending.len(), 1);
        resolve_manual_in(&dir, &pending[0].id, false);
        let outcome = asker.await.unwrap();
        assert!(!outcome.is_approved(), "a denied command must not run");
        assert_eq!(
            outcome,
            ApprovalOutcome::DeniedByUser,
            "the asker must be able to tell a user's no from a timer's"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A request past its own deadline is finalized by the scan, not shown —
    /// and the terminal result remains for its requester to consume.
    #[tokio::test]
    async fn expired_requests_are_pruned_and_audited() {
        let dir = fresh_dir("expired");
        let stale = ApprovalRequest {
            id: "stale-1".into(),
            session: "s".into(),
            command: "c".into(),
            reasons: vec![],
            created_at_ms: 1000,
            wait_timeout_secs: 120,
            status: PENDING.to_string(),
            decided_by: String::new(),
            path: None,
        };
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("stale-1.json"),
            serde_json::to_string(&stale).unwrap(),
        )
        .unwrap();

        assert!(pending_requests_in(&dir).is_empty(), "stale is finalized");
        assert_eq!(request_state_in(&dir, "stale-1").unwrap().status, DENIED);
        // Running the scan again must not double-record.
        assert!(pending_requests_in(&dir).is_empty());
        let audits = audit_lines(&dir);
        assert_eq!(audits.len(), 1, "the audit is idempotent per request");
        assert_eq!(audits[0].by, "auto-timeout");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A queue at its cap refuses the next request without writing a file.
    #[tokio::test]
    async fn a_full_queue_denies_new_requests_immediately() {
        let dir = fresh_dir("cap");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..MAX_PENDING {
            let req = ApprovalRequest {
                id: format!("filler-{i}"),
                session: "s".into(),
                command: "c".into(),
                reasons: vec![],
                created_at_ms: now_ms_of(),
                wait_timeout_secs: 120,
                status: PENDING.to_string(),
                decided_by: String::new(),
                path: None,
            };
            std::fs::write(
                dir.join(format!("filler-{i}.json")),
                serde_json::to_string(&req).unwrap(),
            )
            .unwrap();
        }

        let refused = request_approval_in(
            &dir,
            "one-too-many".into(),
            "rm -rf /".into(),
            &verdict(&["rm -rf"]),
            std::time::Duration::from_millis(100),
        )
        .await;
        assert_eq!(
            refused,
            ApprovalOutcome::DeniedByLimit,
            "the queue is full; the answer is no"
        );
        assert_eq!(
            pending_requests_in(&dir).len(),
            MAX_PENDING,
            "the new request wrote no file"
        );
        let audits = audit_lines(&dir);
        assert!(audits.iter().any(|r| r.by == "auto-limit"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
