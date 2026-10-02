//! Append-only session imports. Parse and validate the entire batch before either
//! the in-memory profile or its on-disk snapshot is changed.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{ConfigFile, ConfigStore, SavedState, Secret, Session, SessionKind};

/// Bound file reads as well as JSON parsing, including files that grow while read.
const MAX_IMPORT_BYTES: usize = 16 * 1024 * 1024;

/// Import results deliberately contain no connection details or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportSummary {
    pub added: usize,
    pub skipped: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ImportWarning>,
}

/// Fixed, non-sensitive compatibility notices. Never echo source field values,
/// session names/IDs, credentials, or unknown field names supplied by a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportWarning {
    pub code: &'static str,
    pub field: &'static str,
    pub entries: usize,
    pub message: &'static str,
}

fn compatibility_warnings(value: &serde_json::Value) -> Result<Vec<ImportWarning>> {
    let Some(sessions) = value.get("sessions").and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    let legacy = [
        ("session_log", "Legacy session logging preferences were not applied; this XenTerm version does not implement them."),
        ("allow_secret_reveal", "Legacy secret-reveal preferences were not applied; importing never enables credential reveal."),
        ("rdp_domain", "Legacy RDP domain settings were not applied; this XenTerm version does not implement RDP."),
        ("rdp_width", "Legacy RDP width settings were not applied; this XenTerm version does not implement RDP."),
        ("rdp_height", "Legacy RDP height settings were not applied; this XenTerm version does not implement RDP."),
        ("rdp_fullscreen", "Legacy RDP fullscreen settings were not applied; this XenTerm version does not implement RDP."),
    ];
    let mut supported: HashSet<String> = serde_json::to_value(Session::new_empty())?
        .as_object()
        .expect("Session serializes as an object")
        .keys()
        .cloned()
        .collect();
    supported.insert("jump_session_ids".into()); // Omitted when empty in v1 exports.
    let mut warnings = Vec::new();
    for (field, message) in legacy {
        let entries = sessions
            .iter()
            .filter(|session| session.get(field).is_some())
            .count();
        if entries > 0 {
            warnings.push(ImportWarning {
                code: "unsupported_session_field",
                field,
                entries,
                message,
            });
        }
    }
    let unknown = sessions
        .iter()
        .filter(|session| {
            session.as_object().is_some_and(|object| {
                object.keys().any(|field| {
                    !supported.contains(field) && !legacy.iter().any(|(known, _)| field == known)
                })
            })
        })
        .count();
    if unknown > 0 {
        warnings.push(ImportWarning { code: "unknown_session_fields", field: "unknown", entries: unknown,
            message: "Additional unrecognized session fields were not applied; retain the original export." });
    }
    for session in sessions {
        if let Some(kind) = session.get("kind").and_then(serde_json::Value::as_str) {
            if !matches!(kind, "ssh" | "serial" | "telnet" | "local") {
                bail!("import contains an unsupported session transport; supported kinds are ssh, serial, telnet and local; no sessions were imported");
            }
        }
    }
    Ok(warnings)
}

#[derive(Deserialize)]
struct ImportedSessions {
    // Deliberately required: an arbitrary settings object is not a session export.
    sessions: Vec<Session>,
}

type SessionIdentity = String;

fn identity(session: &Session, sessions: &[Session]) -> Result<SessionIdentity> {
    // An endpoint is not a profile identity: users deliberately save aliases
    // with different names, credentials, groups, proxies and jump routes.
    // Compare the full configuration, excluding machine-local IDs/last_used.
    // Hash only in memory so secret values are not retained as lookup keys.
    fn fields(session: &Session) -> Result<serde_json::Value> {
        let mut value = serde_json::to_value(session)?;
        let object = value
            .as_object_mut()
            .expect("Session serializes as an object");
        for key in ["id", "last_used", "jump_session_id", "jump_session_ids"] {
            object.remove(key);
        }
        Ok(value)
    }
    let mut value = fields(session)?;
    let route = super::super::jump_chain::resolve_jump_chain(sessions, session)?;
    let route: Vec<_> = route.iter().map(fields).collect::<Result<_>>()?;
    value["resolved_jump_chain"] = serde_json::Value::Array(route);
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
}

fn read_import(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path).context("failed to inspect import file")?;
    if !metadata.is_file() {
        bail!("import source must be a regular JSON file");
    }
    if metadata.len() > MAX_IMPORT_BYTES as u64 {
        bail!("import file exceeds the 16 MiB limit");
    }
    let file = fs::File::open(path).context("failed to open import file")?;
    if !file
        .metadata()
        .context("failed to inspect import file")?
        .is_file()
    {
        bail!("import source must be a regular JSON file");
    }
    let mut bytes = Vec::new();
    file.take((MAX_IMPORT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("failed to read import file")?;
    if bytes.len() > MAX_IMPORT_BYTES {
        bail!("import file exceeds the 16 MiB limit");
    }
    // Do not attach UTF-8/serde errors: they can quote credential-bearing input.
    String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("import file must be UTF-8 JSON"))
}

impl ConfigStore {
    /// Import a portable MeatShell export, native sessions.json or FinalShell
    /// connection JSON. Global settings in native files are never imported.
    /// Dry-run performs the same parsing/validation without saving or mutating
    /// this store. Existing sessions are never overwritten.
    pub fn import_from_preview(&mut self, path: &Path, dry_run: bool) -> Result<ImportSummary> {
        self.import_json_preview(&read_import(path)?, dry_run)
    }

    pub fn import_json_preview(&mut self, raw: &str, dry_run: bool) -> Result<ImportSummary> {
        if raw.len() > MAX_IMPORT_BYTES {
            bail!("import file exceeds the 16 MiB limit");
        }
        let value: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
            anyhow::anyhow!(
                "invalid import JSON at line {}, column {}",
                error.line(),
                error.column()
            )
        })?;
        let meatshell = value.get("meatshell_export").is_some() || value.get("sessions").is_some();
        let warnings = if meatshell {
            compatibility_warnings(&value)?
        } else {
            Vec::new()
        };
        let mut sessions = if meatshell {
            if let Some(version) = value.get("meatshell_export") {
                if version.as_u64() != Some(1) {
                    bail!("unsupported MeatShell export version; expected version 1");
                }
            }
            serde_json::from_value::<ImportedSessions>(value)
                .map_err(|_| anyhow::anyhow!("invalid MeatShell session fields in import file"))?
                .sessions
        } else {
            // FinalShell's decoder already returns plaintext. Never reinterpret
            // a coincidental enc:* prefix in that decoded password as our format.
            super::super::finalshell::parse_export(raw).map_err(|_| {
                anyhow::anyhow!("invalid or unsupported FinalShell import; check connection fields and password encoding")
            })?
        };

        let mut source_ids = HashSet::new();
        for (index, session) in sessions.iter_mut().enumerate() {
            // Match the editor/load normalization for display-only group names,
            // so repeating an import after a reload remains idempotent.
            if super::is_reserved_session_group(session.group.trim()) {
                session.group.clear();
            }
            if session.id.trim().is_empty() || !source_ids.insert(session.id.clone()) {
                bail!(
                    "import entry {} has an empty or repeated session ID",
                    index + 1
                );
            }
            match session.kind {
                SessionKind::Ssh | SessionKind::Telnet => {
                    // A single final dot is a valid absolute DNS name. Keep
                    // the original spelling, while validating its DNS labels.
                    let host = session.host.strip_suffix('.').unwrap_or(&session.host);
                    if !super::super::validation::is_valid_hostname(host) || session.port == 0 {
                        bail!("import entry {} has an invalid host or port", index + 1);
                    }
                }
                SessionKind::Serial if session.serial_port.trim().is_empty() => {
                    bail!("import entry {} has an empty serial device", index + 1);
                }
                _ => {}
            }
            // Paths are portable references, not files to open while importing.
            // They can legitimately refer to another OS or a not-yet-mounted key.
            for path in [
                &session.private_key_path,
                &session.local_working_dir,
                &session.serial_port,
            ] {
                if path.chars().any(char::is_control) {
                    bail!("import entry {} has an invalid path", index + 1);
                }
            }
            if session.user.chars().any(char::is_control) {
                bail!("import entry {} has an invalid user", index + 1);
            }
            if meatshell {
                self.decode_import_secret(&mut session.password, index, "password")?;
                self.decode_import_secret(&mut session.private_key_inline, index, "private key")?;
                for trigger in &mut session.triggers {
                    self.decode_import_secret(&mut trigger.response, index, "trigger response")?;
                }
                let mut invalid = false;
                if let Some(proxy) = Self::map_proxy_password(&session.proxy, |password| {
                    let mut secret = Secret::new(password);
                    match self.decode_import_secret(&mut secret, index, "proxy password") {
                        Ok(()) => Some(secret.as_str().to_string()),
                        Err(_) => {
                            invalid = true;
                            None
                        }
                    }
                }) {
                    session.proxy = proxy;
                }
                if invalid {
                    bail!(
                        "cannot decrypt proxy password in import entry {}",
                        index + 1
                    );
                }
            }
        }

        let mut identities = HashMap::new();
        let mut used_ids = HashSet::new();
        for session in &self.cache.sessions {
            // An unrelated invalid existing route must not prevent importing
            // valid profiles, and is never used as a duplicate match.
            if let Ok(key) = identity(session, &self.cache.sessions) {
                identities.entry(key).or_insert_with(|| session.id.clone());
            }
            used_ids.insert(session.id.clone());
        }
        let existing_ids = used_ids.clone();
        let mut remapped_ids = HashMap::new();
        let mut keep = Vec::with_capacity(sessions.len());
        let mut summary = ImportSummary {
            added: 0,
            skipped: 0,
            warnings,
        };
        let mut source_graph = sessions.clone();
        source_graph.extend(
            self.cache
                .sessions
                .iter()
                .filter(|s| !source_ids.contains(&s.id))
                .cloned(),
        );
        // Pass one maps every source ID, including duplicates, before resolving
        // references. Forward references and references to skipped hops work.
        for session in &sessions {
            let key = identity(session, &source_graph)
                .map_err(|_| anyhow::anyhow!("import contains an invalid SSH jump chain"))?;
            let id = if let Some(existing) = identities.get(&key) {
                summary.skipped += 1;
                keep.push(false);
                existing.clone()
            } else {
                let id = loop {
                    let id = Uuid::new_v4().to_string();
                    if used_ids.insert(id.clone()) {
                        break id;
                    }
                };
                identities.insert(key, id.clone());
                summary.added += 1;
                keep.push(true);
                id
            };
            remapped_ids.insert(session.id.clone(), id);
        }

        let remap = |id: &str, index: usize| -> Result<String> {
            if let Some(mapped) = remapped_ids.get(id) {
                Ok(mapped.clone())
            } else if existing_ids.contains(id) {
                Ok(id.to_string())
            } else {
                bail!(
                    "import entry {} references a missing SSH jump session",
                    index + 1
                );
            }
        };
        for (index, session) in sessions.iter_mut().enumerate() {
            session.id = remapped_ids[&session.id].clone();
            if !session.jump_session_id.is_empty() {
                session.jump_session_id = remap(&session.jump_session_id, index)?;
            }
            for id in &mut session.jump_session_ids {
                *id = remap(id, index)?;
            }
        }

        let mut candidate = self.cache.clone();
        candidate.sessions.extend(
            sessions
                .iter()
                .zip(&keep)
                .filter(|(_, keep)| **keep)
                .map(|(session, _)| session.clone()),
        );
        // Validate the final merged graph before entering a write transaction.
        for (index, session) in sessions.iter().enumerate() {
            super::super::jump_chain::resolve_jump_chain(&candidate.sessions, session).map_err(|_| {
                anyhow::anyhow!("import entry {} has an invalid SSH jump chain (missing/non-SSH hop, cycle or too many hops)", index + 1)
            })?;
        }
        if !dry_run && summary.added > 0 {
            self.commit_import(candidate)?;
        }
        Ok(summary)
    }

    /// Append only the newly validated rows in a single SQLite transaction.
    /// Existing rows, settings and keyring entries are never rewritten. Reject
    /// a stale cache under the database's write lock, before any new row exists.
    /// Imported credentials use local encryption, so rollback also has no
    /// non-transactional OS-keyring side effects.
    fn commit_import(&mut self, candidate: ConfigFile) -> Result<()> {
        let _ordered = Self::save_lock().lock().unwrap_or_else(|p| p.into_inner());
        let saved = self
            .saved_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut conn = Self::open_db(&self.path)
            .map_err(|_| anyhow::anyhow!("failed to open destination configuration"))?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| anyhow::anyhow!("failed to lock destination configuration"))?;
        let existing = Self::read_disk_store(&tx)
            .map_err(|_| anyhow::anyhow!("failed to read destination configuration"))?;
        if let Some((settings, mut sessions, history)) = existing {
            let mut disk: ConfigFile = serde_json::from_str(&settings)
                .map_err(|_| anyhow::anyhow!("invalid destination settings"))?;
            if let Some(plain) = Self::try_decrypt(&self.key, disk.webdav_password.as_str()) {
                disk.webdav_password = Secret::new(plain);
            }
            for session in &mut sessions {
                Self::session_from_disk_form(session, &self.key);
            }
            disk.sessions = sessions;
            disk.command_history = history;
            let current = SavedState::of_cache(&disk);
            if current.sessions != saved.sessions
                || current.order != saved.order
                || current.settings != saved.settings
                || current.history != saved.history
            {
                bail!("configuration changed since it was loaded; reload before importing");
            }
            // Unsaved editor changes must not be falsely marked persisted.
            let cache = SavedState::of_cache(&self.cache);
            if cache.sessions != saved.sessions
                || cache.order != saved.order
                || cache.settings != saved.settings
                || cache.history != saved.history
            {
                bail!("save or discard pending configuration changes before importing");
            }
            for (ordinal, session) in candidate
                .sessions
                .iter()
                .enumerate()
                .skip(self.cache.sessions.len())
            {
                Self::upsert_session_row(&tx, ordinal, session, self.key, false)
                    .map_err(|_| anyhow::anyhow!("failed to write imported session"))?;
            }
        } else {
            if !saved.sessions.is_empty() {
                bail!("destination configuration disappeared; reload before importing");
            }
            let mut settings = candidate.clone();
            settings.sessions.clear();
            settings.command_history.clear();
            if !settings.webdav_password.is_empty()
                && !settings.webdav_password.is_local_ciphertext()
            {
                settings.webdav_password =
                    Secret::new(Self::encrypt(&self.key, settings.webdav_password.as_str())?);
            }
            for (key, value) in [
                ("schema_version", Self::SCHEMA_VERSION.to_string()),
                ("settings", serde_json::to_string(&settings)?),
            ] {
                tx.execute(
                    "INSERT INTO meta(key, value) VALUES(?1, ?2)",
                    rusqlite::params![key, value],
                )
                .map_err(|_| anyhow::anyhow!("failed to initialise import settings"))?;
            }
            for (ordinal, session) in candidate.sessions.iter().enumerate() {
                Self::upsert_session_row(&tx, ordinal, session, self.key, false)
                    .map_err(|_| anyhow::anyhow!("failed to write imported session"))?;
            }
            for command in &candidate.command_history {
                tx.execute("INSERT INTO command_history(command) VALUES(?1)", [command])
                    .map_err(|_| anyhow::anyhow!("failed to initialise import history"))?;
            }
        }
        tx.commit()
            .map_err(|_| anyhow::anyhow!("failed to commit imported sessions"))?;
        // Mutate the cache and snapshot only after the database commit succeeds.
        *self.saved_state.lock().unwrap_or_else(|p| p.into_inner()) =
            SavedState::of_cache(&candidate);
        self.cache = candidate;
        Self::sync_backup_to(self.backup_dir.as_deref(), &self.path, self.path.parent());
        Ok(())
    }

    fn decode_import_secret(&self, secret: &mut Secret, index: usize, field: &str) -> Result<()> {
        let value = secret.as_str();
        let decoded = if value.starts_with(Self::EXPORT_PREFIX) {
            Some(Self::decrypt_export(value))
        } else if value.starts_with(Self::ENC_PREFIX) {
            Some(Self::try_decrypt(&self.key, value))
        } else if value.starts_with("enc:") || value == Self::KEYRING_MARKER {
            Some(None)
        } else {
            None
        };
        if let Some(decoded) = decoded {
            let plaintext = decoded.ok_or_else(|| {
                anyhow::anyhow!("cannot decrypt {} in import entry {}; use a portable MeatShell export or the matching local profile", field, index + 1)
            })?;
            *secret = Secret::new(plaintext);
        }
        Ok(())
    }

    /// Compatibility wrapper used by the GUI and WebDAV importer.
    pub fn import_json(&mut self, raw: &str) -> Result<(usize, usize)> {
        let summary = self.import_json_preview(raw, false)?;
        Ok((summary.added, summary.skipped))
    }

    /// Compatibility wrapper used by the GUI file picker.
    pub fn import_from(&mut self, path: &Path) -> Result<(usize, usize)> {
        let summary = self.import_from_preview(path, false)?;
        Ok((summary.added, summary.skipped))
    }
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
