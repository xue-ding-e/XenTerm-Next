//! Profile export/import round trip over the bridge, plus the encoding and
//! telnet surface added with it.
//!
//! Profile resolution is process-global (the store is a `OnceLock`), so every
//! phase lives in ONE test function, sequential against the same pinned dir.

use std::ffi::{CStr, CString};

fn take(ptr: *mut std::ffi::c_char) -> String {
    assert!(!ptr.is_null(), "bridge returned null");
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    xenterm_bridge::xenterm_string_free(ptr);
    text
}

fn call1(f: extern "C" fn(*const std::ffi::c_char) -> *mut std::ffi::c_char, arg: &str) -> String {
    let c = CString::new(arg).unwrap();
    take(f(c.as_ptr()))
}

fn sessions() -> serde_json::Value {
    serde_json::from_str(&take(xenterm_bridge::xenterm_sessions_json())).unwrap()
}

#[test]
fn profile_encoding_telnet_flow() {
    let dir = std::env::temp_dir().join(format!("xenterm-bridge-prof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);

    // ── Seed two SSH sessions.
    for (id, label, octet) in [("seed-a", "alpha", 1), ("seed-b", "beta", 2)] {
        let payload =
            format!(r#"{{"id":"{id}","label":"{label}","address":"10.0.0.{octet}","port":22}}"#);
        let saved = call1(xenterm_bridge::xenterm_session_upsert, &payload);
        assert!(saved.contains("\"ok\":true"), "{saved}");
    }

    // ── Export: the portable JSON, secrets obfuscated.
    let exported = take(xenterm_bridge::xenterm_profile_export());
    let export: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(export["ok"], true);
    assert_eq!(export["count"], 2);
    let json = export["json"].as_str().unwrap();
    assert!(json.contains("xenterm_export") || json.contains("meatshell_export"));

    // ── Dry-run a re-import: everything exists → all skipped, store intact.
    let dry = call1(
        xenterm_bridge::xenterm_profile_import,
        &format!(r#"{{"json":{},"dry_run":true}}"#, serde_json::json!(json)),
    );
    let dry: serde_json::Value = serde_json::from_str(&dry).unwrap();
    assert_eq!(dry["ok"], true);
    assert_eq!(dry["added"], 0);
    assert_eq!(dry["skipped"], 2);

    // ── Garbage input fails cleanly.
    let bad = call1(
        xenterm_bridge::xenterm_profile_import,
        r#"{"json":"{not json","dry_run":false}"#,
    );
    assert!(bad.contains("\"ok\":false"), "{bad}");

    // ── A GBK-encoded telnet host persists both attributes.
    let saved = call1(
        xenterm_bridge::xenterm_session_upsert,
        r#"{"id":"","label":"legacy-box","address":"10.0.0.9","port":23,"kind":"telnet","encoding":"gbk"}"#,
    );
    assert!(saved.contains("\"ok\":true"), "{saved}");
    let id: String = serde_json::from_str::<serde_json::Value>(&saved).unwrap()["id"]
        .as_str()
        .unwrap()
        .into();
    let rows = sessions().as_array().unwrap().clone();
    let row = rows.iter().find(|r| r["id"] == id.as_str()).unwrap();
    assert_eq!(row["kind"], "telnet");
    assert_eq!(row["encoding"], "gbk");

    // ── Re-save without the field: the stored encoding is kept.
    let again = call1(
        xenterm_bridge::xenterm_session_upsert,
        &format!(
            r#"{{"id":"{id}","label":"legacy-box","address":"10.0.0.9","port":23,"kind":"telnet"}}"#
        ),
    );
    assert!(again.contains("\"ok\":true"), "{again}");
    let rows = sessions().as_array().unwrap().clone();
    let row = rows.iter().find(|r| r["id"] == id.as_str()).unwrap();
    assert_eq!(row["encoding"], "gbk");

    // ── Runtime tunnels: no session open → an empty array, not an error.
    let tunnels = take(xenterm_bridge::xenterm_runtime_tunnels_json());
    assert_eq!(tunnels, "[]");
}
