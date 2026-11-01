//! Serial configuration through the bridge: line parameters persist and the
//! login password is stored without ever being projected back out.

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

fn temp_profile() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xenterm-bridge-serial-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);
    let mut args = vec!["xenterm-bridge-test".to_string()];
    xenterm::config::configure_profile(&mut args).unwrap();
    dir
}

fn sessions() -> serde_json::Value {
    let json = take(xenterm_bridge::xenterm_sessions_json());
    serde_json::from_str(&json).unwrap()
}

fn upsert(payload: serde_json::Value) -> serde_json::Value {
    let payload = c(&payload.to_string());
    let raw = take(xenterm_bridge::xenterm_session_upsert(payload.as_ptr()));
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn serial_settings_and_login_round_trip() {
    let profile = temp_profile();

    let created = upsert(serde_json::json!({
        "id": "",
        "label": "Switch console",
        "address": "COM4",
        "port": 22,
        "username": "ops",
        "group": "",
        "kind": "serial",
        "baud_rate": 9600,
        "data_bits": 7,
        "stop_bits": 2,
        "parity": "even",
        "flow_control": "hardware",
        "password": "console-pass"
    }));
    assert_eq!(created["ok"], true, "{created}");
    let id = created["id"].as_str().unwrap().to_string();

    let rows = sessions();
    let row = &rows[0];
    assert_eq!(row["kind"], "serial");
    assert_eq!(row["address"], "COM4");
    assert_eq!(row["serial_port"], "COM4");
    assert_eq!(row["baud_rate"], 9600);
    assert_eq!(row["data_bits"], 7);
    assert_eq!(row["stop_bits"], 2);
    assert_eq!(row["parity"], "even");
    assert_eq!(row["flow_control"], "hardware");
    assert_eq!(row["has_password"], true);
    // The secret itself is never projected.
    assert!(row.get("password").is_none());

    // Editing without a password keeps the stored one.
    let kept = upsert(serde_json::json!({
        "id": id,
        "label": "Switch console",
        "address": "COM5",
        "port": 22,
        "username": "ops",
        "group": "",
        "kind": "serial",
        "baud_rate": 115200,
        "data_bits": 8,
        "stop_bits": 1,
        "parity": "none",
        "flow_control": "none"
    }));
    assert_eq!(kept["ok"], true, "{kept}");
    let rows = sessions();
    assert_eq!(rows[0]["address"], "COM5");
    assert_eq!(rows[0]["baud_rate"], 115200);
    assert_eq!(rows[0]["has_password"], true);

    // clear_password is the explicit wipe.
    let cleared = upsert(serde_json::json!({
        "id": id,
        "label": "Switch console",
        "address": "COM5",
        "port": 22,
        "username": "ops",
        "group": "",
        "kind": "serial",
        "clear_password": true
    }));
    assert_eq!(cleared["ok"], true, "{cleared}");
    assert_eq!(sessions()[0]["has_password"], false);

    // Port enumeration is a JSON array even on a machine with no devices.
    let ports = take(xenterm_bridge::xenterm_serial_ports_json());
    assert!(serde_json::from_str::<serde_json::Value>(&ports)
        .unwrap()
        .is_array());

    let _ = std::fs::remove_dir_all(profile);
}
