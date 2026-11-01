//! Key-material lifecycle over the bridge: pasted/imported content is stored
//! as `private_key_inline` (never echoed back), auth flips to Key, and
//! `clear_private_key` drops both the inline material and the path.

use std::ffi::{CStr, CString};

fn take(ptr: *mut std::ffi::c_char) -> String {
    assert!(!ptr.is_null(), "bridge returned null");
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    xenterm_bridge::xenterm_string_free(ptr);
    text
}

fn upsert(payload: &str) -> String {
    let c = CString::new(payload).unwrap();
    take(unsafe { xenterm_bridge::xenterm_session_upsert(c.as_ptr()) })
}

fn sessions() -> serde_json::Value {
    serde_json::from_str(&take(unsafe { xenterm_bridge::xenterm_sessions_json() })).unwrap()
}

#[test]
fn inline_key_round_trip_without_leaking_material() {
    let dir = std::env::temp_dir().join(format!("xenterm-bridge-key-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);

    // Create with pasted key material: auth becomes Key. The core owns id
    // generation, so later updates address the returned id.
    let saved = upsert(
        r#"{"id":"","label":"keyed","address":"10.0.0.3","port":22,"username":"root",
            "private_key_inline":"-----BEGIN OPENSSH PRIVATE KEY-----xyz"}"#,
    );
    assert!(saved.contains("\"ok\":true"), "{saved}");
    let id: String = serde_json::from_str::<serde_json::Value>(&saved).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let update = format!(
        r#"{{"id":"{id}","label":"keyed","address":"10.0.0.3","port":22,"username":"root"}}"#
    );
    let clear = format!(
        r#"{{"id":"{id}","label":"keyed","address":"10.0.0.3","port":22,"username":"root",
            "clear_private_key":true}}"#
    );
    let rows = sessions().as_array().unwrap().clone();
    let row = rows.iter().find(|r| r["id"] == id.as_str()).unwrap();
    assert_eq!(row["auth"], "key");
    assert_eq!(row["has_inline_key"], true);
    // The material itself never crosses the bridge in either direction.
    assert!(row.get("private_key_inline").is_none());

    // Saving again with a blank inline field keeps the stored key (the
    // projection still reports it) and stays on Key auth.
    let kept = upsert(&update);
    assert!(kept.contains("\"ok\":true"), "{kept}");
    let rows = sessions().as_array().unwrap().clone();
    let row = rows.iter().find(|r| r["id"] == id.as_str()).unwrap();
    assert_eq!(row["has_inline_key"], true);
    assert_eq!(row["auth"], "key");

    // clear_private_key drops it and returns the host to Password auth.
    let cleared = upsert(&clear);
    assert!(cleared.contains("\"ok\":true"), "{cleared}");
    let rows = sessions().as_array().unwrap().clone();
    let row = rows.iter().find(|r| r["id"] == id.as_str()).unwrap();
    assert_eq!(row["has_inline_key"], false);
    assert_eq!(row["auth"], "password");
    assert_eq!(row["private_key_path"], "");
}
