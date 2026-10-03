//! Deterministic synthetic-profile regressions for every persistence entry point.
//! Queued jobs are invoked directly in chosen orders, without threads or sleeps.
use super::super::tests::{fake_keyring, sample_session, temp_store, KEYRING_TESTS};
use super::*;

fn cleanup(store: &ConfigStore) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", store.path.display()));
    }
}

fn seeded_store() -> ConfigStore {
    let mut store = temp_store();
    store.cache.sessions = vec![sample_session("fixture-a"), sample_session("fixture-b")];
    store.cache.command_history = vec!["fixture initial command".into()];
    store.save().unwrap();
    store
}

/// Each independent editor owns its own baseline and sequence counters. Loading
/// the cache and fingerprint together is essential: a default SavedState would
/// test first-save rejection, rather than rejection of a genuinely stale load.
fn second_store(source: &ConfigStore) -> ConfigStore {
    let (mut cache, disk_fingerprint) =
        ConfigStore::read_cache_snapshot(&source.path, &source.key).unwrap();
    if source.keyring_enabled {
        // Headless builds intentionally do not resolve OS-keyring markers. The
        // fixture's in-memory builder is safe to resolve in either build mode.
        let connection = ConfigStore::open_db(&source.path).unwrap();
        let (_, sessions, _) = ConfigStore::read_disk_store(&connection).unwrap().unwrap();
        for session in sessions {
            if session.password.as_str() == ConfigStore::KEYRING_MARKER {
                cache
                    .sessions
                    .iter_mut()
                    .find(|s| s.id == session.id)
                    .unwrap()
                    .password = Secret::new(
                    fake_keyring::get(ConfigStore::KEYRING_SERVICE, &session.id).unwrap(),
                );
            }
        }
    }
    let saved = SavedState {
        disk_fingerprint,
        ..SavedState::of_cache(&cache)
    };
    ConfigStore {
        path: source.path.clone(),
        backup_dir: None,
        cache,
        key: source.key,
        keyring_enabled: source.keyring_enabled,
        saved_state: Arc::new(Mutex::new(saved)),
    }
}

fn cache_value(store: &ConfigStore) -> serde_json::Value {
    serde_json::to_value(&store.cache).unwrap()
}

/// Exclude generation/error bookkeeping, which must advance on a failed save.
fn baseline(store: &ConfigStore) -> serde_json::Value {
    let saved = store.saved_state.lock().unwrap();
    serde_json::json!({
        "settings": saved.settings,
        "order": saved.order,
        "sessions": saved.sessions,
        "history": saved.history,
        "fingerprint": saved.disk_fingerprint,
    })
}

fn raw_disk(store: &ConfigStore) -> serde_json::Value {
    let connection = ConfigStore::open_db(&store.path).unwrap();
    let rows = |query: &str, columns: usize| {
        let mut statement = connection.prepare(query).unwrap();
        let result = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|index| row.get::<_, String>(index))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        result
    };
    serde_json::json!({
        "meta": rows("SELECT key,value FROM meta ORDER BY key", 2),
        "sessions": rows("SELECT CAST(ordinal AS TEXT),id,data FROM sessions ORDER BY ordinal,id", 3),
        "history": rows("SELECT CAST(seq AS TEXT),command FROM command_history ORDER BY seq", 2),
    })
}

fn revision(store: &ConfigStore) -> String {
    ConfigStore::open_db(&store.path)
        .unwrap()
        .query_row(
            "SELECT value FROM meta WHERE key='write_revision'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn import_payload() -> String {
    serde_json::json!({"sessions": [sample_session("fixture-imported")]}).to_string()
}

fn assert_conflict(error: anyhow::Error, store: &ConfigStore) {
    assert!(error.is::<ConfigurationChanged>(), "{error:#}");
    assert!(store
        .persistence_error()
        .unwrap()
        .contains("changed elsewhere"));
}

fn mutate(store: &mut ConfigStore, kind: &str) {
    match kind {
        "session" => store.cache.sessions[0].name.push_str(" edited"),
        "settings" => store.cache.language = "fixture-language".into(),
        "order" => store.cache.sessions.reverse(),
        "delete" => {
            store.cache.sessions.remove(0);
        }
        "history" => store
            .cache
            .command_history
            .push("fixture newer command".into()),
        "noop" => {}
        _ => unreachable!(),
    }
}

fn check_stale_winner(winner_kind: &str) {
    for stale_kind in ["session", "settings", "order", "delete", "history", "noop"] {
        let mut winner = seeded_store();
        let mut stale = second_store(&winner);
        mutate(&mut winner, winner_kind);
        winner.save().unwrap();
        let disk_before = raw_disk(&winner);
        mutate(&mut stale, stale_kind);
        let cache_before = cache_value(&stale);
        let saved_before = baseline(&stale);
        assert_conflict(stale.save().unwrap_err(), &stale);
        assert_eq!(raw_disk(&winner), disk_before, "{winner_kind}/{stale_kind}");
        assert_eq!(
            cache_value(&stale),
            cache_before,
            "pending edits must survive"
        );
        assert_eq!(
            baseline(&stale),
            saved_before,
            "failed saves cannot acknowledge new disk state"
        );
        cleanup(&winner);
    }
}

#[test]
fn stale_session_writer_cannot_overwrite_any_newer_save() {
    check_stale_winner("session");
}
#[test]
fn stale_settings_writer_cannot_overwrite_any_newer_save() {
    check_stale_winner("settings");
}
#[test]
fn stale_order_writer_cannot_overwrite_any_newer_save() {
    check_stale_winner("order");
}
#[test]
fn stale_delete_writer_cannot_overwrite_any_newer_save() {
    check_stale_winner("delete");
}
#[test]
fn stale_history_writer_cannot_overwrite_any_newer_save() {
    check_stale_winner("history");
}

#[test]
fn successful_import_rejects_every_kind_of_older_save() {
    for kind in ["session", "settings", "order", "delete", "history", "noop"] {
        let mut importer = seeded_store();
        let mut stale = second_store(&importer);
        assert_eq!(importer.import_json(&import_payload()).unwrap(), (1, 0));
        let before = raw_disk(&importer);
        mutate(&mut stale, kind);
        assert_conflict(stale.save().unwrap_err(), &stale);
        assert_eq!(raw_disk(&importer), before);
        assert_eq!(second_store(&importer).cache.sessions.len(), 3);
        cleanup(&importer);
    }
}

#[test]
fn newer_ordinary_save_rejects_stale_import_without_mutating_its_cache() {
    for kind in ["session", "settings", "order", "delete", "history"] {
        let mut winner = seeded_store();
        let mut importer = second_store(&winner);
        let cache_before = cache_value(&importer);
        let saved_before = baseline(&importer);
        mutate(&mut winner, kind);
        winner.save().unwrap();
        let disk_before = raw_disk(&winner);
        assert_conflict(
            importer.import_json(&import_payload()).unwrap_err(),
            &importer,
        );
        assert_eq!(raw_disk(&winner), disk_before);
        assert_eq!(cache_value(&importer), cache_before);
        assert_eq!(baseline(&importer), saved_before);
        cleanup(&winner);
    }
}

#[test]
fn raw_sql_writers_are_detected_even_when_the_revision_token_is_unchanged() {
    for kind in ["session", "settings", "order", "delete", "history"] {
        let store = seeded_store();
        let mut stale = second_store(&store);
        let token = revision(&store);
        let connection = ConfigStore::open_db(&store.path).unwrap();
        match kind {
            "session" => {
                let mut session = store.cache.sessions[0].clone();
                session.name = "fixture legacy edit".into();
                connection
                    .execute(
                        "UPDATE sessions SET data=?1 WHERE id=?2",
                        rusqlite::params![serde_json::to_string(&session).unwrap(), session.id],
                    )
                    .unwrap();
            }
            "settings" => {
                let mut settings = store.cache.clone();
                settings.language = "fixture legacy language".into();
                connection
                    .execute(
                        "UPDATE meta SET value=?1 WHERE key='settings'",
                        [settings_blob(&settings)],
                    )
                    .unwrap();
            }
            "order" => {
                connection
                    .execute("UPDATE sessions SET ordinal=1-ordinal", [])
                    .unwrap();
            }
            "delete" => {
                connection
                    .execute(
                        "DELETE FROM sessions WHERE id=?1",
                        [&store.cache.sessions[0].id],
                    )
                    .unwrap();
            }
            "history" => {
                connection
                    .execute(
                        "INSERT INTO command_history(command) VALUES('fixture legacy command')",
                        [],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        drop(connection);
        assert_eq!(revision(&store), token);
        let before = raw_disk(&store);
        mutate(&mut stale, "settings");
        assert_conflict(stale.save().unwrap_err(), &stale);
        assert_eq!(raw_disk(&store), before, "{kind}");
        cleanup(&store);
    }
}

#[test]
fn raw_history_changes_outside_the_loaded_window_still_conflict() {
    let mut store = seeded_store();
    store.cache.command_history = (0..201).map(|i| format!("fixture command {i}")).collect();
    store.save().unwrap();
    let stale = second_store(&store);
    assert_eq!(stale.cache.command_history.len(), 200);
    let connection = ConfigStore::open_db(&store.path).unwrap();
    connection.execute("UPDATE command_history SET command='fixture changed oldest' WHERE seq=(SELECT MIN(seq) FROM command_history)", []).unwrap();
    drop(connection);
    assert_conflict(stale.save().unwrap_err(), &stale);
    cleanup(&store);
}

fn keyring_store() -> ConfigStore {
    let mut store = temp_store();
    store.keyring_enabled = true;
    let mut session = sample_session("fixture credential");
    session.password = Secret::new("fixture original password");
    store.cache.sessions.push(session);
    store.save().unwrap();
    store
}

#[test]
fn password_only_change_revisions_identical_keyring_marker_rows() {
    let _serial = KEYRING_TESTS.lock().unwrap();
    fake_keyring::install();
    fake_keyring::clear();
    let mut winner = keyring_store();
    let stale = second_store(&winner);
    assert_eq!(
        stale.cache.sessions[0].password.as_str(),
        "fixture original password"
    );
    let rows_before = raw_disk(&winner)["sessions"].clone();
    let token_before = revision(&winner);
    winner.cache.sessions[0].password = Secret::new("fixture newer password");
    winner.save().unwrap();
    assert_eq!(raw_disk(&winner)["sessions"], rows_before);
    assert_ne!(revision(&winner), token_before);
    let writes_before = fake_keyring::write_count();
    assert_conflict(stale.save().unwrap_err(), &stale);
    assert_eq!(fake_keyring::write_count(), writes_before);
    assert_eq!(
        fake_keyring::get(ConfigStore::KEYRING_SERVICE, &winner.cache.sessions[0].id).as_deref(),
        Some("fixture newer password")
    );
    cleanup(&winner);
}

#[test]
fn stale_update_clear_delete_and_editor_save_have_no_credential_side_effects() {
    let _serial = KEYRING_TESTS.lock().unwrap();
    fake_keyring::install();
    for kind in ["update", "clear", "delete", "editor"] {
        fake_keyring::clear();
        let mut winner = keyring_store();
        let mut stale = second_store(&winner);
        let id = winner.cache.sessions[0].id.clone();
        winner.cache.sessions[0].password = Secret::new("fixture winning password");
        winner.save().unwrap();
        let writes_before = fake_keyring::write_count();
        let disk_before = raw_disk(&winner);
        let saved_before = baseline(&stale);
        if kind == "editor" {
            let before = cache_value(&stale);
            let mut edited = stale.cache.sessions[0].clone();
            edited.password = Secret::new("fixture losing password");
            assert_conflict(stale.upsert_and_save(edited).unwrap_err(), &stale);
            assert_eq!(cache_value(&stale), before);
        } else {
            match kind {
                "update" | "clear" => {
                    let mut edited = stale.cache.sessions[0].clone();
                    edited.password = Secret::new(if kind == "clear" {
                        ""
                    } else {
                        "fixture losing password"
                    });
                    stale.upsert(edited);
                }
                "delete" => stale.remove(&id),
                _ => unreachable!(),
            }
            assert_eq!(
                fake_keyring::get(ConfigStore::KEYRING_SERVICE, &id).as_deref(),
                Some("fixture winning password"),
                "editing memory must not change credentials"
            );
            let before = cache_value(&stale);
            assert_conflict(stale.save().unwrap_err(), &stale);
            assert_eq!(cache_value(&stale), before);
        }
        assert_eq!(fake_keyring::write_count(), writes_before, "{kind}");
        assert_eq!(
            fake_keyring::get(ConfigStore::KEYRING_SERVICE, &id).as_deref(),
            Some("fixture winning password")
        );
        assert_eq!(raw_disk(&winner), disk_before);
        assert_eq!(baseline(&stale), saved_before);
        cleanup(&winner);
    }
}

fn reject_revision(store: &ConfigStore, failure: &str) -> rusqlite::Connection {
    let connection = ConfigStore::open_db(&store.path).unwrap();
    if failure == "DEFERRED" {
        // rusqlite's bundled SQLite enables foreign keys by default. A
        // deferred violation fails COMMIT, after SQL and keyring mutations.
        assert_eq!(
            connection
                .pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        connection.execute_batch(
            "CREATE TABLE fixture_parent (id INTEGER PRIMARY KEY);
             CREATE TABLE fixture_child (parent INTEGER REFERENCES fixture_parent(id) DEFERRABLE INITIALLY DEFERRED);
             CREATE TRIGGER fixture_reject_revision AFTER UPDATE ON meta WHEN NEW.key='write_revision'
             BEGIN INSERT INTO fixture_child(parent) VALUES(99); END;"
        ).unwrap();
    } else {
        assert!(matches!(failure, "ABORT" | "ROLLBACK"));
        connection.execute_batch(&format!(
            "CREATE TRIGGER fixture_reject_revision BEFORE UPDATE ON meta WHEN NEW.key='write_revision'
             BEGIN SELECT RAISE({failure}, 'fixture rejected revision'); END;"
        )).unwrap();
    }
    connection
}

fn check_generic_compensation(failure: &str) {
    let _serial = KEYRING_TESTS.lock().unwrap();
    fake_keyring::install();
    for kind in ["update", "clear", "delete", "insert"] {
        fake_keyring::clear();
        let mut store = keyring_store();
        let id = store.cache.sessions[0].id.clone();
        let disk_before = raw_disk(&store);
        let saved_before = baseline(&store);
        let connection = reject_revision(&store, failure);
        let mut inserted_id = None;
        match kind {
            "update" => store.cache.sessions[0].password = Secret::new("fixture changed password"),
            "clear" => store.cache.sessions[0].password = Secret::default(),
            "delete" => store.remove(&id),
            "insert" => {
                let mut session = sample_session("fixture added credential");
                session.password = Secret::new("fixture added password");
                inserted_id = Some(session.id.clone());
                store.upsert(session);
            }
            _ => unreachable!(),
        }
        let cache_before = cache_value(&store);
        let error = store.save().unwrap_err();
        assert!(
            !error.is::<ConfigurationChanged>(),
            "{failure}/{kind}: {error:#}"
        );
        assert!(
            !error.is::<SessionCredentialRollbackFailed>(),
            "{failure}/{kind}: {error:#}"
        );
        if failure == "DEFERRED" {
            assert!(format!("{error:#}").contains("failed to commit"));
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM fixture_child", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        assert_eq!(
            fake_keyring::get(ConfigStore::KEYRING_SERVICE, &id).as_deref(),
            Some("fixture original password"),
            "{failure}/{kind}"
        );
        if let Some(id) = &inserted_id {
            assert!(fake_keyring::get(ConfigStore::KEYRING_SERVICE, id).is_none());
        }
        assert_eq!(raw_disk(&store), disk_before);
        assert_eq!(
            cache_value(&store),
            cache_before,
            "pending generic edits must survive"
        );
        assert_eq!(baseline(&store), saved_before);
        assert!(store
            .persistence_error()
            .unwrap()
            .contains("could not be saved"));
        connection
            .execute_batch("DROP TRIGGER fixture_reject_revision")
            .unwrap();
        store.save().unwrap();
        assert!(store.persistence_error().is_none());
        let expected = match kind {
            "update" => Some("fixture changed password"),
            "clear" | "delete" => None,
            "insert" => Some("fixture original password"),
            _ => unreachable!(),
        };
        assert_eq!(
            fake_keyring::get(ConfigStore::KEYRING_SERVICE, &id).as_deref(),
            expected
        );
        if let Some(id) = &inserted_id {
            assert_eq!(
                fake_keyring::get(ConfigStore::KEYRING_SERVICE, id).as_deref(),
                Some("fixture added password")
            );
        }
        assert_ne!(baseline(&store), saved_before);
        drop(connection);
        cleanup(&store);
    }
}

#[test]
fn generic_save_compensates_credentials_after_abort() {
    check_generic_compensation("ABORT");
}
#[test]
fn generic_save_compensates_credentials_after_automatic_rollback() {
    check_generic_compensation("ROLLBACK");
}
#[test]
fn generic_save_compensates_credentials_after_deferred_commit_failure() {
    check_generic_compensation("DEFERRED");
}

#[test]
fn deleted_database_is_a_conflict_instead_of_an_implicit_full_rewrite() {
    let mut store = seeded_store();
    let saved_before = baseline(&store);
    cleanup(&store);
    mutate(&mut store, "settings");
    let cache_before = cache_value(&store);
    assert_conflict(store.save().unwrap_err(), &store);
    assert_eq!(
        raw_disk(&store),
        serde_json::json!({"meta": [], "sessions": [], "history": []})
    );
    assert_eq!(cache_value(&store), cache_before);
    assert_eq!(baseline(&store), saved_before);
    cleanup(&store);
}

#[test]
fn replaced_database_is_a_conflict_instead_of_an_implicit_full_rewrite() {
    let mut store = seeded_store();
    let saved_before = baseline(&store);
    let mut replacement = seeded_store();
    replacement.cache.language = "fixture replacement".into();
    replacement.save().unwrap();
    let replacement_disk = raw_disk(&replacement);
    cleanup(&store);
    fs::copy(&replacement.path, &store.path).unwrap();
    mutate(&mut store, "session");
    let cache_before = cache_value(&store);
    assert_conflict(store.save().unwrap_err(), &store);
    assert_eq!(raw_disk(&store), replacement_disk);
    assert_eq!(cache_value(&store), cache_before);
    assert_eq!(baseline(&store), saved_before);
    cleanup(&store);
    cleanup(&replacement);
}

#[test]
fn two_caches_of_an_empty_database_have_exactly_one_initial_writer() {
    for first_is_import in [false, true] {
        let mut first = temp_store();
        let mut second = second_store(&first);
        assert!(first.saved_state.lock().unwrap().disk_fingerprint.is_none());
        assert!(second
            .saved_state
            .lock()
            .unwrap()
            .disk_fingerprint
            .is_none());
        if first_is_import {
            first.import_json(&import_payload()).unwrap();
        } else {
            first.upsert(sample_session("fixture initial winner"));
            first.save().unwrap();
        }
        let before = raw_disk(&first);
        second.upsert(sample_session("fixture initial loser"));
        assert_conflict(second.save().unwrap_err(), &second);
        assert_eq!(raw_disk(&first), before);
        assert_eq!(second_store(&first).cache.sessions.len(), 1);
        cleanup(&first);
    }
}

fn queued_snapshot(store: &ConfigStore) -> (ConfigFile, u64) {
    let mut saved = store.saved_state.lock().unwrap();
    saved.submitted += 1;
    (store.cache.clone(), saved.submitted)
}

fn run_queued(store: &ConfigStore, (cache, sequence): (ConfigFile, u64)) {
    ConfigStore::run_background_save(
        cache,
        sequence,
        store.saved_state.clone(),
        store.key,
        store.path.clone(),
        None,
        store.keyring_enabled,
    );
}

#[test]
fn reversed_background_execution_keeps_the_latest_submitted_snapshot() {
    let mut store = seeded_store();
    store.cache.language = "fixture older".into();
    let older = queued_snapshot(&store);
    store.cache.language = "fixture newer".into();
    let newer = queued_snapshot(&store);
    let sequence = newer.1;
    run_queued(&store, newer);
    let before = raw_disk(&store);
    run_queued(&store, older);
    assert_eq!(raw_disk(&store), before);
    assert_eq!(second_store(&store).cache.language, "fixture newer");
    assert_eq!(store.saved_state.lock().unwrap().attempted, sequence);
    assert!(store.persistence_error().is_none());
    cleanup(&store);
}

#[test]
fn newer_foreground_save_cancels_an_older_queued_snapshot() {
    let mut store = seeded_store();
    store.cache.language = "fixture queued".into();
    let queued = queued_snapshot(&store);
    store.cache.language = "fixture foreground".into();
    store.save().unwrap();
    let before = raw_disk(&store);
    run_queued(&store, queued);
    assert_eq!(raw_disk(&store), before);
    assert_eq!(second_store(&store).cache.language, "fixture foreground");
    cleanup(&store);
}

#[test]
fn newer_foreground_noop_also_cancels_an_older_queued_snapshot() {
    let mut store = seeded_store();
    let original = store.cache.language.clone();
    let before = raw_disk(&store);
    store.cache.language = "fixture obsolete change".into();
    let queued = queued_snapshot(&store);
    store.cache.language = original.clone();
    store.save().unwrap();
    run_queued(&store, queued);
    assert_eq!(
        raw_disk(&store),
        before,
        "a no-op submission must supersede queued changes"
    );
    assert_eq!(second_store(&store).cache.language, original);
    cleanup(&store);
}

#[test]
fn toggling_back_to_the_original_value_still_saves_the_latest_snapshot() {
    for reversed in [false, true] {
        let mut store = seeded_store();
        let original = store.cache.language.clone();
        store.cache.language = "fixture temporary value".into();
        let changed = queued_snapshot(&store);
        store.cache.language = original.clone();
        let reverted = queued_snapshot(&store);
        let sequence = reverted.1;
        if reversed {
            run_queued(&store, reverted);
            run_queued(&store, changed);
        } else {
            run_queued(&store, changed);
            run_queued(&store, reverted);
        }
        assert_eq!(second_store(&store).cache.language, original);
        assert_eq!(store.saved_state.lock().unwrap().attempted, sequence);
        assert!(
            ConfigStore::plan_save(&store.cache, &store.saved_state.lock().unwrap()).is_empty()
        );
        cleanup(&store);
    }
}

#[test]
fn failed_newer_background_attempt_cancels_older_jobs_and_allows_latest_retry() {
    let mut store = seeded_store();
    let disk_before = raw_disk(&store);
    let saved_before = baseline(&store);
    store.cache.language = "fixture older".into();
    let older = queued_snapshot(&store);
    store.cache.language = "fixture latest".into();
    let newer = queued_snapshot(&store);
    let sequence = newer.1;
    let connection = reject_revision(&store, "ABORT");
    run_queued(&store, newer);
    assert_eq!(raw_disk(&store), disk_before);
    assert_eq!(baseline(&store), saved_before);
    assert_eq!(store.cache.language, "fixture latest");
    assert_eq!(store.saved_state.lock().unwrap().attempted, sequence);
    let error = store.persistence_error();
    assert!(error.is_some());
    connection
        .execute_batch("DROP TRIGGER fixture_reject_revision")
        .unwrap();
    run_queued(&store, older);
    assert_eq!(raw_disk(&store), disk_before);
    assert_eq!(store.persistence_error(), error);
    store.save().unwrap();
    assert_eq!(second_store(&store).cache.language, "fixture latest");
    assert!(store.persistence_error().is_none());
    drop(connection);
    cleanup(&store);
}

#[test]
fn pending_edits_reject_import_and_cancel_older_queued_saves_until_explicit_retry() {
    let mut store = seeded_store();
    let disk_before = raw_disk(&store);
    let saved_before = baseline(&store);
    store.cache.sessions[0].name = "fixture pending edit".into();
    let queued = queued_snapshot(&store);
    let pending = cache_value(&store);
    let error = store.import_json(&import_payload()).unwrap_err();
    assert!(format!("{error:#}").contains("pending configuration changes"));
    assert_eq!(cache_value(&store), pending);
    assert_eq!(baseline(&store), saved_before);
    run_queued(&store, queued);
    assert_eq!(raw_disk(&store), disk_before);
    assert_eq!(cache_value(&store), pending);
    store.save().unwrap();
    assert_eq!(store.import_json(&import_payload()).unwrap(), (1, 0));
    assert_eq!(
        second_store(&store).cache.sessions[0].name,
        "fixture pending edit"
    );
    assert_eq!(second_store(&store).cache.sessions.len(), 3);
    assert!(store.persistence_error().is_none());
    cleanup(&store);
}

#[test]
fn successful_import_cancels_an_older_queued_baseline_snapshot() {
    let mut store = seeded_store();
    let queued = queued_snapshot(&store);
    assert_eq!(store.import_json(&import_payload()).unwrap(), (1, 0));
    let before = raw_disk(&store);
    run_queued(&store, queued);
    assert_eq!(raw_disk(&store), before);
    assert_eq!(second_store(&store).cache.sessions.len(), 3);
    assert!(store.persistence_error().is_none());
    cleanup(&store);
}

const CHILD_FIXTURE_DIR: &str = "XENTERM_OCC_TEST_FIXTURE_DIR";
const CHILD_EXPECTED_SESSION: &str = "XENTERM_OCC_TEST_EXPECTED_SESSION";
const CHILD_TEST: &str = "config::config::persistence::tests::subprocess_stale_writer_fixture";
const BARRIER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

struct FixtureDirectory(PathBuf);

impl FixtureDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("xenterm-occ-subprocess-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("fixture-only"), b"synthetic OCC regression").unwrap();
        Self(path)
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct FixtureChild(std::process::Child);

impl Drop for FixtureChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for_signal(path: &Path) {
    let deadline = std::time::Instant::now() + BARRIER_TIMEOUT;
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "fixture barrier timed out: {}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn wait_for_child(child: &mut FixtureChild, ready: Option<&Path>) -> bool {
    let deadline = std::time::Instant::now() + BARRIER_TIMEOUT;
    loop {
        if ready.is_some_and(Path::exists) {
            return true;
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            return ready.is_none() && status.success();
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Invoked only by the two parent regressions below. During an ordinary full
/// suite run it does nothing, and does not contribute an ignored test.
#[test]
fn subprocess_stale_writer_fixture() {
    let Some(directory) = std::env::var_os(CHILD_FIXTURE_DIR) else {
        return;
    };
    let directory = PathBuf::from(directory);
    assert_eq!(
        fs::read(directory.join("fixture-only")).unwrap(),
        b"synthetic OCC regression"
    );
    let expected = std::env::var(CHILD_EXPECTED_SESSION).unwrap();
    let mut source = temp_store();
    source.path = directory.join("sessions.db");
    let mut stale = second_store(&source);
    let saved_before = baseline(&stale);
    // Both processes have now captured the same disk baseline. No process
    // can win its save until this file is visible to the parent.
    fs::write(directory.join("child-loaded"), b"ready").unwrap();
    wait_for_signal(&directory.join("parent-committed"));
    stale.cache.language = "fixture stale child edit".into();
    let pending = cache_value(&stale);
    let winning_disk = raw_disk(&stale);
    assert_conflict(stale.save().unwrap_err(), &stale);
    assert_eq!(raw_disk(&stale), winning_disk);
    assert_eq!(cache_value(&stale), pending);
    assert_eq!(baseline(&stale), saved_before);

    // A fresh cache can acknowledge the winner and make a subsequent save.
    let mut reloaded = second_store(&stale);
    assert!(reloaded
        .cache
        .sessions
        .iter()
        .any(|session| session.name == expected));
    reloaded.cache.language = "fixture child retry".into();
    reloaded.save().unwrap();
    assert!(reloaded.persistence_error().is_none());
    fs::write(directory.join("child-verified"), b"done").unwrap();
}

fn subprocess_conflict(initially_empty: bool) {
    let directory = FixtureDirectory::new();
    let mut source = temp_store();
    source.path = directory.0.join("sessions.db");
    if !initially_empty {
        let mut session = sample_session("fixture loaded baseline");
        session.password = Secret::new("fixture file-encrypted password");
        source.upsert(session);
        source.save().unwrap();
    }
    let mut parent = second_store(&source);
    let expected = if initially_empty {
        "fixture first writer"
    } else {
        "fixture imported winner"
    };
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_FIXTURE_DIR, &directory.0)
        .env(CHILD_EXPECTED_SESSION, expected)
        .stdin(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut child = FixtureChild(child);
    assert!(
        wait_for_child(&mut child, Some(&directory.0.join("child-loaded"))),
        "child did not capture the baseline"
    );
    if initially_empty {
        parent.upsert(sample_session(expected));
        parent.save().unwrap();
    } else {
        let payload = serde_json::json!({"sessions": [sample_session(expected)]}).to_string();
        assert_eq!(parent.import_json(&payload).unwrap(), (1, 0));
    }
    fs::write(directory.0.join("parent-committed"), b"committed").unwrap();
    assert!(
        wait_for_child(&mut child, None),
        "child process failed or exceeded its bounded deadline"
    );
    assert!(directory.0.join("child-verified").is_file());
    let reloaded = second_store(&parent);
    assert_eq!(reloaded.cache.language, "fixture child retry");
    assert!(reloaded
        .cache
        .sessions
        .iter()
        .any(|session| session.name == expected));
    assert_eq!(
        reloaded.cache.sessions.len(),
        if initially_empty { 1 } else { 2 }
    );
    if !initially_empty {
        assert_eq!(
            reloaded.cache.sessions[0].password.as_str(),
            "fixture file-encrypted password"
        );
    }
}

#[test]
fn separate_processes_racing_to_initialize_an_empty_profile_have_one_winner() {
    subprocess_conflict(true);
}

#[test]
fn separate_process_save_rejects_a_newer_import_and_succeeds_after_reload() {
    subprocess_conflict(false);
}

#[test]
fn failed_credential_compensation_blocks_every_writer_until_a_fresh_load() {
    let _serial = KEYRING_TESTS.lock().unwrap();
    fake_keyring::install();
    for editor_save in [false, true] {
        fake_keyring::clear();
        let mut store = keyring_store();
        let original_cache = store.cache.clone();
        let id = store.cache.sessions[0].id.clone();
        let disk_before = raw_disk(&store);
        let saved_before = baseline(&store);
        let connection = reject_revision(&store, "ABORT");
        // The attempted edit reaches the fake provider; its compensating
        // write fails after SQLite rejects the commit's revision update.
        fake_keyring::fail_writes_after(1);
        let mut edited = store.cache.sessions[0].clone();
        edited.password = Secret::new("fixture incompletely rolled back password");
        let error = if editor_save {
            store.upsert_and_save(edited).unwrap_err()
        } else {
            store.upsert(edited);
            store.save().unwrap_err()
        };
        assert!(error.is::<SessionCredentialRollbackFailed>(), "{error:#}");
        assert!(store.saved_state.lock().unwrap().credentials_uncertain);
        assert_eq!(baseline(&store), saved_before);
        assert_eq!(raw_disk(&store), disk_before);
        assert_eq!(
            fake_keyring::get(ConfigStore::KEYRING_SERVICE, &id).as_deref(),
            Some("fixture incompletely rolled back password")
        );
        let retained_error = store.persistence_error();
        assert!(retained_error
            .as_deref()
            .unwrap()
            .contains("credential recovery"));
        if editor_save {
            assert_eq!(
                cache_value(&store),
                serde_json::to_value(&original_cache).unwrap()
            );
        } else {
            assert_eq!(
                store.cache.sessions[0].password.as_str(),
                "fixture incompletely rolled back password"
            );
        }

        // Restore the provider and remove the SQL fault. Every rejection
        // below must now come from the shared uncertain-credential state.
        fake_keyring::fail_writes_after(usize::MAX - fake_keyring::write_count());
        connection
            .execute_batch("DROP TRIGGER fixture_reject_revision")
            .unwrap();
        let writes_before = fake_keyring::write_count();
        for kind in ["noop", "noop", "settings", "history", "import"] {
            // An editor save already restored its cache. A generic caller
            // can also discard its pending edit, but that must not clear the
            // persistence warning or make an unchanged save count as success.
            store.cache = original_cache.clone();
            if kind != "import" {
                mutate(&mut store, kind);
            }
            let pending = cache_value(&store);
            let error = if kind == "import" {
                store.import_json(&import_payload()).unwrap_err()
            } else {
                store.save().unwrap_err()
            };
            assert!(
                error.is::<SessionCredentialRollbackFailed>(),
                "{kind}: {error:#}"
            );
            assert!(store.saved_state.lock().unwrap().credentials_uncertain);
            assert_eq!(store.persistence_error(), retained_error, "{kind}");
            assert_eq!(cache_value(&store), pending);
            assert_eq!(baseline(&store), saved_before);
            assert_eq!(raw_disk(&store), disk_before);
            assert_eq!(fake_keyring::write_count(), writes_before, "{kind}");
        }
        drop(connection);

        let mut reloaded = second_store(&store);
        assert!(!reloaded.saved_state.lock().unwrap().credentials_uncertain);
        assert!(reloaded.persistence_error().is_none());
        reloaded.save().unwrap();
        mutate(&mut reloaded, "settings");
        reloaded.save().unwrap();
        mutate(&mut reloaded, "history");
        reloaded.save().unwrap();
        assert_eq!(reloaded.import_json(&import_payload()).unwrap(), (1, 0));
        assert!(reloaded.persistence_error().is_none());
        // Loading another editor cannot silently unpoison the old one.
        assert!(store.saved_state.lock().unwrap().credentials_uncertain);
        assert_eq!(store.persistence_error(), retained_error);
        cleanup(&store);
    }
}
