//! End-to-end acceptance against a REAL SSH server (the paramiko harness in
//! apps/xenterm/tool/sftp_test_server.py). Skipped unless XENTERM_LIVE_PORT is
//! set, so CI (no server) is unaffected. Run locally with:
//!
//!   XENTERM_LIVE_PORT=2222 cargo test -p xenterm-bridge --test live_flow -- --nocapture
//!
//! Everything asserted here goes through the exact bridge API the Flutter UI
//! calls, so it validates the GUI↔core contract, not just the core.

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

fn wait_for<F: Fn(&serde_json::Value) -> bool>(deadline_secs: u64, check: F) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(deadline_secs);
    loop {
        let snapshot = json(xenterm_bridge::xenterm_session_snapshot(handle()));
        if check(&snapshot) {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "condition not met in time");
        std::thread::sleep(Duration::from_millis(200));
    }
}

// The one live session handle; set by the test body.
static mut LIVE_HANDLE: u64 = 0;

fn handle() -> u64 {
    unsafe { LIVE_HANDLE }
}

fn screen_text(snapshot: &serde_json::Value) -> String {
    snapshot["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["text"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Answers any blocking prompt (host key / credentials) as the UI would.
fn answer_pending_prompt(snapshot: &serde_json::Value) -> bool {
    let Some(prompt) = snapshot["prompt"].as_object() else {
        return false;
    };
    let id = prompt["id"].as_u64().unwrap();
    match prompt["kind"].as_str().unwrap() {
        "host_key" => {
            xenterm_bridge::xenterm_session_answer_prompt(
                handle(),
                id,
                1,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            );
        }
        "credential" => {
            let user = CString::new("demo").unwrap();
            let pass = CString::new("demo").unwrap();
            xenterm_bridge::xenterm_session_answer_prompt(
                handle(),
                id,
                1,
                user.as_ptr(),
                pass.as_ptr(),
                std::ptr::null(),
            );
        }
        kind => panic!("unexpected prompt kind: {kind}"),
    }
    true
}

#[test]
fn live_desktop_flow_over_the_bridge() {
    let port: String = std::env::var("XENTERM_LIVE_PORT").unwrap_or_default();
    if port.is_empty() {
        eprintln!("XENTERM_LIVE_PORT not set; skipping live acceptance");
        return;
    }
    let data_dir = std::env::var("XENTERM_LIVE_DATA").unwrap_or_else(|_| {
        let dir = std::env::temp_dir().join(format!("xenterm-bridge-live-{}", std::process::id()));
        dir.to_string_lossy().into_owned()
    });
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&data_dir).unwrap();
    std::env::set_var("XENTERM_DATA_DIR", &data_dir);
    let mut args = vec!["xenterm-bridge-live".to_string()];
    xenterm::config::configure_profile(&mut args).unwrap();

    // Enable session transcripts so the Logs page has real data.
    let settings = CString::new(r#"{"session_logs": true}"#).unwrap();
    let applied = json(xenterm_bridge::xenterm_settings_set(settings.as_ptr()));
    assert_eq!(applied["ok"], true, "{applied}");

    // The host exactly as the UI's host drawer would save it: password auth,
    // one trigger that answers the shell prompt automatically.
    let session_payload = format!(
        r#"{{"id":"","label":"live-host","address":"127.0.0.1","port":{port},"username":"demo","group":"Live","kind":"ssh","password":"demo",
            "triggers":[{{"expect":"$ ","response":"echo TRIGGER_OK","append_enter":true,"repeat":false}}]}}"#
    );
    let created = {
        let text = CString::new(session_payload).unwrap();
        json(xenterm_bridge::xenterm_session_upsert(text.as_ptr()))
    };
    assert_eq!(created["ok"], true, "{created}");
    let id = created["id"].as_str().unwrap().to_string();

    // ── Terminal: connect, accept the host key, let the trigger fire. ──────
    let id_c = CString::new(id.clone()).unwrap();
    let started = json(xenterm_bridge::xenterm_session_start(
        id_c.as_ptr(),
        100,
        30,
    ));
    assert_eq!(started["ok"], true, "{started}");
    unsafe { LIVE_HANDLE = started["handle"].as_u64().unwrap() };

    // First the host-key prompt (the UI's banner), then the trigger's output.
    let mut snapshot = wait_for(20, |s| s["prompt"].is_object());
    let prompt_kind = snapshot["prompt"]["kind"].as_str().unwrap().to_string();
    assert_eq!(prompt_kind, "host_key", "{snapshot}");
    answer_pending_prompt(&snapshot);

    snapshot = wait_for(20, |s| {
        let text = screen_text(s);
        text.contains("TRIGGER_OK") && text.contains("XenTerm SFTP test server")
    });
    println!("trigger fired:\n{}", screen_text(&snapshot));

    // The remote resource monitor reports through the same snapshot: OS probe
    // (ID=ubuntu fixture) and a live sample.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        snapshot = json(xenterm_bridge::xenterm_session_snapshot(handle()));
        let os = snapshot["remoteOs"].as_str().unwrap_or_default();
        let resource_state = snapshot["resourceState"].as_str().unwrap_or_default();
        if os.contains("ubuntu") && resource_state == "available" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "remote OS/resource not live: os={os:?} state={resource_state:?} resource={}",
            snapshot["resource"]
        );
        std::thread::sleep(Duration::from_millis(300));
    }
    let resource = snapshot["resource"].as_object().expect("resource sample");
    assert!(resource.contains_key("cpu_percent"));
    assert!(resource.contains_key("mem_total_kib"));
    assert!(resource.contains_key("disks"));

    // ── Keyboard input through the same path the terminal pane uses. ───────
    let whoami = b"whoami\r";
    assert_eq!(
        xenterm_bridge::xenterm_session_input(handle(), whoami.as_ptr(), whoami.len()),
        1
    );
    snapshot = wait_for(10, |s| {
        let text = screen_text(s);
        // The typed command is echoed by the server and "demo" is its reply.
        text.matches("demo").count() >= 2
    });
    println!("whoami replied:\n{}", screen_text(&snapshot));

    // ── SFTP over the same saved session. ───────────────────────────────────
    let sftp_started = json(xenterm_bridge::xenterm_sftp_start(id_c.as_ptr()));
    assert_eq!(sftp_started["ok"], true, "{sftp_started}");
    let sftp_handle = sftp_started["handle"].as_u64().unwrap();

    // The host key is already trusted from the terminal connect; list "".
    let empty = CString::new("").unwrap();
    assert_eq!(
        xenterm_bridge::xenterm_sftp_list(sftp_handle, empty.as_ptr()),
        1
    );
    let sftp_deadline = Instant::now() + Duration::from_secs(15);
    let sftp = loop {
        let snap = json(xenterm_bridge::xenterm_sftp_snapshot(sftp_handle));
        let entries = snap["entries"].as_array().cloned().unwrap_or_default();
        if !entries.is_empty() {
            break snap;
        }
        assert!(
            Instant::now() < sftp_deadline,
            "sftp listing never arrived: {snap}"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    let names: Vec<String> = sftp["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str().map(str::to_string))
        .collect();
    println!("sftp root: {names:?}");
    assert!(names.iter().any(|n| n == "readme.txt"), "{names:?}");
    assert!(names.iter().any(|n| n == "Docs"), "{names:?}");

    // The tree is a folders-only sidebar (files are not tree nodes); opening a
    // folder switches the entries pane to it — exactly what the UI's SftpTree
    // onOpen does.
    let docs = CString::new("/Docs").unwrap();
    assert_eq!(
        xenterm_bridge::xenterm_sftp_toggle_tree(sftp_handle, docs.as_ptr()),
        1
    );
    assert_eq!(
        xenterm_bridge::xenterm_sftp_list(sftp_handle, docs.as_ptr()),
        1
    );
    let docs_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let snap = json(xenterm_bridge::xenterm_sftp_snapshot(sftp_handle));
        let names = snap["entries"].to_string();
        if snap["path"].as_str() == Some("/Docs") && names.contains("notes.txt") {
            break;
        }
        assert!(Instant::now() < docs_deadline, "/Docs never listed: {snap}");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(xenterm_bridge::xenterm_sftp_close(sftp_handle), 1);

    // ── Known hosts recorded the trusted key. ───────────────────────────────
    let known = json(xenterm_bridge::xenterm_known_hosts_json());
    let known_text = known.to_string();
    assert!(known_text.contains("127.0.0.1"), "{known_text}");

    // ── Snippets (quick commands) round-trip through the store. ─────────────
    let snippet = CString::new(
        r#"{"index":-1,"name":"uptime","command":"uptime","group":"Live","send_enter":true}"#,
    )
    .unwrap();
    let saved = json(xenterm_bridge::xenterm_quick_command_upsert(
        snippet.as_ptr(),
    ));
    assert_eq!(saved["ok"], true, "{saved}");
    let snippets = json(xenterm_bridge::xenterm_quick_commands_json());
    assert!(snippets.to_string().contains("uptime"), "{snippets}");

    // ── Transcript log exists and contains the session's output. ────────────
    assert_eq!(xenterm_bridge::xenterm_session_close(handle()), 1);
    std::thread::sleep(Duration::from_millis(500));
    let logs = json(xenterm_bridge::xenterm_logs_json());
    let logs = logs.as_array().unwrap();
    assert!(!logs.is_empty(), "no session transcript written");
    let name = logs[0]["file"].as_str().unwrap().to_string();
    let name_c = CString::new(name).unwrap();
    let transcript = json(xenterm_bridge::xenterm_log_read(name_c.as_ptr()));
    assert_eq!(transcript["ok"], true, "{transcript}");
    let text = transcript["text"].as_str().unwrap_or_default();
    assert!(text.contains("TRIGGER_OK"), "transcript: {text}");
    assert!(text.contains("demo"), "transcript: {text}");

    println!("LIVE ACCEPTANCE OK (data dir: {data_dir})");
}
