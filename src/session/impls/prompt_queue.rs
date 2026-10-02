//! The three prompts a connection blocks on, and the decisions around them.
//!
//! A session meeting an unknown host, a missing credential or an MFA challenge stops
//! and waits on a one-shot channel. Whoever answers decides whether the connection
//! proceeds, so the rules in this module are security rules rather than presentation:
//! whether a host stays trusted after one confirmation, whether a dismissal is
//! remembered, whether two channels racing to authenticate ask once or twice.
//!
//! They live here — framework-neutral, in `crate::session` beside [`ConnectCtx`] —
//! because the alternative is each frontend deciding for itself, and two answers to
//! "has this host been confirmed" is the one place a wrong answer is a
//! man-in-the-middle. A frontend supplies the dialog; it must not supply the policy.
//!
//! This was `crate::app`'s private state until the session layer needed it. It was
//! moved rather than copied, and the tests below came with it.
//!
//! [`ConnectCtx`]: super::ConnectCtx

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use crate::session::protocol::{
    CredentialReply, CredentialResponder, HostKeyResponder, MfaResponder,
};

use super::prompts::{PendingCred, PendingHostKey, PendingMfa};

thread_local! {
    /// Host-key prompts awaiting a decision; the front one for a window is shown.
    ///
    /// Prompts live on the UI thread — every access is from there — so a
    /// thread-local is accurate rather than a shortcut.
    static HOSTKEY_QUEUE: RefCell<VecDeque<PendingHostKey>> = RefCell::new(VecDeque::new());
    /// [`decided_id`] → accepted, remembered for this run so a second connection
    /// presenting the same key at the same address is answered without a second
    /// dialog.
    ///
    /// Two deliberate limits, both of them the difference between a convenience and
    /// a hole:
    ///
    /// - The identity is the address **and the key**, never the address alone. A
    ///   decision about one key must not vouch for another, which is what an entry
    ///   keyed on `host:port` did.
    /// - A **changed** key is never stored here at all. That prompt is the client's
    ///   only man-in-the-middle warning, and it has to be given every time — see
    ///   [`enqueue_host_key`].
    ///
    /// Only accepts are recorded, and that asymmetry is deliberate: recording a
    /// reject meant one accidental dismissal poisoned the host for the whole run,
    /// auto-rejecting every later connection with "Unknown server key" until the app
    /// was restarted (#152). A rejection now fails only the attempt that met it.
    static HOSTKEY_DECIDED: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());

    /// Credential prompts awaiting an answer.
    static CRED_QUEUE: RefCell<VecDeque<PendingCred>> = RefCell::new(VecDeque::new());
    /// session id → the answer given this run, so a second connection for the same
    /// session is answered without re-prompting.
    static CRED_DECIDED: RefCell<HashMap<String, Option<CredentialReply>>> =
        RefCell::new(HashMap::new());

    /// MFA prompts awaiting an answer.
    ///
    /// No "decided" map, deliberately: a one-time code that was wrong must re-prompt
    /// on reconnect rather than be replayed, so caching an answer here would make a
    /// retry silently reuse it.
    static MFA_QUEUE: RefCell<VecDeque<PendingMfa>> = RefCell::new(VecDeque::new());
}

/// What happened to a prompt that was just enqueued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PromptOutcome {
    /// Answered on the spot: this run has already decided this one.
    AlreadyDecided,
    /// Joined a prompt already on screen for this window, or queued behind one.
    /// Either way the caller shows nothing new.
    Showing,
    /// Queued and it is this window's turn. The caller should open a dialog.
    Show,
}

impl PromptOutcome {
    /// Whether the caller should open a dialog for this prompt.
    pub(crate) fn should_show(self) -> bool {
        matches!(self, Self::Show)
    }
}

// ---------------------------------------------------------------------------
// Host key
// ---------------------------------------------------------------------------

/// What a dialog needs to render a host-key prompt.
///
/// A copy rather than the queued entry, because that holds the responders and a view
/// has no business with them: the only way to answer is [`resolve_host_key`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HostKeyPrompt {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) changed: bool,
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) detail: String,
    pub(crate) confirm_label: String,
}

/// The words a host-key dialog shows.
///
/// Moved here from `crate::app::auth_dialogs` so both frontends show the same text:
/// the wording is part of what the user is being asked to decide, and two frontends
/// with two phrasings for "this key changed, this could be an attack" would be two
/// different warnings about the same fact.
///
/// Returns `(title, message, detail, confirm_label)`. The detail is the part worth
/// comparing character by character against `ssh-keyscan`, so it puts the key type
/// beside the address and the fingerprint on its own line.
pub(crate) fn hostkey_text(
    host: &str,
    port: u16,
    key_type: &str,
    fingerprint: &str,
    changed: bool,
) -> (String, String, String, String) {
    let detail = format!("{host}:{port}  ({key_type})\n{fingerprint}");
    if changed {
        (
            crate::i18n::t("主机密钥已改变", "Host key changed").to_string(),
            crate::i18n::t(
                "该主机的密钥与之前记录的不一致,可能存在中间人攻击。仅当你确知服务器密钥已更换时才继续。",
                "This host's key differs from the one stored earlier — this could be a man-in-the-middle attack. Only continue if you know the server's key really changed.",
            )
            .to_string(),
            detail,
            crate::i18n::t("仍然信任", "Trust anyway").to_string(),
        )
    } else {
        (
            crate::i18n::t("未知主机", "Unknown host").to_string(),
            crate::i18n::t(
                "首次连接该主机。请核对下面的密钥指纹,确认无误后再信任并连接。",
                "First time connecting to this host. Verify the key fingerprint below before you trust and connect.",
            )
            .to_string(),
            detail,
            crate::i18n::t("信任并连接", "Trust & connect").to_string(),
        )
    }
}

/// The identity a remembered host-key decision is stored under.
///
/// The question a prompt asks is about a *key*, not about an address, and the same
/// address can present a second one — that is precisely what a changed-key prompt
/// means. Folding the key type and fingerprint into the identity is what stops a
/// decision made about one key from being replayed as the answer about another.
fn decided_id(host: &str, port: u16, key_type: &str, fingerprint: &str) -> String {
    format!("{host}:{port}:{key_type}:{fingerprint}")
}

/// Queue a host-key prompt.
///
/// Four rules, each with a reason:
///
/// - A **changed** key is never answered from memory. That prompt exists because
///   the key is not the one this host was trusted with, it is the only
///   man-in-the-middle warning the client has, and it therefore has to reach the
///   user every single time. Answering it from a decision recorded earlier in the
///   run silenced the warning and let the new key be written to `known_hosts`.
/// - A key this run already accepted is answered immediately, so a second
///   connection does not ask what the user just answered. The memory is keyed on
///   the address *and* the key (see [`decided_id`]), so accepting one key never
///   vouches for another.
/// - A second prompt for the same address **and the same key** in the same window
///   **merges** into the open one. The shell and its SFTP channel authenticate
///   concurrently and both meet the unknown host; asking twice would make one
///   decision look like two. A prompt about a *different* key stays a dialog of its
///   own, because merging those would answer for a key the user never saw.
/// - Otherwise it queues, and only this window's oldest entry is shown — so a second
///   prompt waits its turn rather than stacking two dialogs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn enqueue_host_key(
    window_id: u64,
    host: String,
    port: u16,
    key_type: String,
    fingerprint: String,
    changed: bool,
    responder: HostKeyResponder,
) -> PromptOutcome {
    let id = decided_id(&host, port, &key_type, &fingerprint);
    // A changed key is not even looked up: only a re-presentation of a key this run
    // already accepted may be answered from memory.
    let remembered = if changed {
        None
    } else {
        HOSTKEY_DECIDED.with(|d| d.borrow().get(&id).copied())
    };
    if let Some(accepted) = remembered {
        responder.respond(accepted);
        return PromptOutcome::AlreadyDecided;
    }

    let show_now = HOSTKEY_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if let Some(pending) = q.iter_mut().find(|p| {
            p.window_id == window_id
                && p.host == host
                && p.port == port
                && p.key_type == key_type
                && p.fingerprint == fingerprint
        }) {
            pending.responders.push(responder);
            return false;
        }
        let show_now = !q.iter().any(|p| p.window_id == window_id);
        let (title, message, detail, confirm_label) =
            hostkey_text(&host, port, &key_type, &fingerprint, changed);
        q.push_back(PendingHostKey {
            window_id,
            host,
            port,
            key_type,
            fingerprint,
            changed,
            title,
            message,
            detail,
            confirm_label,
            responders: vec![responder],
        });
        show_now
    });

    if show_now {
        PromptOutcome::Show
    } else {
        PromptOutcome::Showing
    }
}

/// Answer the window's oldest host-key prompt.
///
/// Returns whether the window has another waiting, which is the caller's cue to show
/// the next dialog rather than closing the prompt UI entirely.
pub(crate) fn resolve_host_key(window_id: u64, accept: bool) -> bool {
    HOSTKEY_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if let Some(pos) = q.iter().position(|p| p.window_id == window_id) {
            let pending = q.remove(pos).expect("position checked above");
            // Remembered under the exact key it was decided about, so a later
            // connection presenting a different key at the same address still has to
            // ask. A changed key is not remembered at all: it is the one prompt whose
            // whole purpose is to be shown again, every time.
            if accept && !pending.changed {
                let id = decided_id(
                    &pending.host,
                    pending.port,
                    &pending.key_type,
                    &pending.fingerprint,
                );
                HOSTKEY_DECIDED.with(|d| {
                    d.borrow_mut().insert(id, true);
                });
            }
            for responder in &pending.responders {
                responder.respond(accept);
            }
        }
        q.iter().any(|p| p.window_id == window_id)
    })
}

/// The window's queued host-key prompt, if it has one.
///
/// A view needs this to repaint a dialog it already opened; the caller that just got
/// [`PromptOutcome::Show`] can also use it to read the text rather than having it
/// passed back.
pub(crate) fn host_key_prompt(window_id: u64) -> Option<HostKeyPrompt> {
    HOSTKEY_QUEUE.with(|q| {
        q.borrow()
            .iter()
            .find(|p| p.window_id == window_id)
            .map(|p| HostKeyPrompt {
                host: p.host.clone(),
                port: p.port,
                changed: p.changed,
                title: p.title.clone(),
                message: p.message.clone(),
                detail: p.detail.clone(),
                confirm_label: p.confirm_label.clone(),
            })
    })
}

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

/// What a dialog needs to render a credential prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CredentialPrompt {
    pub(crate) session_id: String,
    pub(crate) host: String,
    pub(crate) user: String,
    pub(crate) need_user: bool,
    pub(crate) need_password: bool,
}

/// Queue a credential prompt.
///
/// A prompt for the same session in the same window merges, for the same reason the
/// host-key one does: the shell and its SFTP channel reach this together.
pub(crate) fn enqueue_credential(
    window_id: u64,
    session_id: String,
    host: String,
    user: String,
    need_user: bool,
    need_password: bool,
    responder: CredentialResponder,
) -> PromptOutcome {
    if let Some(reply) = CRED_DECIDED.with(|d| d.borrow().get(&session_id).cloned()) {
        responder.respond(reply);
        return PromptOutcome::AlreadyDecided;
    }

    let show_now = CRED_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if let Some(pending) = q
            .iter_mut()
            .find(|p| p.window_id == window_id && p.session_id == session_id)
        {
            pending.responders.push(responder);
            return false;
        }
        let show_now = !q.iter().any(|p| p.window_id == window_id);
        q.push_back(PendingCred {
            window_id,
            session_id,
            host,
            user,
            need_user,
            need_password,
            responders: vec![responder],
        });
        show_now
    });

    if show_now {
        PromptOutcome::Show
    } else {
        PromptOutcome::Showing
    }
}

/// Answer the window's oldest credential prompt.
///
/// Unlike the host-key case a cancellation *is* remembered, because cancelling is an
/// explicit act rather than a possible misclick: a user who declined should not be
/// asked again for that session in the same run.
///
/// The caller persists credentials when the user asked to remember them; this module
/// holds no store and does no I/O.
pub(crate) fn resolve_credential(window_id: u64, reply: Option<CredentialReply>) -> bool {
    CRED_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if let Some(pos) = q.iter().position(|p| p.window_id == window_id) {
            let pending = q.remove(pos).expect("position checked above");
            CRED_DECIDED.with(|d| {
                d.borrow_mut()
                    .insert(pending.session_id.clone(), reply.clone());
            });
            for responder in &pending.responders {
                responder.respond(reply.clone());
            }
        }
        q.iter().any(|p| p.window_id == window_id)
    })
}

/// The window's queued credential prompt, if it has one.
pub(crate) fn credential_prompt(window_id: u64) -> Option<CredentialPrompt> {
    CRED_QUEUE.with(|q| {
        q.borrow()
            .iter()
            .find(|p| p.window_id == window_id)
            .map(|p| CredentialPrompt {
                session_id: p.session_id.clone(),
                host: p.host.clone(),
                user: p.user.clone(),
                need_user: p.need_user,
                need_password: p.need_password,
            })
    })
}

// ---------------------------------------------------------------------------
// MFA
// ---------------------------------------------------------------------------

/// What a dialog needs to render an MFA prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MfaPrompt {
    pub(crate) host: String,
    pub(crate) prompt: String,
    /// Whether the answer should be visible. An OTP is echoed by convention even
    /// though it is a secret, because the user has to read it back; a
    /// password-shaped challenge is not.
    pub(crate) echo: bool,
}

/// Queue an MFA prompt.
pub(crate) fn enqueue_mfa(
    window_id: u64,
    session_id: String,
    host: String,
    prompt: String,
    echo: bool,
    responder: MfaResponder,
) -> PromptOutcome {
    let show_now = MFA_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if let Some(pending) = q
            .iter_mut()
            .find(|p| p.window_id == window_id && p.session_id == session_id)
        {
            pending.responders.push(responder);
            return false;
        }
        let show_now = !q.iter().any(|p| p.window_id == window_id);
        q.push_back(PendingMfa {
            window_id,
            session_id,
            host,
            prompt,
            echo,
            responders: vec![responder],
        });
        show_now
    });

    if show_now {
        PromptOutcome::Show
    } else {
        PromptOutcome::Showing
    }
}

/// Answer the window's oldest MFA prompt.
pub(crate) fn resolve_mfa(window_id: u64, answer: Option<String>) -> bool {
    MFA_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if let Some(pos) = q.iter().position(|p| p.window_id == window_id) {
            let pending = q.remove(pos).expect("position checked above");
            for responder in &pending.responders {
                responder.respond(answer.clone());
            }
        }
        q.iter().any(|p| p.window_id == window_id)
    })
}

/// The window's queued MFA prompt, if it has one.
pub(crate) fn mfa_prompt(window_id: u64) -> Option<MfaPrompt> {
    MFA_QUEUE.with(|q| {
        q.borrow()
            .iter()
            .find(|p| p.window_id == window_id)
            .map(|p| MfaPrompt {
                host: p.host.clone(),
                prompt: p.prompt.clone(),
                echo: p.echo,
            })
    })
}

// ---------------------------------------------------------------------------
// Window teardown
// ---------------------------------------------------------------------------

/// Abort every prompt owned by `window_id`, answering each so a blocked auth flow
/// fails instead of waiting forever on a dialog nobody will show.
///
/// Called when a window closes. Without it the queues would hold responders whose
/// auth flows are still parked on a one-shot channel with no one left to send to.
/// Drop the remembered interactive answer for one session — the answer holds a
/// password, and once the session's task is done nothing can ask for it again.
/// Called on session end so the cache lives for reconnects within a run, not
/// for the whole process lifetime (audit M-19).
pub(crate) fn forget_credentials(session_id: &str) {
    CRED_DECIDED.with(|d| {
        d.borrow_mut().remove(session_id);
    });
}

pub(crate) fn abort_window(window_id: u64) {
    HOSTKEY_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let mut i = 0;
        while i < q.len() {
            if q[i].window_id == window_id {
                let pending = q.remove(i).expect("index checked above");
                for responder in &pending.responders {
                    responder.respond(false);
                }
            } else {
                i += 1;
            }
        }
    });
    CRED_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let mut i = 0;
        while i < q.len() {
            if q[i].window_id == window_id {
                let pending = q.remove(i).expect("index checked above");
                for responder in &pending.responders {
                    responder.respond(None);
                }
            } else {
                i += 1;
            }
        }
    });
    MFA_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let mut i = 0;
        while i < q.len() {
            if q[i].window_id == window_id {
                let pending = q.remove(i).expect("index checked above");
                for responder in &pending.responders {
                    responder.respond(None);
                }
            } else {
                i += 1;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    fn host_key(host: &str, window_id: u64) -> (PromptOutcome, oneshot::Receiver<bool>) {
        let (tx, rx) = oneshot::channel();
        let outcome = enqueue_host_key(
            window_id,
            host.to_string(),
            22,
            "ssh-ed25519".to_string(),
            "SHA256:abc".to_string(),
            false,
            HostKeyResponder::new(tx),
        );
        (outcome, rx)
    }

    /// Answer everything this window has queued, so one test cannot leave a prompt
    /// behind for the next one.
    fn drain(window_id: u64) {
        while resolve_host_key(window_id, false) {}
        while resolve_credential(window_id, None) {}
        while resolve_mfa(window_id, None) {}
    }

    #[test]
    fn a_second_prompt_for_the_same_host_merges_and_answers_both() {
        // The shell and its SFTP channel meet the unknown host together. One dialog,
        // one decision — and an unanswered merged responder is a connection that
        // hangs forever, which is why both are asserted.
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let first = enqueue_host_key(
            101,
            "merge.example".into(),
            22,
            "ssh-ed25519".into(),
            "SHA256:abc".into(),
            false,
            HostKeyResponder::new(tx1),
        );
        assert!(first.should_show());

        let second = enqueue_host_key(
            101,
            "merge.example".into(),
            22,
            "ssh-ed25519".into(),
            "SHA256:abc".into(),
            false,
            HostKeyResponder::new(tx2),
        );
        assert_eq!(second, PromptOutcome::Showing, "no second dialog");

        assert!(!resolve_host_key(101, true), "nothing left queued");
        assert!(rx1.blocking_recv().unwrap());
        assert!(rx2.blocking_recv().unwrap(), "both responders answered");
    }

    #[test]
    fn an_accepted_host_is_answered_without_being_asked_again() {
        let (outcome, rx) = host_key("known.example", 102);
        assert!(outcome.should_show());
        resolve_host_key(102, true);
        assert!(rx.blocking_recv().unwrap());

        let (tx2, rx2) = oneshot::channel();
        let again = enqueue_host_key(
            102,
            "known.example".into(),
            22,
            "ssh-ed25519".into(),
            "SHA256:abc".into(),
            false,
            HostKeyResponder::new(tx2),
        );
        assert_eq!(again, PromptOutcome::AlreadyDecided);
        assert!(rx2.blocking_recv().unwrap(), "answered from the memory");
    }

    #[test]
    fn an_accepted_key_does_not_vouch_for_another_key_at_the_same_address() {
        // The memory is an answer about a key, not about an address. A second key at
        // an address the user has already accepted is a new question, however
        // conveniently the address matches.
        let (outcome, rx) = host_key("swap.example", 112);
        assert!(outcome.should_show());
        resolve_host_key(112, true);
        assert!(rx.blocking_recv().unwrap());

        let (tx2, rx2) = oneshot::channel();
        let other = enqueue_host_key(
            112,
            "swap.example".into(),
            22,
            "ssh-ed25519".into(),
            "SHA256:not-the-same-key".into(),
            false,
            HostKeyResponder::new(tx2),
        );
        assert!(
            other.should_show(),
            "a different key at the same address must be asked about"
        );
        drain(112);
        assert!(!rx2.blocking_recv().unwrap(), "drained as a rejection");
    }

    #[test]
    fn a_changed_key_asks_every_time_even_after_this_run_accepted_the_host() {
        // H-01. The changed-key prompt is the client's only man-in-the-middle
        // warning, and it used to be answered from memory for the rest of the run as
        // soon as the same host:port had been accepted once — which both silenced
        // the warning and let `verify_host_key` write the new key to known_hosts.
        // It has to be shown however often it arrives.
        let (tx1, rx1) = oneshot::channel();
        let unknown = enqueue_host_key(
            113,
            "changed.example".into(),
            22,
            "ssh-ed25519".into(),
            "SHA256:first".into(),
            false,
            HostKeyResponder::new(tx1),
        );
        assert!(unknown.should_show());
        resolve_host_key(113, true);
        assert!(rx1.blocking_recv().unwrap());

        let (tx2, rx2) = oneshot::channel();
        let changed = enqueue_host_key(
            113,
            "changed.example".into(),
            22,
            "ssh-ed25519".into(),
            "SHA256:attacker".into(),
            true,
            HostKeyResponder::new(tx2),
        );
        assert!(
            changed.should_show(),
            "a changed key must never be answered from memory"
        );
        assert!(
            host_key_prompt(113)
                .map(|p| p.detail.contains("SHA256:attacker"))
                .unwrap_or(false),
            "and the dialog must show the key that is actually being offered"
        );
        resolve_host_key(113, false);
        assert!(!rx2.blocking_recv().unwrap());

        // Accepting a changed key must not license a silent third one either: the
        // cache stays empty for it.
        let (tx3, rx3) = oneshot::channel();
        let again = enqueue_host_key(
            113,
            "changed.example".into(),
            22,
            "ssh-ed25519".into(),
            "SHA256:attacker".into(),
            true,
            HostKeyResponder::new(tx3),
        );
        assert!(again.should_show(), "still asked, not replayed");
        resolve_host_key(113, true);
        assert!(rx3.blocking_recv().unwrap());

        let (tx4, rx4) = oneshot::channel();
        let fourth = enqueue_host_key(
            113,
            "changed.example".into(),
            22,
            "ssh-ed25519".into(),
            "SHA256:attacker".into(),
            true,
            HostKeyResponder::new(tx4),
        );
        assert!(
            fourth.should_show(),
            "accepting a changed key is not a standing licence for that key"
        );
        drain(113);
        assert!(
            !rx4.blocking_recv().unwrap(),
            "and it was answered, not left hanging"
        );
    }

    #[test]
    fn a_rejected_host_is_asked_again_rather_than_remembered() {
        // The #152 regression: remembering a reject meant one accidental dismissal
        // auto-rejected every later connection for the whole run.
        let (outcome, rx) = host_key("rejected.example", 103);
        assert!(outcome.should_show());
        resolve_host_key(103, false);
        assert!(!rx.blocking_recv().unwrap());

        let (tx2, _rx2) = oneshot::channel();
        let again = enqueue_host_key(
            103,
            "rejected.example".into(),
            22,
            "ssh-ed25519".into(),
            "SHA256:abc".into(),
            false,
            HostKeyResponder::new(tx2),
        );
        assert!(
            again.should_show(),
            "a rejection must not be remembered, so this asks again"
        );
        drain(103);
    }

    #[test]
    fn a_second_prompt_in_the_same_window_waits_its_turn() {
        // One dialog per window at a time, because two dialogs stacked over each other
        // means the user answers whichever happens to be on top and never sees the
        // other. Two *different* windows each get their own — that is the point of the
        // rule being per-window rather than process-wide.
        let (first, _rx1) = host_key("first.example", 111);
        assert!(first.should_show(), "the window's first prompt shows");

        let (second, _rx2) = host_key("second.example", 111);
        assert_eq!(
            second,
            PromptOutcome::Showing,
            "a second prompt in the same window waits rather than stacking"
        );
        assert_eq!(
            host_key_prompt(111).map(|p| p.host),
            Some("first.example".to_string()),
            "the first one is still the one on screen"
        );

        // Answering the first promotes the second, which is how the queued prompt gets
        // its dialog without the caller tracking a queue of its own.
        assert!(resolve_host_key(111, false), "the second is still waiting");
        assert_eq!(
            host_key_prompt(111).map(|p| p.host),
            Some("second.example".to_string()),
            "the second is now the front"
        );
        drain(111);
    }

    #[test]
    fn a_cancelled_credential_is_remembered_but_mfa_never_is() {
        // The asymmetry is the design: cancelling credentials is an explicit act, so
        // asking again in the same run is noise. A wrong MFA code must be asked again,
        // because replaying it would silently reuse a code the server rejected.
        let (ctx, crx) = oneshot::channel();
        assert!(enqueue_credential(
            106,
            "sess".into(),
            "h".into(),
            "u".into(),
            true,
            true,
            CredentialResponder::new(ctx),
        )
        .should_show());
        resolve_credential(106, None);
        assert!(crx.blocking_recv().unwrap().is_none());

        let (ctx2, crx2) = oneshot::channel();
        let again = enqueue_credential(
            106,
            "sess".into(),
            "h".into(),
            "u".into(),
            true,
            true,
            CredentialResponder::new(ctx2),
        );
        assert_eq!(
            again,
            PromptOutcome::AlreadyDecided,
            "the cancellation is the answer for this run"
        );
        assert!(crx2.blocking_recv().unwrap().is_none());

        let (mtx, mrx) = oneshot::channel();
        assert!(enqueue_mfa(
            106,
            "sess".into(),
            "h".into(),
            "code?".into(),
            true,
            MfaResponder::new(mtx),
        )
        .should_show());
        resolve_mfa(106, Some("123456".into()));
        assert_eq!(mrx.blocking_recv().unwrap().as_deref(), Some("123456"));

        let (mtx2, mrx2) = oneshot::channel();
        let asked_again = enqueue_mfa(
            106,
            "sess".into(),
            "h".into(),
            "code?".into(),
            true,
            MfaResponder::new(mtx2),
        );
        assert!(
            asked_again.should_show(),
            "an MFA answer is never replayed, so this prompts again"
        );
        resolve_mfa(106, None);
        assert!(mrx2.blocking_recv().unwrap().is_none());
    }

    #[test]
    fn aborting_a_window_answers_everything_it_owns() {
        // A window closing must not leave three auth flows parked with nobody left to
        // answer them.
        let (htx, hrx) = oneshot::channel();
        enqueue_host_key(
            107,
            "closing.example".into(),
            22,
            "ssh-ed25519".into(),
            "SHA256:abc".into(),
            false,
            HostKeyResponder::new(htx),
        );
        let (ctx, crx) = oneshot::channel();
        enqueue_credential(
            107,
            "closing".into(),
            "h".into(),
            "u".into(),
            true,
            true,
            CredentialResponder::new(ctx),
        );
        let (mtx, mrx) = oneshot::channel();
        enqueue_mfa(
            107,
            "closing".into(),
            "h".into(),
            "code?".into(),
            true,
            MfaResponder::new(mtx),
        );

        abort_window(107);

        assert!(!hrx.blocking_recv().unwrap(), "host key rejected on abort");
        assert!(
            crx.blocking_recv().unwrap().is_none(),
            "credential cancelled"
        );
        assert!(mrx.blocking_recv().unwrap().is_none(), "MFA cancelled");
        assert!(host_key_prompt(107).is_none(), "nothing left queued");
    }

    #[test]
    fn a_prompt_carries_the_text_its_dialog_shows() {
        // The view reads this instead of being handed the strings back, so a dialog
        // reopened for a queued prompt shows the same thing the first one did.
        let (tx, _rx) = oneshot::channel();
        assert!(enqueue_host_key(
            108,
            "text.example".into(),
            2222,
            "ssh-rsa".into(),
            "SHA256:xyz".into(),
            true,
            HostKeyResponder::new(tx),
        )
        .should_show());

        let prompt = host_key_prompt(108).expect("queued");
        assert_eq!(prompt.host, "text.example");
        assert_eq!(prompt.port, 2222);
        assert!(
            prompt.changed,
            "a changed key is what the danger wording is for"
        );
        assert!(!prompt.confirm_label.is_empty());
        assert!(prompt.detail.contains("text.example"));
        drain(108);
    }
}
