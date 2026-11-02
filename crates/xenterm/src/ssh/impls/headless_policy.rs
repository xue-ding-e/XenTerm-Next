//! Narrow frozen inputs for the in-process saved-forward owner. This is not a
//! platform-consistent profile loader: path-backed private keys are deliberately
//! ineligible for reuse until their parsed-key ownership is available.
use crate::config::Session;
use crate::ssh::{known_hosts::HostKeySnapshot, ProxyConfig};
use anyhow::{Context, Result};
use russh::keys::PrivateKey;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct Policy {
    trust: Arc<HostKeySnapshot>,
    // The outer Option is supplied by TASK.try_with: a captured None means
    // explicitly direct, never a request to consult ALL_PROXY again.
    proxy: Option<ProxyConfig>,
    identity: [u8; 32],
    reusable: bool,
    // A dropped authentication future cannot cancel spawn_blocking. Keep its
    // join handle with this owner's worker instead of detaching secret work.
    key_loads: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}
tokio::task_local! { static TASK: Policy; }
impl Policy {
    pub(crate) fn capture(session: &Session, jumps: &[Session]) -> Result<Self> {
        let path = crate::config::data_dir().join("known_hosts");
        let trust = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(_) => anyhow::bail!("cannot read the saved host-key trust policy"),
        };
        let outer = jumps.last().unwrap_or(session);
        let proxy = crate::ssh::proxy::resolve(&outer.proxy)?;
        let reusable = reusable_credentials(session, jumps);
        Self::from_parts(&trust, proxy, reusable)
    }
    fn from_parts(trust: &str, proxy: Option<ProxyConfig>, reusable: bool) -> Result<Self> {
        let proxy_parts = proxy.as_ref().map(|proxy| {
            (
                format!("{:?}", proxy.kind),
                &proxy.host,
                proxy.port,
                &proxy.auth,
            )
        });
        let bytes = zeroize::Zeroizing::new(
            serde_json::to_vec(&(trust, proxy_parts))
                .context("cannot identify frozen forwarding policy")?,
        );
        Ok(Self {
            trust: Arc::new(HostKeySnapshot::from_text(trust)),
            proxy,
            identity: Sha256::digest(bytes.as_slice()).into(),
            reusable,
            key_loads: Arc::new(Mutex::new(Vec::new())),
        })
    }
    pub(crate) fn identity(&self) -> [u8; 32] {
        self.identity
    }
    pub(crate) fn reusable(&self) -> bool {
        self.reusable
    }
    /// The retained owner worker must await this scope, even when the inner
    /// connection is cancelled. Its completion includes all blocking key work.
    pub(crate) async fn run<F: Future>(&self, future: F) -> Result<F::Output> {
        let output = TASK.scope(self.clone(), future).await;
        let tasks = std::mem::take(&mut *self.key_loads.lock().unwrap_or_else(|e| e.into_inner()));
        let mut confirmed = true;
        for task in tasks {
            // Join every loader, including after one has panicked.
            confirmed &= task.await.is_ok();
        }
        anyhow::ensure!(confirmed, "private-key loader task failed to drain");
        Ok(output)
    }
}

pub(crate) async fn load_private_key(
    work: impl FnOnce() -> Result<PrivateKey> + Send + 'static,
) -> Result<PrivateKey> {
    if let Ok(loads) = TASK.try_with(|policy| policy.key_loads.clone()) {
        let (result, received) = tokio::sync::oneshot::channel();
        let task = tokio::task::spawn_blocking(move || {
            // If authentication was cancelled, dropping the failed send also
            // drops the parsed key before the owned task can finish.
            let _ = result.send(work());
        });
        // No await between spawn and registration: cancellation cannot orphan
        // the loader in this hand-off to the retained owner worker.
        loads.lock().unwrap_or_else(|e| e.into_inner()).push(task);
        received.await.context("private-key loader task failed")?
    } else {
        tokio::task::spawn_blocking(work)
            .await
            .context("private-key loader task failed")?
    }
}
fn reusable_credentials(session: &Session, jumps: &[Session]) -> bool {
    std::iter::once(session).chain(jumps.iter()).all(|hop| {
        !matches!(hop.auth, crate::config::AuthMethod::Key)
            || !hop.private_key_inline.as_str().trim().is_empty()
    })
}
/// Recheck trust/effective proxy before publishing ready. The connection itself
/// already consumed the frozen policy; this never substitutes a new verifier.
pub(crate) fn still_current(session: &Session, jumps: &[Session]) -> bool {
    let Ok(current) = Policy::capture(session, jumps) else {
        return false;
    };
    TASK.try_with(|policy| policy.identity == current.identity)
        .unwrap_or(false)
}
pub(crate) fn current_trust() -> Option<Arc<HostKeySnapshot>> {
    TASK.try_with(|policy| policy.trust.clone()).ok()
}
pub(crate) fn proxy_for_connection(saved: &str) -> Result<Option<ProxyConfig>> {
    match TASK.try_with(|policy| policy.proxy.clone()) {
        Ok(captured) => Ok(captured),
        Err(_) => crate::ssh::proxy::resolve(saved),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn captured_direct_never_falls_back_to_later_proxy_input() {
        let policy = Policy::from_parts("", None, true).unwrap();
        policy
            .run(async {
                assert!(proxy_for_connection("invalid://must-not-be-read")
                    .unwrap()
                    .is_none());
            })
            .await
            .unwrap();
        assert!(proxy_for_connection("invalid://must-not-be-read").is_err());
    }
    #[test]
    fn changed_trust_changes_private_reuse_identity() {
        let first = Policy::from_parts("host:22 key-a", None, true).unwrap();
        let second = Policy::from_parts("host:22 key-b", None, true).unwrap();
        assert_ne!(first.identity(), second.identity());
    }
    #[test]
    fn file_key_paths_never_count_as_reusable_credential_snapshots() {
        let mut target = Session::new_empty();
        target.auth = crate::config::AuthMethod::Key;
        target.private_key_path = "/synthetic/key".into();
        assert!(!reusable_credentials(&target, &[]));
        target.private_key_inline = crate::config::Secret::new("synthetic-inline-material");
        assert!(reusable_credentials(&target, &[]));
        let mut jump = target.clone();
        jump.private_key_inline = crate::config::Secret::default();
        assert!(!reusable_credentials(&target, &[jump]));
    }
}
