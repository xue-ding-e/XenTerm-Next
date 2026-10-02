//! The human-approval queue for risky automation commands.
//!
//! The MCP server is its own stdio process with no window, and the person who
//! can approve a risky command is sitting at the main XenTerm window — a
//! different process, possibly not running at all. The bridge is a directory
//! under the data dir: a risky request lands there as a JSON file in state
//! `pending`, the main window's poll finds it and asks the human, and the
//! answer is written back into the same file in place. The MCP side polls its
//! one file until the status moves or its deadline passes.
//!
//! Fail-closed on every path: no window running, an expired request, a
//! timeout, a file that will not parse — all of it resolves to "not
//! approved", and the command does not run.
//!
//! Every finished request — approved or denied, by a human or by a timer —
//! lands in an append-only audit journal, one JSONL file per day. The files
//! are never edited or deleted one by one; a retention sweep drops whole days
//! past the configured age, and that is the only deletion there is.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::risk::RiskVerdict;

/// Where the request files live; the audit journal lives in `audit/` beneath.
pub(crate) fn queue_dir() -> PathBuf {
    crate::config::data_dir().join("mcp-approvals")
}

/// How long a pending request stays answerable before every reader must treat
/// it as denied. The MCP caller waits its own timeout plus a grace margin —
/// the *window* is the one that times an unanswered request out, so the two
/// sides must not race to delete the file first.
const REQUEST_TTL: std::time::Duration = std::time::Duration::from_secs(300);

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

fn is_expired(request: &ApprovalRequest, now: std::time::SystemTime) -> bool {
    let created = std::time::UNIX_EPOCH + std::time::Duration::from_millis(request.created_at_ms);
    now.duration_since(created)
        .map(|age| age > REQUEST_TTL)
        .unwrap_or(true)
}

/// Seconds left on a request's decision deadline, for the deny button's
/// countdown. Never negative.
pub(crate) fn seconds_left(request: &ApprovalRequest) -> u64 {
    let created = std::time::UNIX_EPOCH + std::time::Duration::from_millis(request.created_at_ms);
    std::time::SystemTime::now()
        .duration_since(created)
        .map(|age| request.wait_timeout_secs.saturating_sub(age.as_secs()))
        .unwrap_or(0)
}

/// Write a request file without following a symlink planted at the path, and
/// refuse to create one if the name has been replaced by a directory. The
/// queue directory is inside the user's data tree; a local process that can
/// plant a symlink there should not get the approval protocol to overwrite
/// some other file through it.
fn write_request_file(path: &Path, body: &str) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        // A planted symlink (or anything not a regular file) is removed and
        // rewritten as a fresh regular file.
        Ok(meta) if !meta.is_file() => std::fs::remove_file(path)?,
        Ok(_) => {}
        Err(_) => {}
    }
    std::fs::write(path, body)
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
    /// "manual" | "auto-timeout" | "auto-expired" | "auto-limit"
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
fn append_audit_in(queue: &Path, record: &AuditRecord) {
    let path = audit_path_for(queue, record.decided_at_ms);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if existing.lines().any(|line| {
            serde_json::from_str::<AuditRecord>(line)
                .map(|r| r.id == record.id)
                .unwrap_or(false)
        }) {
            return;
        }
    }
    if std::fs::create_dir_all(audit_dir(queue)).is_err() {
        return;
    }
    if let Ok(mut line) = serde_json::to_string(record) {
        line.push('\n');
        use std::io::Write as _;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = file.write_all(line.as_bytes());
        }
    }
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
        append_audit_in(
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
        wait_timeout_secs: timeout.as_secs(),
        status: PENDING.to_string(),
        decided_by: String::new(),
        path: None,
    };
    let path = dir.join(format!("{}.json", request.id));
    let Ok(body) = serde_json::to_string_pretty(&request) else {
        return ApprovalOutcome::DeniedUnreachable;
    };
    if let Err(error) = write_request_file(&path, &body) {
        tracing::warn!("approval request {} could not be written: {error}", request.id);
        return ApprovalOutcome::DeniedUnreachable;
    }

    // Poll our own file: the window rewrites it in place with the answer, and
    // removes it once it has also shown its feedback — the waiter's deadline
    // carries the margin that makes that ordering safe.
    let begun = std::time::Instant::now();
    while begun.elapsed() < timeout + WAITER_MARGIN {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        match std::fs::read_to_string(&path)
            .ok()
            .and_then(|body| serde_json::from_str::<ApprovalRequest>(&body).ok())
        {
            Some(current) if current.id == request.id => match current.status.as_str() {
                APPROVED => {
                    let _ = std::fs::remove_file(&path);
                    return ApprovalOutcome::Approved;
                }
                DENIED => {
                    let by = if current.decided_by == "manual" {
                        ApprovalOutcome::DeniedByUser
                    } else {
                        // The window's own deadline answered first.
                        ApprovalOutcome::DeniedByTimeout
                    };
                    let _ = std::fs::remove_file(&path);
                    return by;
                }
                _ if is_expired(&current, std::time::SystemTime::now()) => {
                    let _ = std::fs::remove_file(&path);
                    return ApprovalOutcome::DeniedByTimeout;
                }
                _ => {}
            },
            // Our file vanished (the window pruned it as stale): denied.
            _ => return ApprovalOutcome::DeniedUnreachable,
        }
    }
    // Deadline passed with no answer — the window is gone or the human never
    // came. Remove the file and record why the command did not run.
    let _ = std::fs::remove_file(&path);
    append_audit_in(
        dir,
        &AuditRecord {
            id: request.id,
            decided_at_ms: now_ms_of(),
            session: request.session,
            command: request.command,
            reasons: verdict.reasons.clone(),
            outcome: DENIED.to_string(),
            by: "auto-timeout".to_string(),
        },
    );
    ApprovalOutcome::DeniedByTimeout
}

/// The real queue's wrapper — what the automation layer calls.
pub(crate) async fn request_approval(
    session: String,
    command: String,
    verdict: &RiskVerdict,
    timeout: std::time::Duration,
) -> ApprovalOutcome {
    request_approval_in(&queue_dir(), session, command, verdict, timeout).await
}

/// Every live pending request in `dir`, oldest first — what the window's poll
/// shows. Expired pendings are pruned here (and audited as auto-expired)
/// rather than shown.
pub(crate) fn pending_requests_in(dir: &Path) -> Vec<ApprovalRequest> {
    let now = std::time::SystemTime::now();
    scan_in(dir)
        .into_iter()
        .filter(|request| {
            let expired = is_expired(request, now);
            if expired {
                if let Some(path) = &request.path {
                    let _ = std::fs::remove_file(path);
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
                        by: "auto-expired".to_string(),
                    },
                );
            }
            !expired
        })
        .collect()
}

/// Every request file in `dir` regardless of status — what the window's
/// feedback pass reads: a file that has left `pending` is a decision the
/// human has made or a timeout the window itself recorded, and the asking
/// process is waiting for that fact to be seen.
pub(crate) fn scan_in(dir: &Path) -> Vec<ApprovalRequest> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut requests: Vec<ApprovalRequest> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|path| {
            let body = std::fs::read_to_string(&path).ok()?;
            let mut request: ApprovalRequest = serde_json::from_str(&body).ok()?;
            request.path = Some(path);
            Some(request)
        })
        .collect();
    requests.sort_by_key(|request| request.created_at_ms);
    requests
}

/// Write the human's answer into the request file and audit it as a manual
/// decision. Called from the dialog's buttons.
pub(crate) fn resolve_manual(id: &str, approved: bool) {
    resolve_manual_in(&queue_dir(), id, approved)
}

pub(crate) fn resolve_manual_in(dir: &Path, id: &str, approved: bool) {
    let path = dir.join(format!("{id}.json"));
    if symlinked(&path) {
        return;
    }
    let Ok(body) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut request) = serde_json::from_str::<ApprovalRequest>(&body) else {
        return;
    };
    let outcome = if approved { APPROVED } else { DENIED };
    request.status = outcome.to_string();
    request.decided_by = "manual".to_string();
    if let Ok(resolved) = serde_json::to_string_pretty(&request) {
        let _ = std::fs::write(&path, resolved);
    }
    append_audit_in(
        &dir,
        &AuditRecord {
            id: request.id,
            decided_at_ms: now_ms_of(),
            session: request.session,
            command: request.command,
            reasons: request.reasons,
            outcome: outcome.to_string(),
            by: "manual".to_string(),
        },
    )
}

/// The window timing an unanswered request out: deny in place and audit it
/// as an automatic decision. `id`'s file may already be gone — that means
/// the asking side gave up first, and there is nothing to answer.
pub(crate) fn resolve_timeout_in(dir: &Path, id: &str) {
    let path = dir.join(format!("{id}.json"));
    if symlinked(&path) {
        return;
    }
    let Ok(body) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(request) = serde_json::from_str::<ApprovalRequest>(&body) else {
        return;
    };
    append_audit_in(
        dir,
        &AuditRecord {
            id: request.id,
            decided_at_ms: now_ms_of(),
            session: request.session,
            command: request.command,
            reasons: request.reasons,
            outcome: DENIED.to_string(),
            by: "auto-timeout".to_string(),
        },
    );
    let _ = std::fs::remove_file(&path);
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

    /// A request older than the TTL is pruned by the scan, not shown — and
    /// the pruning writes an auto-expired audit line, once.
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

        assert!(pending_requests_in(&dir).is_empty(), "stale is pruned");
        assert!(!dir.join("stale-1.json").exists(), "and its file is gone");
        // Running the scan again must not double-record.
        assert!(pending_requests_in(&dir).is_empty());
        let audits = audit_lines(&dir);
        assert_eq!(audits.len(), 1, "the audit is idempotent per request");
        assert_eq!(audits[0].by, "auto-expired");
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
