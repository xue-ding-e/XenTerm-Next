//! End-to-end check of the host-group C ABI against a temporary profile:
//! create → collapse → rename (moving sessions) → delete, all persisted
//! through the core's ConfigStore.

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
        "xenterm-bridge-groups-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);
    let mut args = vec!["xenterm-bridge-test".to_string()];
    xenterm::config::configure_profile(&mut args).unwrap();
    dir
}

fn groups() -> serde_json::Value {
    let json = take(xenterm_bridge::xenterm_groups_json());
    serde_json::from_str(&json).unwrap()
}

fn sessions() -> serde_json::Value {
    let json = take(xenterm_bridge::xenterm_sessions_json());
    serde_json::from_str(&json).unwrap()
}

fn call(name: &str, ptr: *mut std::ffi::c_char) -> serde_json::Value {
    let raw = take(ptr);
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        value["ok"], true,
        "{name} failed: {}",
        value["error"].as_str().unwrap_or("")
    );
    value
}

#[test]
fn host_groups_round_trip() {
    let profile = temp_profile();
    assert_eq!(groups().as_array().unwrap().len(), 0);

    let production = c("Production");
    call(
        "create",
        xenterm_bridge::xenterm_group_create(production.as_ptr()),
    );
    let rows = groups();
    assert_eq!(rows[0]["name"], "Production");
    assert_eq!(rows[0]["collapsed"], false); // a fresh folder is expanded

    // A second create of the same name is rejected.
    let duplicate = take(xenterm_bridge::xenterm_group_create(
        production.as_ptr(),
    ));
    let duplicate: serde_json::Value = serde_json::from_str(&duplicate).unwrap();
    assert_eq!(duplicate["ok"], false);

    let payload = c(r#"{"id":"","label":"db","address":"10.0.0.5","port":22,"username":"root","group":"Production","kind":"ssh"}"#);
    let created = call(
        "session",
        xenterm_bridge::xenterm_session_upsert(payload.as_ptr()),
    );
    let id = created["id"].as_str().unwrap().to_string();

    // Expanding the folder persists the open state.
    call(
        "collapse",
        xenterm_bridge::xenterm_group_collapse(production.as_ptr(), 0),
    );
    let rows = groups();
    assert_eq!(rows[0]["collapsed"], false);

    // Rename moves the session along.
    let prod = c("Prod");
    call(
        "rename",
        xenterm_bridge::xenterm_group_rename(
            production.as_ptr(),
            prod.as_ptr(),
        ),
    );
    let rows = groups();
    assert_eq!(rows[0]["name"], "Prod");
    let rows = sessions();
    assert_eq!(rows[0]["group"], "Prod");
    assert_eq!(rows[0]["id"], id);

    // Moving a session to another group and back to ungrouped.
    let staging = c("Staging");
    call(
        "create staging",
        xenterm_bridge::xenterm_group_create(staging.as_ptr()),
    );
    let id_c = c(&id);
    call(
        "move",
        xenterm_bridge::xenterm_session_move_group(id_c.as_ptr(), staging.as_ptr()),
    );
    assert_eq!(sessions()[0]["group"], "Staging");
    let empty = c("");
    call(
        "move ungrouped",
        xenterm_bridge::xenterm_session_move_group(id_c.as_ptr(), empty.as_ptr()),
    );
    assert_eq!(sessions()[0]["group"], "");

    // Deleting a folder drops it from the list.
    call(
        "delete",
        xenterm_bridge::xenterm_group_delete(prod.as_ptr()),
    );
    let names: Vec<String> = groups()
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["Staging".to_string()]);

    // Reserved names cannot be created.
    let default = c("default");
    let rejected = take(xenterm_bridge::xenterm_group_create(default.as_ptr()));
    let rejected: serde_json::Value = serde_json::from_str(&rejected).unwrap();
    assert_eq!(rejected["ok"], false);

    let _ = std::fs::remove_dir_all(profile);
}
