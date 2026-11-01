//! OpenSSH config import through the bridge: parse → dedupe → persist.

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
        "xenterm-bridge-import-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);
    let mut args = vec!["xenterm-bridge-test".to_string()];
    xenterm::config::configure_profile(&mut args).unwrap();
    dir
}

fn import(text: &str) -> serde_json::Value {
    let payload = serde_json::json!({ "text": text }).to_string();
    let payload = CString::new(payload).unwrap();
    let raw = take(xenterm_bridge::xenterm_import_ssh_config(
        payload.as_ptr(),
    ));
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn ssh_config_import_adds_only_new_hosts() {
    let profile = temp_profile();
    const CONFIG: &str = "\
Host web
  HostName web.example.com
  User deploy
  Port 2222
  IdentityFile ~/.ssh/id_ed25519

Host db
  HostName db.internal
";

    let first = import(CONFIG);
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["added"], 2);
    assert_eq!(first["skipped"], 0);

    let sessions = take(xenterm_bridge::xenterm_sessions_json());
    let sessions: serde_json::Value = serde_json::from_str(&sessions).unwrap();
    let names: Vec<&str> = sessions
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["label"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"web"));
    assert!(names.contains(&"db"));
    let web = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["label"] == "web")
        .unwrap();
    assert_eq!(web["address"], "web.example.com");
    assert_eq!(web["username"], "deploy");
    assert_eq!(web["port"], 2222);
    assert!(web["private_key_path"]
        .as_str()
        .unwrap()
        .ends_with("/.ssh/id_ed25519"));
    assert_eq!(web["auth"], "key");

    // A second import finds nothing new.
    let again = import(CONFIG);
    assert_eq!(again["ok"], true);
    assert_eq!(again["added"], 0);
    assert_eq!(again["skipped"], 2);

    let _ = std::fs::remove_dir_all(profile);
}
