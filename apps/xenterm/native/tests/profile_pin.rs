//! Regression: the bridge must honor `XENTERM_DATA_DIR`.
//!
//! The CLI resolves the explicit profile in `configure_profile` at startup
//! (crates/xenterm/src/main.rs); the bridge used to skip that step, so a GUI
//! process pointed at an isolated profile silently read/wrote the portable
//! `config/` directory beside the executable instead (found while running the
//! desktop app against a seeded profile: the home screen showed leftover
//! smoke sessions, not the seeded host).
//!
//! One test per binary: profile resolution is process-global (the store is a
//! `OnceLock`), so this file must stay the only user of the pinned profile in
//! its process. Everything goes through the same C ABI the Flutter app calls.

use std::ffi::{CStr, CString};

fn take(ptr: *mut std::ffi::c_char) -> String {
    assert!(!ptr.is_null(), "bridge returned null");
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    xenterm_bridge::xenterm_string_free(ptr);
    text
}

#[test]
fn bridge_pins_xenterm_data_dir() {
    let dir = std::env::temp_dir().join(format!("xenterm-bridge-pin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);

    // Force store initialization through the public ABI, then write a session.
    let listed = take(unsafe { xenterm_bridge::xenterm_sessions_json() });
    let rows: Vec<serde_json::Value> = serde_json::from_str(&listed).unwrap();
    assert!(
        rows.is_empty(),
        "fresh profile must start empty, got {rows:?}"
    );

    let payload = CString::new(
        r#"{"id":"pin-check","label":"pin-check","address":"127.0.0.1","port":2222,"username":"demo","auth":"password"}"#,
    )
    .unwrap();
    let saved = take(unsafe { xenterm_bridge::xenterm_session_upsert(payload.as_ptr()) });
    assert!(saved.contains("\"ok\":true"), "upsert failed: {saved}");

    // The write must land inside the pinned directory, not beside the test
    // binary (the old bug) and not in the per-user config dir.
    let db = dir.join("sessions.db");
    assert!(db.is_file(), "sessions.db must live in XENTERM_DATA_DIR");
    let bytes = std::fs::read(&db).unwrap_or_default();
    assert!(
        bytes.windows(b"pin-check".len()).any(|w| w == b"pin-check"),
        "pinned store must contain the saved session"
    );
}
