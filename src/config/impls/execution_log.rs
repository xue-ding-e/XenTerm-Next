//! Serialize history publication with committed opt-outs, across processes.
use super::*;

#[derive(serde::Deserialize)]
pub(crate) struct ExecutionLogState {
    #[serde(default, rename = "mcp_execution_log_enabled")]
    pub enabled: bool,
    #[serde(
        default = "default_execution_log_retention",
        rename = "mcp_execution_log_retention_days"
    )]
    pub retention_days: u64,
    /// Missing on legacy profiles. Reads never initialize or change this token.
    #[serde(skip)]
    pub epoch: Option<String>,
}

fn read_state(conn: &rusqlite::Connection) -> Result<ExecutionLogState> {
    let settings: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='settings'", [], |row| {
            row.get(0)
        })
        .optional()?;
    let mut state: ExecutionLogState = serde_json::from_str(settings.as_deref().unwrap_or("{}"))?;
    state.epoch = conn
        .query_row(
            "SELECT value FROM meta WHERE key='execution_log_epoch'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    state.retention_days = state.retention_days.clamp(1, 365);
    Ok(state)
}

/// Called after OCC validation, in the same transaction as the settings write.
/// Keeping the token outside exported settings prevents a stale cache/import
/// from restoring a revoked generation. Failed saves roll this back as well.
pub(super) fn revoke_if_disabled(tx: &rusqlite::Transaction<'_>, enabled: bool) -> Result<()> {
    if !enabled && read_state(tx)?.enabled {
        tx.execute(
            "INSERT INTO meta(key,value) VALUES('execution_log_epoch',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [Uuid::new_v4().to_string()],
        )?;
    }
    Ok(())
}

impl ConfigStore {
    /// The history lock, when needed, is acquired BEFORE these locks. Config
    /// writers never acquire it. Keep the profile guard until append/sync ends:
    /// a successful opt-out cannot return while an admitted write is pending.
    /// The callback must not reenter config APIs or wait for the history lock.
    pub(crate) fn with_execution_log_state<T>(
        path: &Path,
        action: impl FnOnce(ExecutionLogState) -> Result<T>,
    ) -> Result<T> {
        let _ordered = Self::save_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _io = profile_io::ProfileIoGuard::acquire(path)?;
        // History cannot recover credentials and must fail closed on an
        // unresolved profile. No keys or session data are needed for this read.
        if profile_io::pending_journal(path)? {
            return Err(SessionCredentialRollbackFailed.into());
        }
        let mut conn = Self::open_existing_db(path)?;
        let tx = conn.transaction()?;
        let state = read_state(&tx)?;
        tx.commit()?;
        action(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(root: &Path) -> ConfigStore {
        let mut store = super::super::tests::temp_store();
        store.path = root.join("sessions.db");
        store.set_mcp_execution_log_enabled(true);
        store.save().unwrap();
        store
    }

    fn epoch(store: &ConfigStore) -> Option<String> {
        ConfigStore::with_execution_log_state(&store.path, |state| Ok(state.epoch)).unwrap()
    }

    #[test]
    fn epoch_changes_only_for_committed_opt_outs() {
        let root = tempfile::tempdir().unwrap();
        let mut store = store(root.path());
        assert_eq!(epoch(&store), None);
        let original = store.saved_state.lock().unwrap().disk_fingerprint;
        assert_eq!(epoch(&store), None);
        assert_eq!(store.saved_state.lock().unwrap().disk_fingerprint, original);
        store.set_mcp_execution_log_enabled(false);
        store.save().unwrap();
        let revoked = epoch(&store);
        assert!(revoked.is_some());
        store.save().unwrap();
        store.set_mcp_execution_log_enabled(true);
        store.save().unwrap();
        store.cache.language = "fixture".into();
        store.set_mcp_execution_log_retention_days(30);
        store.save().unwrap();
        assert_eq!(epoch(&store), revoked);
        store.set_mcp_execution_log_enabled(false);
        store.save_all().unwrap();
        assert_ne!(epoch(&store), revoked);
    }

    #[test]
    fn failed_settings_write_rolls_back_epoch_and_retry_revokes() {
        let root = tempfile::tempdir().unwrap();
        let mut store = store(root.path());
        let conn = ConfigStore::open_existing_db(&store.path).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_fixture_settings BEFORE UPDATE ON meta WHEN NEW.key='settings' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;",
        ).unwrap();
        store.set_mcp_execution_log_enabled(false);
        assert!(store.save().is_err());
        assert_eq!(epoch(&store), None);
        assert!(
            ConfigStore::with_execution_log_state(&store.path, |state| Ok(state.enabled)).unwrap()
        );
        conn.execute_batch("DROP TRIGGER reject_fixture_settings")
            .unwrap();
        store.save().unwrap();
        assert!(epoch(&store).is_some());
    }

    #[test]
    fn commit_failure_rolls_back_the_revocation_token() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        let mut conn = ConfigStore::open_existing_db(&store.path).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE fixture_parent(id INTEGER PRIMARY KEY);
             CREATE TABLE fixture_child(id INTEGER REFERENCES fixture_parent(id) DEFERRABLE INITIALLY DEFERRED);",
        ).unwrap();
        let tx = conn.transaction().unwrap();
        revoke_if_disabled(&tx, false).unwrap();
        assert!(read_state(&tx).unwrap().epoch.is_some());
        tx.execute("INSERT INTO fixture_child VALUES(1)", [])
            .unwrap();
        assert!(tx.commit().is_err());
        assert_eq!(epoch(&store), None);
    }

    #[test]
    fn old_settings_and_session_imports_cannot_restore_a_revoked_epoch() {
        let root = tempfile::tempdir().unwrap();
        let mut store = store(root.path());
        let old_settings = store.cache.clone();
        store.set_mcp_execution_log_enabled(false);
        store.save().unwrap();
        let revoked = epoch(&store);
        store.cache = old_settings;
        store.save_all().unwrap();
        assert_eq!(epoch(&store), revoked);
        let session = super::super::tests::sample_session("fixture-import");
        store
            .import_json(&serde_json::json!({"sessions": [session]}).to_string())
            .unwrap();
        assert_eq!(epoch(&store), revoked);
        let mut candidate = store.cache.clone();
        candidate.sessions[0].name = "fixture native update".into();
        store.commit_session_changes(candidate).unwrap();
        assert_eq!(epoch(&store), revoked);
    }

    #[test]
    fn stale_opt_out_does_not_revoke_and_background_opt_out_does() {
        let root = tempfile::tempdir().unwrap();
        let mut store = store(root.path());
        let (cache, disk_fingerprint) =
            ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap();
        let saved = SavedState {
            disk_fingerprint,
            ..SavedState::of_cache(&cache)
        };
        let mut other = ConfigStore {
            path: store.path.clone(),
            backup_dir: None,
            cache,
            key: store.key,
            keyring_enabled: false,
            saved_state: Mutex::new(saved).into(),
        };
        store.cache.language = "newer".into();
        store.save().unwrap();
        other.set_mcp_execution_log_enabled(false);
        assert!(other.save().unwrap_err().is::<ConfigurationChanged>());
        assert_eq!(epoch(&store), None);
        store.set_mcp_execution_log_enabled(false);
        let sequence = {
            let mut saved = store.saved_state.lock().unwrap();
            saved.submitted += 1;
            saved.submitted
        };
        ConfigStore::run_background_save(
            store.cache.clone(),
            sequence,
            store.saved_state.clone(),
            store.key,
            store.path.clone(),
            None,
            false,
        );
        assert!(store.persistence_error().is_none());
        assert!(epoch(&store).is_some());
    }

    #[test]
    fn publication_callback_holds_the_cross_process_profile_lock() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        ConfigStore::with_execution_log_state(&store.path, |state| {
            assert!(state.enabled);
            let lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(root.path().join("sessions.db.profile-lock"))?;
            assert!(fs2::FileExt::try_lock_exclusive(&lock).is_err());
            Ok(())
        })
        .unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.path().join("sessions.db.profile-lock"))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    }
}
