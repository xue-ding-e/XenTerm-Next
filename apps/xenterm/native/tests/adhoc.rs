//! Temporary local sessions: the bridge starts them without creating a saved
//! host, and the handle can be closed like any other terminal.

use std::ffi::{CStr, CString};
use std::path::PathBuf;

fn take(ptr: *mut std::ffi::c_char) -> String {
    assert!(!ptr.is_null(), "bridge returned null");
    let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    xenterm_bridge::xenterm_string_free(ptr);
    text
}

fn temp_profile() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xenterm-bridge-adhoc-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);
    let mut args = vec!["xenterm-bridge-test".to_string()];
    xenterm::config::configure_profile(&mut args).unwrap();
    dir
}

#[test]
fn a_local_tab_starts_without_creating_a_saved_host() {
    let profile = temp_profile();
    let payload = CString::new(
        r#"{"label":"Local terminal","kind":"local","address":"","username":"","cols":80,"rows":24}"#,
    )
    .unwrap();
    let raw = take(xenterm_bridge::xenterm_session_start_adhoc(
        payload.as_ptr(),
    ));
    let started: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(started["ok"], true, "{started}");
    let handle = started["handle"].as_u64().unwrap();
    assert!(handle > 0);

    // No session row was written.
    let sessions = take(xenterm_bridge::xenterm_sessions_json());
    let sessions: serde_json::Value = serde_json::from_str(&sessions).unwrap();
    assert_eq!(sessions.as_array().unwrap().len(), 0);

    // The snapshot is a live local shell (it eventually reports rows/cols).
    std::thread::sleep(std::time::Duration::from_millis(300));
    let snapshot = take(xenterm_bridge::xenterm_session_snapshot(handle));
    let snapshot: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(snapshot["cols"], 80);
    assert_eq!(snapshot["rowCount"], 24);

    assert_eq!(xenterm_bridge::xenterm_session_close(handle), 1);
    let _ = std::fs::remove_dir_all(profile);
}
