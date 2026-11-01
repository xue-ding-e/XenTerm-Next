//! End-to-end check of the port-forward C ABI: create an SSH session, add a
//! rule (validated by the same core rules the desktop editor uses), list it,
//! and delete it from the store.

use std::ffi::{CStr, CString};

fn take(ptr: *mut std::ffi::c_char) -> String {
    assert!(!ptr.is_null(), "bridge returned null");
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    xenterm_bridge::xenterm_string_free(ptr);
    text
}

fn json(ptr: *mut std::ffi::c_char) -> serde_json::Value {
    serde_json::from_str(&take(ptr)).unwrap()
}

#[test]
fn forwards_round_trip() {
    let dir = std::env::temp_dir().join(format!(
        "xenterm-bridge-forwards-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);
    let mut args = vec!["xenterm-bridge-test".to_string()];
    xenterm::config::configure_profile(&mut args).unwrap();

    let session = CString::new(
        r#"{"id":"","label":"gw","address":"10.0.0.2","port":22,"username":"root","group":"","kind":"ssh"}"#,
    )
    .unwrap();
    let created = json(xenterm_bridge::xenterm_session_upsert(
        session.as_ptr(),
    ));
    assert_eq!(created["ok"], true);
    let session_id = created["id"].as_str().unwrap().to_string();

    let rule = serde_json::json!({
        "session_id": session_id,
        "index": -1,
        "kind": "local",
        "name": "DB",
        "bind_addr": "127.0.0.1",
        "bind_port": 15432,
        "host": "10.0.0.5",
        "host_port": 5432,
    });
    let rule = CString::new(rule.to_string()).unwrap();
    let saved = json(xenterm_bridge::xenterm_forward_upsert(rule.as_ptr()));
    assert_eq!(saved["ok"], true);
    assert_eq!(saved["index"], 0);

    let rows = json(xenterm_bridge::xenterm_forwards_json());
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["session_name"], "gw");
    assert_eq!(rows[0]["kind"], "local");
    assert_eq!(rows[0]["bind_port"], 15432);
    assert_eq!(rows[0]["host"], "10.0.0.5");
    assert_eq!(rows[0]["host_port"], 5432);

    // A dynamic rule drops its target and defaults the bind address.
    let dynamic = serde_json::json!({
        "session_id": session_id,
        "index": -1,
        "kind": "dynamic",
        "name": "SOCKS",
        "bind_addr": "",
        "bind_port": "1080",
        "host": "ignored",
        "host_port": "1",
    });
    let dynamic = CString::new(dynamic.to_string()).unwrap();
    let saved = json(xenterm_bridge::xenterm_forward_upsert(dynamic.as_ptr()));
    assert_eq!(saved["ok"], true);
    let rows = json(xenterm_bridge::xenterm_forwards_json());
    assert_eq!(rows[1]["bind_addr"], "127.0.0.1");
    assert_eq!(rows[1]["host"], "");
    assert_eq!(rows[1]["host_port"], 0);

    // Invalid ports are refused before the store is touched.
    let bad = serde_json::json!({
        "session_id": session_id,
        "index": -1,
        "kind": "local",
        "bind_port": 70000,
        "host": "10.0.0.5",
        "host_port": 5432,
    });
    let bad = CString::new(bad.to_string()).unwrap();
    let rejected = json(xenterm_bridge::xenterm_forward_upsert(bad.as_ptr()));
    assert_eq!(rejected["ok"], false);

    let reference = serde_json::json!({"session_id": session_id, "index": 0});
    let reference = CString::new(reference.to_string()).unwrap();
    let deleted = json(xenterm_bridge::xenterm_forward_delete(
        reference.as_ptr(),
    ));
    assert_eq!(deleted["ok"], true);
    let rows = json(xenterm_bridge::xenterm_forwards_json());
    assert_eq!(rows.as_array().unwrap().len(), 1);

    let _ = std::fs::remove_dir_all(dir);
}
