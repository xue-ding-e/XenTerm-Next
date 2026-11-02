//! Synthetic stores only: no global profile, keyring, or user settings.
use super::*;

fn fixture() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore {
        path: dir.path().join("sessions.db"),
        backup_dir: None,
        cache: ConfigFile::default(),
        key: [7; 32],
        keyring_enabled: false,
        saved_state: std::sync::Mutex::new(SavedState::default()).into(),
    };
    store.save().unwrap();
    (dir, store)
}

fn all_fields() -> SettingsUpdate {
    SettingsUpdate {
        session_logs: Some(true), cursor_style: Some("bar".into()),
        line_spacing: Some(1.3), padding: Some(false), bold: Some(true),
        sftp_follow_cd: Some(false), download_dir: Some("failed-request".into()),
    }
}

#[test]
fn conflict_restores_raw_settings_and_prior_dirty_state_without_changing_baseline() {
    let (_dir, mut store) = fixture();
    let (cache, fingerprint) = ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap();
    let mut other = ConfigStore {
        path: store.path.clone(), backup_dir: None, key: store.key,
        keyring_enabled: false,
        saved_state: std::sync::Mutex::new(SavedState {
            disk_fingerprint: fingerprint, ..SavedState::of_cache(&cache)
        }).into(), cache,
    };
    other.set_language("en".into());
    other.save().unwrap();
    // These raw legacy values would be lost by a getter/setter-based rollback.
    store.cache.terminal_cursor_style = "legacy-shape".into();
    store.cache.terminal_line_spacing = 0.0;
    store.cache.download_dir = "prior-unsaved".into();
    store.cache.command_history.push("synthetic pending history".into());
    let before = serde_json::to_value(&store.cache).unwrap();
    let baseline = store.saved_state.lock().unwrap().clone();
    let error = store.update_settings_and_save(all_fields()).unwrap_err();
    assert!(error.downcast_ref::<ConfigurationChanged>().is_some());
    assert_eq!(serde_json::to_value(&store.cache).unwrap(), before);
    let after = store.saved_state.lock().unwrap().clone();
    assert_eq!(after.settings, baseline.settings);
    assert_eq!(after.history, baseline.history);
    assert_eq!(after.disk_fingerprint, baseline.disk_fingerprint);
    assert_eq!(after.submitted, baseline.submitted + 1);
    assert_eq!(after.attempted, after.submitted);
    assert!(after.error.is_some());
    assert!(!ConfigStore::plan_save(&store.cache, &after).is_empty());
    assert_eq!(ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap().0.language, "en");
    // A normal retry must retain CAS, not silently overwrite the external writer.
    assert!(store.save().unwrap_err().downcast_ref::<ConfigurationChanged>().is_some());
}

#[test]
fn storage_failure_does_not_leak_failed_values_into_a_later_save() {
    let (_dir, mut store) = fixture();
    store.set_language("en".into()); // independent pending edit
    let before = serde_json::to_value(&store.cache).unwrap();
    // Capture an older queued job, then execute it only after the failed request.
    // This uses the real worker deterministically, without scheduling assumptions.
    let queued_cache = store.cache.clone();
    let queued_sequence = {
        let mut saved = store.saved_state.lock().unwrap();
        saved.submitted += 1;
        saved.submitted
    };
    let held = store.path.with_extension("held");
    fs::rename(&store.path, &held).unwrap();
    fs::create_dir(&store.path).unwrap(); // deterministic open failure, even as root
    assert!(store.update_settings_and_save(all_fields()).is_err());
    assert_eq!(serde_json::to_value(&store.cache).unwrap(), before);
    fs::remove_dir(&store.path).unwrap();
    fs::rename(&held, &store.path).unwrap();
    let disk_before_worker = ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap().0;
    let attempted = store.saved_state.lock().unwrap().attempted;
    assert!(attempted > queued_sequence);
    ConfigStore::run_background_save(
        queued_cache,
        queued_sequence,
        store.saved_state.clone(),
        store.key,
        store.path.clone(),
        None,
        false,
    );
    assert_eq!(store.saved_state.lock().unwrap().attempted, attempted);
    assert_eq!(
        serde_json::to_value(ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap().0).unwrap(),
        serde_json::to_value(disk_before_worker).unwrap(),
    );
    assert_eq!(serde_json::to_value(&store.cache).unwrap(), before);
    store.save().unwrap();
    let disk = ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap().0;
    assert_eq!(serde_json::to_value(disk).unwrap(), before);
    assert!(store.persistence_error().is_none());
}

#[test]
fn successful_partial_update_keeps_omitted_raw_fields_and_advances_baseline() {
    let (_dir, mut store) = fixture();
    store.cache.terminal_cursor_style = "legacy-shape".into();
    store.cache.terminal_line_spacing = 0.0;
    store.update_settings_and_save(SettingsUpdate {
        bold: Some(true), ..SettingsUpdate::default()
    }).unwrap();
    assert!(store.terminal_bold());
    assert_eq!(store.cache.terminal_cursor_style, "legacy-shape");
    assert_eq!(store.cache.terminal_line_spacing, 0.0);
    let saved = store.saved_state.lock().unwrap();
    assert!(ConfigStore::plan_save(&store.cache, &saved).is_empty());
    assert!(saved.error.is_none());
}
