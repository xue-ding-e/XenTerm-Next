//! End-to-end check of the known-hosts C ABI against a temporary profile:
//! the core parses a real OpenSSH ed25519 line, the bridge projects its
//! fingerprint, and deletion rewrites the file.

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
fn known_hosts_list_and_forget() {
    let dir = std::env::temp_dir().join(format!(
        "xenterm-bridge-known-hosts-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);
    let mut args = vec!["xenterm-bridge-test".to_string()];
    xenterm::config::configure_profile(&mut args).unwrap();

    std::fs::write(
        dir.join("known_hosts"),
        "example.test:2222 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIK75sFE9+V5bYLYWAGR1USc9b+cIBq/9/OUGhh4p0vRr test\n",
    )
    .unwrap();

    let json = take(xenterm_bridge::xenterm_known_hosts_json());
    let rows: serde_json::Value = serde_json::from_str(&json).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "example.test:2222");
    assert_eq!(rows[0]["key_type"], "ssh-ed25519");
    assert!(rows[0]["fingerprint"]
        .as_str()
        .unwrap()
        .starts_with("SHA256:"));

    let id = CString::new("example.test:2222").unwrap();
    let deleted = take(xenterm_bridge::xenterm_known_host_delete(id.as_ptr()));
    let deleted: serde_json::Value = serde_json::from_str(&deleted).unwrap();
    assert_eq!(deleted["ok"], true);
    assert_eq!(deleted["removed"], true);

    let json = take(xenterm_bridge::xenterm_known_hosts_json());
    let rows: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 0);

    // Removing an absent entry is a no-op, not an error.
    let again = take(xenterm_bridge::xenterm_known_host_delete(id.as_ptr()));
    let again: serde_json::Value = serde_json::from_str(&again).unwrap();
    assert_eq!(again["ok"], true);
    assert_eq!(again["removed"], false);

    let _ = std::fs::remove_dir_all(dir);
}
