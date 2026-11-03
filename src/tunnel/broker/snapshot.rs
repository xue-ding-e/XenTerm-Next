//! Immutable route material built from trusted, already-opened input handles.
//! Platform/profile acquisition is deliberately separate: it must pin directory
//! identity, hold the profile read lock and validate file identity/generations.
//! This constructor never reopens a path supplied in a Session.
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use anyhow::{anyhow, ensure, Result};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
use crate::config::{AuthMethod, Secret, Session};
use crate::ssh::frozen_auth::FrozenAuthentication;

pub(super) struct ResolvedRouteSnapshot {
    // Retain the directory handle for the entire route lifetime. The future
    // OS loader must verify identity rather than treating its path as identity.
    _profile: File,
    session: Session,
    jumps: Vec<Session>,
    index: usize,
    fingerprint: [u8; 32],
    permission_generation: u64,
    authentication: Arc<FrozenAuthentication>,
}

impl ResolvedRouteSnapshot {
    /// The trusted loader supplies a consistent ConfigStore route and open
    /// key/trust handles acquired under its read/identity checks. Parsing/KDF
    /// work belongs on spawn_blocking, not a Tokio I/O worker.
    pub fn from_open_files(profile: File, mut session: Session, mut jumps: Vec<Session>,
        index: usize, mut key_files: HashMap<String, File>, trust_file: File,
        permission_generation: u64) -> Result<Self> {
        ensure!(profile.metadata()?.is_dir(), "profile handle is not a directory");
        let trust = read_text(trust_file, 4 * 1024 * 1024)?;
        let mut ids = std::collections::HashSet::new();
        for hop in jumps.iter_mut().chain(std::iter::once(&mut session)) {
            ensure!(ids.insert(hop.id.clone()), "duplicate session in route snapshot");
            if hop.auth == AuthMethod::Key && hop.private_key_inline.as_str().trim().is_empty() {
                let file = key_files.remove(&hop.id).ok_or_else(|| anyhow!("opened key missing from route snapshot"))?;
                let bytes = read_text(file, 1024 * 1024)?;
                hop.private_key_inline = Secret::new(bytes.as_str());
            }
            // A parsed-key-only route must not retain any filesystem fallback.
            hop.private_key_path.clear();
        }
        ensure!(key_files.is_empty(), "unrelated key supplied to route snapshot");
        let effective_proxy = crate::ssh::proxy::ResolvedProxy::capture(&jumps.last().unwrap_or(&session).proxy)?;
        let proxy_fingerprint = effective_proxy.fingerprint()?;
        let route = super::super::owner::route_fingerprint(&session, &jumps, index)?;
        let authentication = Arc::new(FrozenAuthentication::parse(
            &jumps.iter().chain(std::iter::once(&session)).cloned().collect::<Vec<_>>(), &trust, effective_proxy)?);
        let mut digest = Sha256::new();
        digest.update(route);
        digest.update(proxy_fingerprint);
        digest.update(trust.as_bytes());
        digest.update(permission_generation.to_be_bytes());
        Ok(Self { _profile: profile, session, jumps, index, fingerprint: digest.finalize().into(),
            permission_generation, authentication })
    }

    pub fn permission_generation(&self) -> u64 { self.permission_generation }
    pub fn fingerprint(&self) -> [u8; 32] { self.fingerprint }
    pub fn rule_index(&self) -> usize { self.index }

    /// Actual authentication consumes the parsed keys and captured verifier
    /// associated with this exact fingerprint, not a fresh ConfigStore read.
    pub async fn connect(&self) -> Result<(russh::client::Handle<crate::ssh::ClientHandler>,
        Vec<russh::client::Handle<crate::ssh::ClientHandler>>)> {
        self.authentication.clone().run(crate::ssh::open_forward_connection(&self.session, &self.jumps)).await
    }
}

fn read_text(mut file: File, maximum: u64) -> Result<Zeroizing<String>> {
    ensure!(file.metadata()?.is_file(), "snapshot input is not a regular file");
    let mut bytes = Zeroizing::new(Vec::new());
    file.by_ref().take(maximum + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= maximum, "snapshot input exceeds size limit");
    let text = std::str::from_utf8(&bytes).map_err(|_| anyhow!("snapshot input is not UTF-8"))?;
    Ok(Zeroizing::new(text.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::{PrivateKey, ssh_key::{private::Ed25519Keypair, LineEnding}};

    fn open_directory(path: &std::path::Path) -> File {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)] {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_FLAG_BACKUP_SEMANTICS: open the fixture directory itself.
            options.custom_flags(0x02000000);
        }
        options.open(path).unwrap()
    }

    #[tokio::test]
    async fn opened_key_and_trust_replacement_cannot_mix_digest_and_authentication() {
        let directory = tempfile::tempdir().unwrap();
        let first = PrivateKey::new(Ed25519Keypair::from_seed(&[21; 32]).into(), "fixture").unwrap();
        let second = PrivateKey::new(Ed25519Keypair::from_seed(&[22; 32]).into(), "fixture").unwrap();
        let key_path = directory.path().join("fixture-key");
        let trust_path = directory.path().join("known_hosts");
        let first_key = first.to_openssh(LineEnding::LF).unwrap();
        std::fs::write(&key_path, first_key.as_bytes()).unwrap();
        std::fs::write(&trust_path, format!("fixture.invalid:22 {}", first.public_key().to_openssh().unwrap())).unwrap();
        let old_key = File::open(&key_path).unwrap();
        let old_trust = File::open(&trust_path).unwrap();
        // A deterministic barrier at the handle boundary: replace the names
        // after opening, before reading/parsing. No snapshot path may reopen.
        std::fs::rename(&key_path, directory.path().join("old-key")).unwrap();
        std::fs::rename(&trust_path, directory.path().join("old-trust")).unwrap();
        std::fs::write(&key_path, second.to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();
        std::fs::write(&trust_path, format!("fixture.invalid:22 {}", second.public_key().to_openssh().unwrap())).unwrap();
        let mut session = Session::new_empty();
        session.auth = AuthMethod::Key;
        session.host = "fixture.invalid".into();
        session.private_key_path = key_path.to_string_lossy().into_owned();
        session.forwards.push(crate::config::PortForward { kind: "dynamic".into(), bind_port: 1080, ..Default::default() });
        let old = ResolvedRouteSnapshot::from_open_files(open_directory(directory.path()), session.clone(), vec![], 0,
            HashMap::from([(session.id.clone(), old_key)]), old_trust, 3).unwrap();
        let current = ResolvedRouteSnapshot::from_open_files(open_directory(directory.path()), session.clone(), vec![], 0,
            HashMap::from([(session.id.clone(), File::open(&key_path).unwrap())]), File::open(&trust_path).unwrap(), 3).unwrap();
        assert_ne!(old.fingerprint(), current.fingerprint());
        old.authentication.clone().run(async {
            let parsed = crate::ssh::frozen_auth::key(&session).unwrap().unwrap();
            assert_eq!(parsed.public_key(), first.public_key());
            let trust = crate::ssh::frozen_auth::trust().unwrap();
            assert!(trust.accepts("fixture.invalid", 22, first.public_key()));
            assert!(!trust.accepts("fixture.invalid", 22, second.public_key()));
        }).await;
    }
}
