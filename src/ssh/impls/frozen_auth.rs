//! Parsed authentication inputs scoped to one immutable broker route. This
//! module does not load profiles or authorize requests. Its caller supplies a
//! consistent saved route and already-read trust bytes, and must revalidate
//! trust/permission generation before publishing readiness.
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use anyhow::{anyhow, ensure, Result};
use russh::keys::PrivateKey;
use crate::config::{AuthMethod, Session};
use super::known_hosts::HostKeySnapshot;

tokio::task_local! { static FROZEN: Arc<FrozenAuthentication>; }

pub(crate) struct FrozenAuthentication {
    keys: HashMap<String, Arc<PrivateKey>>,
    trust: Arc<HostKeySnapshot>,
    proxy: crate::ssh::proxy::ResolvedProxy,
}

impl FrozenAuthentication {
    /// Sessions have their key file bytes frozen into private_key_inline by
    /// the snapshot reader. This constructor NEVER follows private_key_path.
    pub(crate) fn parse(sessions: &[Session], trust_text: &str, proxy: crate::ssh::proxy::ResolvedProxy) -> Result<Self> {
        let mut keys = HashMap::new();
        for session in sessions {
            if session.auth != AuthMethod::Key { continue; }
            let text = session.private_key_inline.as_str();
            ensure!(!text.trim().is_empty(), "frozen private key bytes are missing");
            let pass = session.password.as_str();
            let key = crate::ssh::key_import::decode_saved_inline(text, pass)
                .map_err(|_| anyhow!("frozen private key could not be parsed"))?;
            ensure!(keys.insert(session.id.clone(), Arc::new(key)).is_none(), "duplicate SSH identity in frozen route");
        }
        Ok(Self { keys, trust: Arc::new(HostKeySnapshot::from_text(trust_text)), proxy })
    }

    pub(crate) async fn run<F: Future>(self: Arc<Self>, future: F) -> F::Output {
        FROZEN.scope(self, future).await
    }
}

/// None means the existing interactive/non-broker path. Some(Err) means a
/// broken frozen route and MUST NOT fall back to filesystem key loading.
pub(crate) fn key(session: &Session) -> Option<Result<Arc<PrivateKey>>> {
    FROZEN.try_with(|snapshot| snapshot.keys.get(&session.id).cloned()
        .ok_or_else(|| anyhow!("private key missing from frozen route"))).ok()
}

/// Capture this into the handler before russh spawns its transport task;
/// Tokio task locals themselves do not cross a spawned-task boundary.
pub(crate) fn trust() -> Option<Arc<HostKeySnapshot>> {
    FROZEN.try_with(|snapshot| snapshot.trust.clone()).ok()
}

/// Some(ExplicitDirect) is a resolved choice, not permission to read env.
pub(crate) fn proxy() -> Option<crate::ssh::proxy::ResolvedProxy> {
    FROZEN.try_with(|snapshot| snapshot.proxy.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::{private::Ed25519Keypair, LineEnding};

    #[tokio::test]
    async fn frozen_key_and_verifier_do_not_follow_later_path_or_trust_changes() {
        let original = PrivateKey::new(Ed25519Keypair::from_seed(&[17; 32]).into(), "fixture").unwrap();
        let replacement = PrivateKey::new(Ed25519Keypair::from_seed(&[18; 32]).into(), "fixture").unwrap();
        let mut session = Session::new_empty(); session.auth = AuthMethod::Key;
        session.private_key_path = "/not-opened-by-frozen-auth".into();
        session.private_key_inline = crate::config::Secret::new(original.to_openssh(LineEnding::LF).unwrap().to_string());
        let old_trust = format!("fixture.invalid:22 {}", original.public_key().to_openssh().unwrap());
        let frozen = Arc::new(FrozenAuthentication::parse(&[session.clone()], &old_trust, crate::ssh::proxy::ResolvedProxy::ExplicitDirect).unwrap());
        session.private_key_inline = crate::config::Secret::new(replacement.to_openssh(LineEnding::LF).unwrap().to_string());
        frozen.run(async {
            let parsed = key(&session).unwrap().unwrap();
            assert_eq!(parsed.public_key(), original.public_key());
            let verifier = trust().unwrap();
            // Use a spawned task to test the captured verifier, not the task local.
            tokio::spawn(async move {
                assert!(verifier.accepts("fixture.invalid", 22, original.public_key()));
                assert!(!verifier.accepts("fixture.invalid", 22, replacement.public_key()));
            }).await.unwrap();
        }).await;
        assert!(key(&session).is_none());
    }

    #[tokio::test]
    async fn missing_frozen_key_does_not_fall_back_to_saved_path() {
        let mut session = Session::new_empty(); session.auth = AuthMethod::Key;
        session.private_key_path = "/must-not-be-read".into();
        let frozen = Arc::new(FrozenAuthentication::parse(&[], "", crate::ssh::proxy::ResolvedProxy::ExplicitDirect).unwrap());
        frozen.run(async { assert!(key(&session).unwrap().is_err()); }).await;
        assert!(FrozenAuthentication::parse(&[session], "", crate::ssh::proxy::ResolvedProxy::ExplicitDirect).is_err());
    }
}
