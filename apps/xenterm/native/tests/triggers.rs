//! Session triggers (expect/send) projection and round-trip. The response
//! text is a secret: the inventory only reports its presence, and a blank
//! response in an update keeps the stored value.

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

fn upsert(payload: &str) -> serde_json::Value {
    let text = CString::new(payload).unwrap();
    json(xenterm_bridge::xenterm_session_upsert(text.as_ptr()))
}

fn session_row(id: &str) -> serde_json::Value {
    let rows = json(xenterm_bridge::xenterm_sessions_json());
    rows.as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"].as_str() == Some(id))
        .cloned()
        .expect("session row")
}

#[test]
fn triggers_round_trip_without_leaking_the_response() {
    let dir = std::env::temp_dir().join(format!(
        "xenterm-bridge-triggers-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);
    let mut args = vec!["xenterm-bridge-test".to_string()];
    xenterm::config::configure_profile(&mut args).unwrap();

    let created = upsert(
        r#"{"id":"","label":"triggered","address":"10.0.0.9","port":22,"username":"root","group":"","kind":"ssh",
            "triggers":[
                {"expect":"login:","response":"opsuser","append_enter":true,"repeat":false},
                {"expect":"Password:","response":"s3cret","append_enter":true,"repeat":true}
            ]}"#,
    );
    assert_eq!(created["ok"], true);
    let id = created["id"].as_str().unwrap().to_string();

    let row = session_row(&id);
    let triggers = row["triggers"].as_array().unwrap();
    assert_eq!(triggers.len(), 2);
    assert_eq!(triggers[0]["expect"], "login:");
    assert_eq!(triggers[0]["has_response"], true);
    // The secret itself never crosses the bridge.
    assert!(triggers[0].get("response").is_none());
    assert_eq!(triggers[0]["append_enter"], true);
    assert_eq!(triggers[0]["repeat"], false);
    assert_eq!(triggers[1]["repeat"], true);

    // Update with blank responses keeps both stored secrets; a changed expect
    // text starts with an empty response; clear_response drops it.
    let updated = upsert(&format!(
        r#"{{"id":"{id}","label":"triggered","address":"10.0.0.9","port":22,"username":"root","group":"","kind":"ssh",
            "triggers":[
                {{"expect":"login:","response":"","append_enter":true,"repeat":true}},
                {{"expect":"username:","response":"","append_enter":false,"repeat":false}},
                {{"expect":"Password:","response":"","clear_response":true,"append_enter":true,"repeat":true}}
            ]}}"#
    ));
    assert_eq!(updated["ok"], true);

    let row = session_row(&id);
    let triggers = row["triggers"].as_array().unwrap();
    assert_eq!(triggers.len(), 3);
    // Kept by matching expect text; flags follow the new payload.
    assert_eq!(triggers[0]["expect"], "login:");
    assert_eq!(triggers[0]["repeat"], true);
    // New expect text ⇒ no stored response to keep.
    assert_eq!(triggers[1]["expect"], "username:");
    assert_eq!(triggers[1]["has_response"], false);
    assert_eq!(triggers[1]["append_enter"], false);
    // Explicit clear removes the stored response.
    assert_eq!(triggers[2]["expect"], "Password:");
    assert_eq!(triggers[2]["has_response"], false);

    // Empty expect rows are dropped instead of becoming no-op rules.
    let updated = upsert(&format!(
        r#"{{"id":"{id}","label":"triggered","address":"10.0.0.9","port":22,"username":"root","group":"","kind":"ssh",
            "triggers":[{{"expect":"  ","response":"x"}},{{"expect":"ok"}}]}}"#
    ));
    assert_eq!(updated["ok"], true);
    let row = session_row(&id);
    let triggers = row["triggers"].as_array().unwrap();
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0]["expect"], "ok");
}
