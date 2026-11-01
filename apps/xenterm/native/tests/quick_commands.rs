//! End-to-end check of the snippets (quick commands) C ABI against a temporary
//! profile: create → list → update → delete, all persisted through the core's
//! ConfigStore.

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
        "xenterm-bridge-quick-commands-{}",
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
fn quick_commands_round_trip() {
    let profile = temp_profile();
    let list = || {
        let json = take(xenterm_bridge::xenterm_quick_commands_json());
        serde_json::from_str::<serde_json::Value>(&json).unwrap()
    };
    assert_eq!(list().as_array().unwrap().len(), 0);

    let create = CString::new(
        r#"{"index":-1,"name":"Reload nginx","command":"sudo nginx -s reload","group":"ops","send_enter":true}"#,
    )
    .unwrap();
    let created = take(xenterm_bridge::xenterm_quick_command_upsert(create.as_ptr()));
    let created: serde_json::Value = serde_json::from_str(&created).unwrap();
    assert_eq!(created["ok"], true);
    assert_eq!(created["index"], 0);

    let rows = list();
    assert_eq!(rows[0]["name"], "Reload nginx");
    assert_eq!(rows[0]["send_enter"], true);

    let update = CString::new(
        r#"{"index":0,"name":"Reload nginx","command":"nginx -s reload","group":"ops","send_enter":false}"#,
    )
    .unwrap();
    let updated = take(xenterm_bridge::xenterm_quick_command_upsert(update.as_ptr()));
    let updated: serde_json::Value = serde_json::from_str(&updated).unwrap();
    assert_eq!(updated["ok"], true);

    let rows = list();
    assert_eq!(rows[0]["command"], "nginx -s reload");
    assert_eq!(rows[0]["send_enter"], false);

    let deleted = take(xenterm_bridge::xenterm_quick_command_delete(0));
    let deleted: serde_json::Value = serde_json::from_str(&deleted).unwrap();
    assert_eq!(deleted["ok"], true);
    assert_eq!(list().as_array().unwrap().len(), 0);

    // A failed delete is reported instead of panicking.
    let missing = take(xenterm_bridge::xenterm_quick_command_delete(3));
    let missing: serde_json::Value = serde_json::from_str(&missing).unwrap();
    assert_eq!(missing["ok"], false);

    // The empty command is rejected before touching the store.
    let empty = CString::new(r#"{"name":"x","command":"  "}"#).unwrap();
    let rejected = take(xenterm_bridge::xenterm_quick_command_upsert(
        empty.as_ptr(),
    ));
    let rejected: serde_json::Value = serde_json::from_str(&rejected).unwrap();
    assert_eq!(rejected["ok"], false);

    let _ = std::fs::remove_dir_all(profile);
}
