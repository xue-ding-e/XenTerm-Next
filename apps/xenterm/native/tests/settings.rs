//! Real settings C ABI against an owned temporary profile; no native keyring.
use std::ffi::{CStr, CString};

fn take(ptr: *mut std::ffi::c_char) -> serde_json::Value {
    assert!(!ptr.is_null());
    let text = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
    xenterm_bridge::xenterm_string_free(ptr);
    serde_json::from_str(&text).unwrap()
}
fn settings() -> serde_json::Value { take(xenterm_bridge::xenterm_settings_json()) }
fn save(value: serde_json::Value) -> serde_json::Value {
    let json = CString::new(value.to_string()).unwrap();
    take(xenterm_bridge::xenterm_settings_set(json.as_ptr()))
}

#[test]
fn failed_settings_save_keeps_bridge_projection_and_external_disk_unchanged() {
    let profile = std::env::temp_dir().join(format!("xenterm-settings-{}", std::process::id()));
    std::fs::create_dir(&profile).unwrap(); // refuse preexisting, never delete unknown data
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
    let _cleanup = Cleanup(profile.clone());
    std::env::set_var("XENTERM_DATA_DIR", &profile);
    assert_eq!(save(serde_json::json!({"bold": false, "download_dir": "original"}))["ok"], true);
    let before = settings();
    let mut external = xenterm::config::ConfigStore::load().unwrap();
    external.set_language("en".into());
    external.save().unwrap();
    let changed = serde_json::json!({
        "session_logs": !before["session_logs"].as_bool().unwrap(),
        "cursor_style": "bar", "line_spacing": 1.3,
        "padding": !before["padding"].as_bool().unwrap(), "bold": true,
        "sftp_follow_cd": !before["sftp_follow_cd"].as_bool().unwrap(),
        "download_dir": "rejected-change"
    });
    let result = save(changed);
    assert_eq!(result["ok"], false);
    assert!(result["error"].as_str().unwrap().contains("configuration changed"));
    assert_eq!(settings(), before);
    let disk = xenterm::config::ConfigStore::load().unwrap();
    assert_eq!(disk.language(), "en");
    assert_eq!(disk.download_dir(), "original");
    assert!(!disk.terminal_bold());
    assert_eq!(save(serde_json::json!({"padding": false}))["ok"], false);
    assert_eq!(settings(), before);
}
