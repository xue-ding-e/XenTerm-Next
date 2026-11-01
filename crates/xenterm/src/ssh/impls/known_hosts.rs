//! Host-key verification store (#109-5 / #105).
//!
//! Replaces the old "accept any server key" behaviour with a TOFU-style
//! known_hosts file plus a first-connect confirmation dialog:
//!   • unknown host  → prompt the user with the key fingerprint; on accept the
//!                     key is remembered here.
//!   • known + match  → connect silently.
//!   • known + differ → flagged as *changed* (possible MITM); the user must
//!                     re-confirm before the new key replaces the stored one.
//!
//! The file lives next to `sessions.json` (one entry per line):
//!     `host:port ssh-ed25519 AAAA...`
//! i.e. the `host:port` id followed by the key in its OpenSSH one-line form.

use std::path::PathBuf;

use super::structs::HostKeyStatus;
use anyhow::{Context, Result};
use russh::keys::{HashAlg, PublicKey};
use std::sync::Mutex;

/// Serializes load→rewrite across remember/forget.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// `host:port` lookup key.
fn id(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

/// Path to the known_hosts file (alongside sessions.json, in the portable-first
/// data dir — #141).
fn path() -> Option<PathBuf> {
    Some(crate::config::data_dir().join("known_hosts"))
}

/// The presented key in its canonical OpenSSH one-line form (`type base64`,
/// no comment), used for exact comparison and for storage.
fn openssh_line(key: &PublicKey) -> String {
    // `to_openssh` only fails on an unsupported/!encodable key, which russh
    // would not have negotiated; fall back to the SHA256 fingerprint so a
    // freak case still stores *something* stable rather than panicking.
    key.to_openssh().unwrap_or_else(|_| fingerprint(key))
}

/// Human-readable SHA256 fingerprint (`SHA256:base64…`) shown in the dialog.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// Parse the file into `(id, openssh_key)` entries. Missing file → empty.
/// Malformed / comment (`#`) lines are skipped.
fn load() -> Vec<(String, String)> {
    let Some(p) = path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (id, key) = line.split_once(char::is_whitespace)?;
            Some((id.to_string(), key.trim().to_string()))
        })
        .collect()
}

/// Check a presented server key against the store.
pub fn verify(host: &str, port: u16, key: &PublicKey) -> HostKeyStatus {
    let want = openssh_line(key);
    let id = id(host, port);
    let mut seen_host = false;
    for (entry_id, entry_key) in load() {
        if entry_id != id {
            continue;
        }
        seen_host = true;
        if entry_key == want {
            return HostKeyStatus::Match;
        }
    }
    if seen_host {
        HostKeyStatus::Changed
    } else {
        HostKeyStatus::Unknown
    }
}

/// One stored host key, projected for a read-only list in the frontend.
pub struct KnownHostEntry {
    /// `host:port` lookup key, as stored.
    pub id: String,
    /// Negotiated algorithm name (`ssh-ed25519`, `rsa-sha2-256`, …).
    pub key_type: String,
    /// SHA256 fingerprint in OpenSSH form; empty when the stored line cannot
    /// be parsed (the entry is still listed so it can be removed).
    pub fingerprint: String,
}

/// Every remembered host key. Missing file → empty; malformed lines are
/// skipped, matching the verifier's own reading rules.
pub fn entries() -> Vec<KnownHostEntry> {
    load()
        .into_iter()
        .map(|(id, key)| {
            let parsed = PublicKey::from_openssh(&key).ok();
            let key_type = parsed
                .as_ref()
                .map(|key| key.algorithm().as_str().to_string())
                .unwrap_or_else(|| {
                    key.split_whitespace()
                        .next()
                        .unwrap_or("unknown")
                        .to_string()
                });
            let fingerprint = parsed
                .as_ref()
                .map(fingerprint)
                .unwrap_or_default();
            KnownHostEntry {
                id,
                key_type,
                fingerprint,
            }
        })
        .collect()
}

/// Forget every stored key for one `host:port` id. Returns whether anything was
/// removed. Rewrites atomically like [`remember`].
pub fn forget(id: &str) -> Result<bool> {
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let p = path().context("could not determine config directory")?;
    let rows = load();
    let total = rows.len();
    let kept: Vec<(String, String)> = rows
        .into_iter()
        .filter(|(entry_id, _)| entry_id != id)
        .collect();
    let removed = kept.len() != total;
    if removed {
        let mut out = String::new();
        for (entry_id, entry_key) in kept {
            out.push_str(&entry_id);
            out.push(' ');
            out.push_str(&entry_key);
            out.push('\n');
        }
        let mut tmp = p.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        std::fs::write(&tmp, out).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &p)
            .with_context(|| format!("replace {}", p.display()))?;
    }
    Ok(removed)
}

/// Remember (or replace) the key for `host:port`. Rewrites the file with any
/// stale entry for the same id removed, then appends the new one.
pub fn remember(host: &str, port: u16, key: &PublicKey) -> Result<()> {
    // Serialize load→rewrite inside this process: a shell and an SFTP
    // connection accepting the same first-seen host concurrently would
    // otherwise each rewrite from their own load, dropping the other's entry
    // (audit N-低2).
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let p = path().context("could not determine config directory")?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).context("create config dir")?;
    }
    let id = id(host, port);
    let line = openssh_line(key);
    let mut out = String::new();
    for (entry_id, entry_key) in load() {
        if entry_id == id {
            continue; // drop the old key for this host:port
        }
        out.push_str(&entry_id);
        out.push(' ');
        out.push_str(&entry_key);
        out.push('\n');
    }
    out.push_str(&id);
    out.push(' ');
    out.push_str(&line);
    out.push('\n');
    // Write-then-rename so a crash mid-write cannot leave a truncated file: a
    // truncated known_hosts demotes every host back to "unknown", which is a
    // re-confirmation window an attacker could social-engineer through.
    let mut tmp = p.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, out).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &p)
        .with_context(|| format!("replace {}", p.display()))?;
    Ok(())
}
