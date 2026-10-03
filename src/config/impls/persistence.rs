//! Optimistic, ordered commits shared by every configuration writer.
use super::*;
use sha2::{Digest, Sha256};
use std::sync::Arc;

impl ConfigStore {
    /// The caller holds one SQLite transaction across all three reads. Include
    /// raw rows (not decoded/truncated history) to notice older writers too.
    pub(super) fn disk_fingerprint(conn: &rusqlite::Connection) -> Result<Option<[u8; 32]>> {
        let mut digest = Sha256::new();
        let mut count = 0;
        for (table, query, columns) in [
            ("meta", "SELECT key,value FROM meta ORDER BY key", 2),
            (
                "sessions",
                "SELECT CAST(ordinal AS TEXT),id,data FROM sessions ORDER BY ordinal,id",
                3,
            ),
            (
                "history",
                "SELECT CAST(seq AS TEXT),command FROM command_history ORDER BY seq",
                2,
            ),
        ] {
            digest.update(table.as_bytes());
            let mut statement = conn.prepare(query)?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                count += 1;
                for column in 0..columns {
                    let value: String = row.get(column)?;
                    digest.update((value.len() as u64).to_le_bytes());
                    digest.update(value.as_bytes());
                }
            }
        }
        Ok((count > 0).then(|| digest.finalize().into()))
    }

    pub(super) fn validate_snapshot(conn: &rusqlite::Connection, saved: &SavedState) -> Result<()> {
        if Self::disk_fingerprint(conn)? != saved.disk_fingerprint {
            return Err(ConfigurationChanged.into());
        }
        Ok(())
    }

    /// The token changes even when a keyring-only edit leaves its SQL marker
    /// identical. Raw fingerprints additionally cover old/uninstrumented SQL.
    pub(super) fn stamp_commit(conn: &rusqlite::Connection, token: &str) -> Result<()> {
        conn.execute(
            "INSERT INTO meta(key,value) VALUES('write_revision',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [token],
        )?;
        Ok(())
    }

    pub(super) fn finish_snapshot(
        saved: &mut SavedState,
        cache: &ConfigFile,
        fingerprint: Option<[u8; 32]>,
    ) {
        let next = SavedState::of_cache(cache);
        *saved = SavedState {
            disk_fingerprint: fingerprint,
            submitted: saved.submitted,
            attempted: saved.attempted,
            ..next
        };
    }

    pub(super) fn record_save_error(saved: &mut SavedState, error: &anyhow::Error) {
        if error
            .downcast_ref::<SessionCredentialRollbackFailed>()
            .is_some()
        {
            saved.credentials_uncertain = true;
        }
        saved.error = Some(if saved.credentials_uncertain {
            "Could not confirm configuration save or credential recovery. Keep pending edits, reopen XenTerm to attempt recovery, and check saved credentials before retrying.".into()
        } else if error.downcast_ref::<ConfigurationChanged>().is_some() {
            "Configuration changed elsewhere. Your changes were not saved. Keep pending edits, then reopen XenTerm to reload.".into()
        } else {
            "Configuration could not be saved. Your changes remain in memory; keep pending edits before closing XenTerm and retry when storage is available.".into()
        });
    }

    pub fn persistence_error(&self) -> Option<String> {
        self.saved_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .error
            .clone()
    }

    /// One consistent load, including keyring reads. BEGIN IMMEDIATE keeps a
    /// cooperating writer from changing credentials before their SQL commits.
    pub(super) fn read_cache_snapshot(
        path: &Path,
        key: &[u8; 32],
    ) -> Result<(ConfigFile, Option<[u8; 32]>)> {
        let _ordered = Self::save_lock().lock().unwrap_or_else(|p| p.into_inner());
        let io = profile_io::ProfileIoGuard::acquire(path)?;
        Self::read_cache_snapshot_locked(path, key, &io)
    }

    pub(super) fn read_cache_snapshot_locked(
        path: &Path,
        key: &[u8; 32],
        io: &profile_io::ProfileIoGuard,
    ) -> Result<(ConfigFile, Option<[u8; 32]>)> {
        let mut conn = if path.exists() {
            Self::open_existing_db(path)?
        } else {
            Self::open_db(path)?
        };
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        io.recover(&tx, key)?;
        let fingerprint = Self::disk_fingerprint(&tx)?;
        let cache = if let Some((settings, sessions, history)) = Self::read_disk_store(&tx)? {
            let mut cache: ConfigFile = serde_json::from_str(&settings).map_err(|_| {
                anyhow::anyhow!("sessions.db settings blob is not a valid config document")
            })?;
            if let Some(plain) = Self::try_decrypt(key, cache.webdav_password.as_str()) {
                cache.webdav_password = Secret::new(plain);
            }
            cache.sessions = sessions;
            for session in &mut cache.sessions {
                Self::session_from_disk_form(session, key);
            }
            cache.command_history = history;
            cache
        } else {
            if fingerprint.is_some() {
                bail!("configuration settings are missing; original database preserved");
            }
            fresh_config()
        };
        tx.commit()?;
        Ok((cache, fingerprint))
    }

    /// Called under the process save lock, with this store's state locked.
    /// All errors leave both the disk baseline and live cache intact.
    pub(super) fn persist_snapshot(
        cache: &ConfigFile,
        saved: &mut SavedState,
        forced: Option<SavePlan>,
        key: [u8; 32],
        path: &Path,
        backup_dir: Option<&Path>,
        keyring_enabled: bool,
    ) -> Result<()> {
        if saved.credentials_uncertain {
            return Err(SessionCredentialRollbackFailed.into());
        }
        let mut plan = forced.unwrap_or_else(|| Self::plan_save(cache, saved));
        // A no-op still checks staleness: callers must not mistake a stale
        // editor submission for a confirmed save.
        if saved.disk_fingerprint.is_some() && !path.exists() {
            return Err(ConfigurationChanged.into());
        }
        let io = profile_io::ProfileIoGuard::acquire(path)?;
        let mut conn = if saved.disk_fingerprint.is_some() {
            Self::open_existing_db(path)?
        } else {
            Self::open_db(path)?
        };
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        io.recover(&tx, &key)?;
        Self::validate_snapshot(&tx, saved)?;
        if plan.is_empty() && saved.disk_fingerprint.is_some() {
            saved.error = None;
            return Ok(());
        }
        if saved.disk_fingerprint.is_none() {
            plan = SavePlan::all();
        }

        // Capture only credentials this plan can modify, after OCC accepted
        // the cache and while its writer lock is held. Never guess a credential
        // that could not be read: use encrypted-file fallback for that write.
        let mut credentials = std::collections::HashMap::new();
        if keyring_enabled {
            let affected = if plan.all_sessions {
                saved
                    .sessions
                    .keys()
                    .chain(cache.sessions.iter().map(|s| &s.id))
                    .cloned()
                    .collect::<std::collections::HashSet<_>>()
            } else {
                plan.sessions.iter().cloned().collect()
            };
            for id in affected {
                if let Ok(previous) = Self::read_keyring_password(&id) {
                    credentials.insert(id, previous);
                }
            }
        }
        let commit_token = Uuid::new_v4().to_string();
        if !credentials.is_empty() {
            io.prepare(&tx, &key, &credentials, &commit_token)?;
        }
        let result = (|| -> Result<Option<[u8; 32]>> {
            Self::write_store(&tx, cache, &plan, key, &credentials)?;
            // Cleanup participates in compensation as well; doing it after
            // COMMIT would race the next writer's newly saved password.
            for id in credentials.keys() {
                if cache
                    .sessions
                    .iter()
                    .find(|s| &s.id == id)
                    .is_none_or(|s| s.password.is_empty())
                {
                    Self::keyring_set_password(id, "")?;
                }
            }
            Self::stamp_commit(&tx, &commit_token)?;
            let fingerprint = Self::disk_fingerprint(&tx)?;
            // Keep the Transaction object alive on failure so compensation
            // precedes releasing SQLite's writer lock.
            tx.execute_batch("COMMIT")
                .context("failed to commit the config transaction")?;
            Ok(fingerprint)
        })();
        let fingerprint = match result {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                // An automatic SQLite rollback can release the write lock.
                // Reacquire and validate before compensation; never overwrite
                // credentials belonging to a newer successful commit.
                if tx.is_autocommit() && (!credentials.is_empty()) {
                    if tx.execute_batch("BEGIN IMMEDIATE").is_err()
                        || Self::validate_snapshot(&tx, saved).is_err()
                    {
                        return Err(error.context(SessionCredentialRollbackFailed));
                    }
                }
                let mut failed = false;
                for (id, previous) in &credentials {
                    failed |= Self::restore_keyring_password(id, previous.as_ref()).is_err();
                }
                if failed {
                    return Err(error.context(SessionCredentialRollbackFailed));
                }
                if !credentials.is_empty() && io.finish().is_err() {
                    return Err(error.context(SessionCredentialRollbackFailed));
                }
                return Err(error);
            }
        };
        // SQL is committed: failure to remove a now-committed journal must
        // not falsely roll the caller's cache back. The next guarded load
        // recognizes this commit token and completes harmless cleanup.
        if !credentials.is_empty() {
            if let Err(error) = io.finish() {
                tracing::warn!("committed credential journal cleanup deferred: {error:#}");
            }
        }
        Self::finish_snapshot(saved, cache, fingerprint);
        drop(tx);
        if plan.settings || plan.all_sessions || !plan.sessions.is_empty() {
            Self::sync_backup_to(backup_dir, path, path.parent());
        }
        Ok(())
    }

    /// Execute a queued snapshot in logical submission order. A newer attempt
    /// (even a failed one) cancels earlier queued snapshots; no stale task may
    /// silently save after its caller already submitted a newer state.
    pub(super) fn run_background_save(
        cache: ConfigFile,
        sequence: u64,
        shared: Arc<Mutex<SavedState>>,
        key: [u8; 32],
        path: PathBuf,
        backup_dir: Option<PathBuf>,
        keyring_enabled: bool,
    ) {
        let _ordered = Self::save_lock().lock().unwrap_or_else(|p| p.into_inner());
        let mut saved = shared.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if sequence <= saved.attempted {
            return;
        }
        saved.attempted = sequence;
        if let Err(error) = Self::persist_snapshot(
            &cache,
            &mut saved,
            None,
            key,
            &path,
            backup_dir.as_deref(),
            keyring_enabled,
        ) {
            Self::record_save_error(&mut saved, &error);
            tracing::warn!("background config save failed: {error:#}");
        }
        Self::publish_snapshot(&shared, saved);
    }

    pub(super) fn publish_snapshot(shared: &Arc<Mutex<SavedState>>, mut saved: SavedState) {
        let mut current = shared.lock().unwrap_or_else(|p| p.into_inner());
        // Queue submissions may happen while SQLite/keyring work is running.
        saved.submitted = current.submitted;
        *current = saved;
    }
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
