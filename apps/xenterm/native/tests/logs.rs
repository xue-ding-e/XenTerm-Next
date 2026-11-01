//! Settings round-trip and transcript listing/reading/deletion through the C
//! ABI. The writer itself is exercised by a live session; here the on-disk
//! format is validated verbatim.

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
fn settings_and_transcripts_round_trip() {
    let dir = std::env::temp_dir().join(format!(
        "xenterm-bridge-logs-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &dir);
    let mut args = vec!["xenterm-bridge-test".to_string()];
    xenterm::config::configure_profile(&mut args).unwrap();

    // Settings written through the bridge are visible in the next read.
    let settings = CString::new(
        r#"{"session_logs":true,"cursor_style":"bar","line_spacing":1.15,"padding":false,"bold":true,"sftp_follow_cd":false,"download_dir":"C:/downloads"}"#,
    )
    .unwrap();
    let result = json(xenterm_bridge::xenterm_settings_set(settings.as_ptr()));
    assert_eq!(result["ok"], true);
    let read = json(xenterm_bridge::xenterm_settings_json());
    assert_eq!(read["session_logs"], true);
    assert_eq!(read["cursor_style"], "bar");
    let spacing = read["line_spacing"].as_f64().unwrap();
    assert!((spacing - 1.15).abs() < 0.001, "spacing {spacing}");
    assert_eq!(read["padding"], false);
    assert_eq!(read["bold"], true);
    assert_eq!(read["sftp_follow_cd"], false);
    assert_eq!(read["download_dir"], "C:/downloads");

    // A transcript matching the writer's format is listed and readable.
    let logs = dir.join("session_logs");
    std::fs::create_dir_all(&logs).unwrap();
    let file = logs.join("1700000000000-abc.jsonl");
    std::fs::write(
        &file,
        concat!(
            "{\"kind\":\"meta\",\"session_id\":\"abc\",\"name\":\"gateway\",\"host\":\"10.0.0.2\",\"user\":\"root\",\"port\":22,\"started_ms\":1700000000000}\n",
            "{\"kind\":\"output\",\"text\":\"hello \"}\n",
            "{\"kind\":\"output\",\"text\":\"world\\n\"}\n",
            "{\"kind\":\"output\",\"text\":\"\\u001b[32mgreen\\u001b[0m\\r\\n\"}\n",
            "{\"kind\":\"end\",\"ended_ms\":1700000001000,\"reason\":\"done\"}\n",
        ),
    )
    .unwrap();

    let rows = json(xenterm_bridge::xenterm_logs_json());
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["file"], "1700000000000-abc.jsonl");
    assert_eq!(rows[0]["name"], "gateway");
    assert_eq!(rows[0]["user"], "root");

    let name = CString::new("1700000000000-abc.jsonl").unwrap();
    let content = json(xenterm_bridge::xenterm_log_read(name.as_ptr()));
    assert_eq!(content["ok"], true);
    assert_eq!(content["text"], "hello world\ngreen\n");
    assert_eq!(content["meta"]["session_id"], "abc");
    assert_eq!(content["ended_ms"], 1700000001000u64);
    assert_eq!(content["close_reason"], "done");

    // Path traversal is refused.
    let escape = CString::new("../../evil.jsonl").unwrap();
    let refused = json(xenterm_bridge::xenterm_log_read(escape.as_ptr()));
    assert_eq!(refused["ok"], false);

    let deleted = json(xenterm_bridge::xenterm_log_delete(name.as_ptr()));
    assert_eq!(deleted["ok"], true);
    assert_eq!(deleted["removed"], true);
    let rows = json(xenterm_bridge::xenterm_logs_json());
    assert_eq!(rows.as_array().unwrap().len(), 0);

    let _ = std::fs::remove_dir_all(dir);
}
