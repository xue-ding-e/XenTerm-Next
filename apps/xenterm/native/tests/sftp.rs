//! SFTP ABI plumbing without a server: opening a channel to a closed port must
//! produce a real snapshot that ends in an error/closed state, and the handle
//! must be releasable. Live listings need an SSH server and are covered by the
//! UI contract tests plus manual verification.

use std::ffi::{CStr, CString};
use std::time::{Duration, Instant};

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
fn sftp_failure_is_reported_and_closeable() {
    let dir = std::env::temp_dir().join(format!(
        "xenterm-bridge-sftp-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);
    let mut args = vec!["xenterm-bridge-test".to_string()];
    xenterm::config::configure_profile(&mut args).unwrap();

    let session = CString::new(
        r#"{"id":"","label":"unreachable","address":"127.0.0.1","port":1,"username":"root","group":"","kind":"ssh"}"#,
    )
    .unwrap();
    let created = json(xenterm_bridge::xenterm_session_upsert(
        session.as_ptr(),
    ));
    assert_eq!(created["ok"], true);
    let session_id = created["id"].as_str().unwrap().to_string();
    let session_id = CString::new(session_id).unwrap();

    let started = json(xenterm_bridge::xenterm_sftp_start(session_id.as_ptr()));
    assert_eq!(started["ok"], true);
    let handle = started["handle"].as_u64().unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = serde_json::Value::Null;
    loop {
        let snapshot = json(xenterm_bridge::xenterm_sftp_snapshot(handle));
        assert!(snapshot.get("entries").is_some());
        assert!(snapshot.get("path").is_some());
        let failed = snapshot["closed"].as_str().is_some()
            || snapshot["error"].as_str().is_some()
            || snapshot["status"]
                .as_str()
                .is_some_and(|status| status.contains("refused") || status.contains("失败"));
        last = snapshot;
        if failed || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        last["closed"].as_str().is_some() || last["error"].as_str().is_some(),
        "expected a terminal state, got {last}"
    );

    assert_eq!(xenterm_bridge::xenterm_sftp_close(handle), 1);
    assert_eq!(xenterm_bridge::xenterm_sftp_close(handle), 0);
    let _ = std::fs::remove_dir_all(dir);
}
