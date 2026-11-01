//! Jump-chain projection through the bridge: a saved SSH session points at a
//! jump host, the Tunnels & Jumps page sees the resolved chain, and clearing
//! the jump removes it.

use std::ffi::{CStr, CString};
use std::path::PathBuf;

fn take(ptr: *mut std::ffi::c_char) -> String {
    assert!(!ptr.is_null(), "bridge returned null");
    let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    xenterm_bridge::xenterm_string_free(ptr);
    text
}

fn c(value: &str) -> CString {
    CString::new(value).unwrap()
}

fn json(ptr: *mut std::ffi::c_char) -> serde_json::Value {
    serde_json::from_str(&take(ptr)).unwrap()
}

fn temp_profile() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xenterm-bridge-jump-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);
    let mut args = vec!["xenterm-bridge-test".to_string()];
    xenterm::config::configure_profile(&mut args).unwrap();
    dir
}

fn upsert(payload: serde_json::Value) -> String {
    let payload = c(&payload.to_string());
    let created = json(xenterm_bridge::xenterm_session_upsert(payload.as_ptr()));
    assert_eq!(created["ok"], true, "{created}");
    created["id"].as_str().unwrap().to_string()
}

#[test]
fn jump_chains_follow_the_saved_jump_host() {
    let profile = temp_profile();

    let bastion = upsert(serde_json::json!({
        "id": "",
        "label": "Bastion",
        "address": "10.0.0.1",
        "port": 22,
        "username": "root",
        "group": "",
        "kind": "ssh"
    }));
    let target = upsert(serde_json::json!({
        "id": "",
        "label": "Internal DB",
        "address": "10.0.0.9",
        "port": 22,
        "username": "root",
        "group": "",
        "kind": "ssh",
        "jump_session_id": bastion
    }));

    let chains = json(xenterm_bridge::xenterm_jump_chains_json());
    let chains = chains.as_array().unwrap();
    assert_eq!(chains.len(), 1);
    assert_eq!(chains[0]["session_id"], target);
    assert_eq!(chains[0]["session_name"], "Internal DB");
    assert_eq!(chains[0]["jumps"][0], "Bastion");

    // Clearing the selection removes the chain.
    let cleared = upsert(serde_json::json!({
        "id": target,
        "label": "Internal DB",
        "address": "10.0.0.9",
        "port": 22,
        "username": "root",
        "group": "",
        "kind": "ssh",
        "jump_session_id": ""
    }));
    assert_eq!(cleared, target);
    assert_eq!(
        json(xenterm_bridge::xenterm_jump_chains_json())
            .as_array()
            .unwrap()
            .len(),
        0
    );

    let _ = std::fs::remove_dir_all(profile);
}
