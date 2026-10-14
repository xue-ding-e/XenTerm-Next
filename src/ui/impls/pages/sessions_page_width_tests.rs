//! Non-GUI regression tests for the completed-resize persistence boundary.
use super::*;
use std::path::Path;

fn fixture(width: f32) -> (tempfile::TempDir, ConfigStore) {
    let directory = tempfile::tempdir().unwrap();
    let mut cache = crate::config::ConfigFile::default();
    cache.session_list_width = width;
    let store = ConfigStore {
        path: directory.path().join("sessions.db"),
        backup_dir: None,
        cache,
        key: [7; 32],
        keyring_enabled: false,
        saved_state: std::sync::Mutex::new(crate::config::SavedState::default()).into(),
    };
    store.save().unwrap();
    (directory, store)
}

fn persisted_width(path: &Path) -> f64 {
    let connection = rusqlite::Connection::open(path).unwrap();
    let encoded: String = connection
        .query_row("SELECT value FROM meta WHERE key='settings'", [], |row| {
            row.get(0)
        })
        .unwrap();
    let settings: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    settings["session_list_width"].as_f64().unwrap()
}

#[test]
fn completed_resize_saves_the_new_width() {
    let (_directory, mut store) = fixture(380.0);
    persist_list_width(&mut store, 500.0).unwrap();
    assert_eq!(store.session_list_width(), 500.0);
    assert_eq!(persisted_width(&store.path), 500.0);
    // The UI and persistence share the setting's range, not two separate clamps.
    persist_list_width(&mut store, 900.0).unwrap();
    assert_eq!(store.session_list_width(), 640.0);
    assert_eq!(persisted_width(&store.path), 640.0);
}

#[test]
fn failed_resize_restores_the_previous_width_without_discarding_other_edits() {
    let (directory, mut store) = fixture(380.0);
    let saved_path = store.path.clone();
    // A directory cannot be opened as a SQLite database, on any supported OS.
    store.path = directory.path().to_path_buf();
    store.cache.nav_spacing = 12.0;
    assert!(persist_list_width(&mut store, 500.0).is_err());
    assert_eq!(store.cache.session_list_width, 380.0);
    assert_eq!(store.session_list_width(), 380.0);
    assert_eq!(
        store.cache.nav_spacing, 12.0,
        "rollback owns only the width"
    );
    assert_eq!(persisted_width(&saved_path), 380.0);
}

#[test]
fn failed_resize_preserves_the_unset_width_representation() {
    let (directory, mut store) = fixture(0.0);
    let saved_path = store.path.clone();
    assert_eq!(store.session_list_width(), 380.0);
    store.path = directory.path().to_path_buf();
    assert!(persist_list_width(&mut store, 500.0).is_err());
    // Restoring via set_session_list_width(0) would clamp it to 240 instead.
    assert_eq!(store.cache.session_list_width, 0.0);
    assert_eq!(store.session_list_width(), 380.0);
    assert_eq!(persisted_width(&saved_path), 0.0);
}

#[test]
fn a_failed_resize_can_be_retried_after_the_write_problem_is_removed() {
    let (directory, mut store) = fixture(380.0);
    let saved_path = store.path.clone();
    store.path = directory.path().to_path_buf();
    assert!(persist_list_width(&mut store, 500.0).is_err());
    assert_eq!(store.session_list_width(), 380.0);
    store.path = saved_path;
    persist_list_width(&mut store, 500.0).unwrap();
    assert_eq!(store.session_list_width(), 500.0);
    assert_eq!(persisted_width(&store.path), 500.0);
}
