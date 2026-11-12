//! The Flutter frontend's bridge onto the xenterm service core
//! (docs/flutter-bridge.md).
//!
//! Step 1 of the API ladder: a minimal C ABI the Dart side loads with
//! `dart:ffi` — no flutter_rust_bridge codegen yet, so the first real
//! Rust↔Flutter link stays easy to audit. Strings cross as UTF-8 C pointers;
//! every returned string is owned by the caller and released through
//! [`xenterm_string_free`].
//!
//! Secrets never cross this surface: the session projection deliberately
//! carries only the fields the hosts list draws (docs/features.md §1 —
//! reveal-permission policy lives in the core's editor, not here).

use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use xenterm::frontend::TerminalSession;

/// The core, initialized exactly once per process (the bridge outlives
/// individual screens; ConfigStore::load is cheap after the first). The
/// mutex guards the store's save path (`upsert_and_save`/`remove`).
static STORE: OnceLock<Result<Mutex<xenterm::config::ConfigStore>, String>> = OnceLock::new();

/// This preview always pins a profile before any store or known-hosts access.
/// An explicit XENTERM_DATA_DIR remains an intentional caller override (including
/// test fixtures); otherwise use a distinct directory next to the executable.
/// A read-only install fails closed instead of falling back to GPUI's AppData,
/// and pinned profiles never run the core's legacy migration/backup recovery.
static PROFILE: OnceLock<Result<(), String>> = OnceLock::new();

fn profile() -> Result<(), String> {
    match PROFILE.get_or_init(|| {
        xenterm::config::configure_profile_from_env().map_err(|e| e.to_string())?;
        if xenterm::config::has_explicit_data_dir() {
            return Ok(());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let path = preview_profile_path(&exe)?;
        let path = path.to_str().ok_or("Flutter profile path is not valid Unicode")?;
        let mut args = vec!["xenterm-bridge".into(), "--data-dir".into(), path.into()];
        xenterm::config::configure_profile(&mut args)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }) {
        Ok(()) => Ok(()),
        Err(error) => Err(error.clone()),
    }
}

fn preview_profile_path(exe: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let parent = exe.parent().ok_or("Cannot locate Flutter executable directory")?;
    if !parent.is_absolute() {
        return Err("Flutter executable directory must be absolute".into());
    }
    Ok(parent.join("config-flutter-next"))
}

#[cfg(test)]
mod preview_profile_tests {
    use super::preview_profile_path;
    #[test]
    fn default_profile_is_distinct_and_has_no_fallback() {
        let root = std::env::temp_dir();
        assert_eq!(preview_profile_path(&root.join("xenterm.exe")).unwrap(),
                   root.join("config-flutter-next"));
        assert_ne!(preview_profile_path(&root.join("xenterm.exe")).unwrap(), root.join("config"));
        assert!(preview_profile_path(std::path::Path::new("relative.exe")).is_err());
    }
}

fn store() -> Result<&'static Mutex<xenterm::config::ConfigStore>, String> {
    profile()?;
    match STORE.get_or_init(|| {
        xenterm::config::ConfigStore::load()
            .map(Mutex::new)
            .map_err(|e| e.to_string())
    }) {
        Ok(store) => Ok(store),
        Err(error) => Err(error.clone()),
    }
}

/// The tokio runtime the session workers run on (same crate/version as the
/// core's dependency; Cargo unifies it).
static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();

fn runtime() -> Result<&'static tokio::runtime::Runtime, String> {
    match RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())
    }) {
        Ok(runtime) => Ok(runtime),
        Err(error) => Err(error.clone()),
    }
}

/// Live terminal sessions, keyed by the handle returned to Dart.
static SESSIONS: OnceLock<Arc<Mutex<HashMap<u64, Arc<TerminalSession>>>>> = OnceLock::new();
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

fn sessions() -> &'static Arc<Mutex<HashMap<u64, Arc<TerminalSession>>>> {
    SESSIONS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn session(handle: u64) -> Option<Arc<TerminalSession>> {
    sessions()
        .lock()
        .ok()
        .and_then(|map| map.get(&handle).cloned())
}

/// SAFETY (callers): `ptr` must have come from Dart's `toNativeUtf8`.
unsafe fn take_c(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

fn to_c(s: String) -> *mut c_char {
    // A failed allocation aborts; a NUL inside `s` cannot occur in the JSON
    // we produce (serde escapes it), so unwrap is honest here.
    CString::new(s)
        .expect("interior NUL in bridge string")
        .into_raw()
}

/// The service core's version string (版本号规范 v1: `xt-…`).
#[no_mangle]
pub extern "C" fn xenterm_core_version() -> *mut c_char {
    to_c(xenterm::version::VERSION.to_string())
}

/// The saved sessions as a JSON array, projected to the fields the hosts
/// screen draws. `null` on store failure (the Dart side surfaces the error).
#[no_mangle]
pub extern "C" fn xenterm_sessions_json() -> *mut c_char {
    let json = (|| -> Result<String, String> {
        let store = store()?;
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let rows: Vec<serde_json::Value> = store
            .sessions()
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "label": if s.name.is_empty() { &s.host } else { &s.name },
                    "address": if s.kind == xenterm::config::SessionKind::Serial {
                        &s.serial_port
                    } else {
                        &s.host
                    },
                    "port": s.port,
                    "username": s.user,
                    "group": s.group,
                    "kind": match s.kind {
                        xenterm::config::SessionKind::Ssh => "ssh",
                        xenterm::config::SessionKind::Serial => "serial",
                        xenterm::config::SessionKind::Telnet => "telnet",
                        xenterm::config::SessionKind::Local => "local",
                    },
                    "last_used": s.last_used,
                    "auth": s.auth.as_str(),
                    "private_key_path": s.private_key_path,
                    // Whether imported/pasted key material is stored — the
                    // material itself never crosses the bridge.
                    "has_inline_key": !s.private_key_inline.is_empty(),
                    "jump_session_id": s.jump_session_id,
                    // Terminal charset (WHATWG label); the core falls back to
                    // UTF-8 for unknown labels.
                    "encoding": s.encoding,
                    "serial": !s.serial_port.is_empty(),
                    // Serial settings for the connection dialog. The password
                    // itself never crosses the bridge — only whether one is
                    // stored (the dialog shows "stored" and leaves it blank).
                    "serial_port": s.serial_port,
                    "baud_rate": s.baud_rate,
                    "data_bits": s.data_bits,
                    "stop_bits": s.stop_bits,
                    "parity": s.parity,
                    "flow_control": s.flow_control,
                    "has_password": !s.password.is_empty(),
                    // Expect/send rules. The response text is a secret: only
                    // its presence crosses the bridge; the editor leaves the
                    // field blank to keep the stored value (as with passwords).
                    "triggers": s
                        .triggers
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "expect": t.expect,
                                "has_response": !t.response.is_empty(),
                                "append_enter": t.append_enter,
                                "repeat": t.repeat,
                            })
                        })
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        serde_json::to_string(&rows).map_err(|e| e.to_string())
    })();
    match json {
        Ok(json) => to_c(json),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Create or update one session from the JSON the drawer/editor sends
/// (`id` empty = create). Returns `{"ok":true,"id":"…"}` on success or
/// `{"ok":false,"error":"…"}`.
#[no_mangle]
pub extern "C" fn xenterm_session_upsert(json: *const c_char) -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let raw = unsafe { take_c(json) };
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("invalid payload: {e}"))?;
        let id = value.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        let label = value
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let address = value
            .get("address")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let port = value.get("port").and_then(|v| v.as_u64()).unwrap_or(22) as u16;
        let username = value
            .get("username")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let group = value
            .get("group")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let kind = value.get("kind").and_then(|v| v.as_str()).unwrap_or("ssh");
        let key_path = value
            .get("private_key_path")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        // Pasted/imported key material. Empty means "keep the stored key";
        // `clear_private_key` drops it (and the path) explicitly.
        let key_inline = value
            .get("private_key_inline")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let clear_key = value
            .get("clear_private_key")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let password = value
            .get("password")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let clear_password = value
            .get("clear_password")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // Terminal charset for this session (WHATWG label, e.g. "gbk").
        // The core decodes with `Encoding::for_label` and falls back to
        // UTF-8, so unknown labels are safe; empty means "keep the stored
        // value" so the dialog never wipes it by omission.
        let encoding = value
            .get("encoding")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let baud_rate = value
            .get("baud_rate")
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(50, 4_000_000) as u32);
        let data_bits = value
            .get("data_bits")
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(5, 8) as u8);
        let stop_bits = value
            .get("stop_bits")
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(1, 2) as u8);
        let parity = value.get("parity").and_then(|v| v.as_str());
        let flow_control = value.get("flow_control").and_then(|v| v.as_str());
        let jump_session_id = value.get("jump_session_id").and_then(|v| v.as_str());
        // Optional full replacement of the expect/send rule list. Rows with a
        // blank response keep the stored response of the rule with the same
        // expect text; `clear_response` drops it explicitly.
        let trigger_rows = value.get("triggers").and_then(|v| v.as_array()).cloned();

        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = store
            .sessions()
            .iter()
            .find(|s| !id.is_empty() && s.id == id)
            .cloned()
            .unwrap_or_else(xenterm::config::Session::new_empty);

        session.name = label.to_string();
        session.host = address.to_string();
        session.port = port;
        session.user = username.to_string();
        session.group = group.to_string();
        session.kind = xenterm::config::SessionKind::from_str(kind);
        if clear_key {
            session.private_key_path = String::new();
            session.private_key_inline = xenterm::config::Secret::default();
        } else {
            session.private_key_path = key_path.trim().to_string();
            if !key_inline.trim().is_empty() {
                session.private_key_inline = xenterm::config::Secret::new(key_inline);
            }
        }
        session.auth =
            if session.private_key_path.is_empty() && session.private_key_inline.is_empty() {
                xenterm::config::AuthMethod::Password
            } else {
                xenterm::config::AuthMethod::Key
            };
        if !encoding.trim().is_empty() {
            session.encoding = encoding.trim().to_string();
        }
        if let Some(baud) = baud_rate {
            session.baud_rate = baud;
        }
        if let Some(bits) = data_bits {
            session.data_bits = bits;
        }
        if let Some(bits) = stop_bits {
            session.stop_bits = bits;
        }
        if let Some(value) = parity {
            session.parity = value.to_string();
        }
        if let Some(value) = flow_control {
            session.flow_control = value.to_string();
        }
        // An empty password means "keep the stored one": the dialog never
        // receives the secret, so a blank field cannot be a wipe. Clearing is
        // an explicit flag.
        if clear_password {
            session.password = xenterm::config::Secret::default();
        } else if !password.is_empty() {
            session.password = xenterm::config::Secret::new(password);
        }
        if let Some(jump) = jump_session_id {
            session.jump_session_id = jump.trim().to_string();
            session.jump_session_ids.clear();
        }
        if session.kind == xenterm::config::SessionKind::Serial {
            session.serial_port = address.to_string();
        }
        if let Some(rows) = trigger_rows {
            let old = session.triggers.clone();
            session.triggers = rows
                .iter()
                .filter_map(|row| {
                    let expect = row
                        .get("expect")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    if expect.is_empty() {
                        return None;
                    }
                    let response = row
                        .get("response")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let response = if !response.is_empty() {
                        xenterm::config::Secret::new(response)
                    } else if row
                        .get("clear_response")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        xenterm::config::Secret::default()
                    } else {
                        old.iter()
                            .find(|t| t.expect == expect)
                            .map(|t| t.response.clone())
                            .unwrap_or_default()
                    };
                    Some(xenterm::config::SessionTrigger {
                        expect,
                        response,
                        append_enter: row
                            .get("append_enter")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true),
                        repeat: row.get("repeat").and_then(|v| v.as_bool()).unwrap_or(false),
                    })
                })
                .collect();
        }
        let id = session.id.clone();
        store
            .upsert_and_save(session)
            .map_err(|e| format!("save failed: {e}"))?;
        Ok(id)
    })();
    match result {
        Ok(id) => to_c(serde_json::json!({"ok": true, "id": id}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Remove one saved session. Returns `{"ok":true}` or `{"ok":false,"error"}`.
#[no_mangle]
pub extern "C" fn xenterm_session_delete(id: *const c_char) -> *mut c_char {
    let result = (|| -> Result<(), String> {
        let id = unsafe { take_c(id) };
        if id.is_empty() {
            return Err("empty session id".into());
        }
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        store.remove(&id);
        store.save().map_err(|e| format!("save failed: {e}"))
    })();
    match result {
        Ok(()) => to_c(serde_json::json!({"ok": true}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Display-only groups owned by the core's built-in shells; they must never
/// surface as user folders (`config::is_reserved_session_group` is crate
/// private, so the bridge mirrors the predicate).
fn is_reserved_group(name: &str) -> bool {
    let name = name.trim();
    name.is_empty() || name.eq_ignore_ascii_case("default") || name.eq_ignore_ascii_case("system")
}

/// Named host groups as a JSON array of `{"name", "collapsed"}`: explicit
/// folders ∪ the groups sessions are filed under, in persisted display order.
#[no_mangle]
pub extern "C" fn xenterm_groups_json() -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let store = store()?;
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let names = store.ordered_session_groups();
        let collapsed = store.collapsed_session_groups().unwrap_or(&[]);
        let rows: Vec<serde_json::Value> = names
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "collapsed": collapsed.iter().any(|c| c.eq_ignore_ascii_case(name)),
                })
            })
            .collect();
        serde_json::to_string(&rows).map_err(|e| e.to_string())
    })();
    match result {
        Ok(json) => to_c(json),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Reorder all named groups using an exact display-order compare-and-swap.
/// JSON: {"expected":[current names],"order":[requested names]}.
#[no_mangle]
pub extern "C" fn xenterm_group_reorder(request: *const c_char) -> *mut c_char {
    fn names(value: &serde_json::Value, field: &str) -> Result<Vec<String>, String> {
        let values = value
            .as_array()
            .ok_or_else(|| format!("group order {field} must be an array of strings"))?;
        values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("group order {field} must contain only strings"))
            })
            .collect()
    }
    let result = (|| -> Result<(), String> {
        let request: serde_json::Value = serde_json::from_str(&unsafe { take_c(request) })
            .map_err(|error| format!("invalid group order: {error}"))?;
        let request = request
            .as_object()
            .ok_or("group order request must be an object")?;
        if request.len() != 2 || !request.contains_key("expected") || !request.contains_key("order") {
            return Err("group order request must contain exactly expected and order".into());
        }
        let expected = names(&request["expected"], "expected")?;
        let order = names(&request["order"], "order")?;
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        store
            .reorder_session_groups_and_save(&expected, &order)
            .map_err(|error| error.to_string())
    })();
    match result {
        Ok(()) => to_c(serde_json::json!({"ok": true}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Create an empty group. Returns `{"ok":true}` or `{"ok":false,"error"}`.
#[no_mangle]
pub extern "C" fn xenterm_group_create(name: *const c_char) -> *mut c_char {
    let result = (|| -> Result<(), String> {
        let name = unsafe { take_c(name) }.trim().to_string();
        if is_reserved_group(&name) {
            return Err("that name is reserved".into());
        }
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        if store.session_group_exists(&name) {
            return Err("a group with that name already exists".into());
        }
        store.add_group(name);
        store.save().map_err(|e| format!("save failed: {e}"))
    })();
    match result {
        Ok(()) => to_c(serde_json::json!({"ok": true}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Rename a group and move its sessions along.
#[no_mangle]
pub extern "C" fn xenterm_group_rename(old: *const c_char, new: *const c_char) -> *mut c_char {
    let result = (|| -> Result<(), String> {
        let old = unsafe { take_c(old) }.trim().to_string();
        let new = unsafe { take_c(new) }.trim().to_string();
        if is_reserved_group(&old) {
            return Err("that group no longer exists".into());
        }
        if is_reserved_group(&new) {
            return Err("the name cannot be empty".into());
        }
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        if !store.session_group_exists(&old) {
            return Err("that group no longer exists".into());
        }
        if !new.eq_ignore_ascii_case(&old) && store.session_group_exists(&new) {
            return Err("a group with that name already exists".into());
        }
        store.rename_group(&old, new);
        store.save().map_err(|e| format!("save failed: {e}"))
    })();
    match result {
        Ok(()) => to_c(serde_json::json!({"ok": true}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Delete a group; its sessions fall back to ungrouped.
#[no_mangle]
pub extern "C" fn xenterm_group_delete(name: *const c_char) -> *mut c_char {
    let result = (|| -> Result<(), String> {
        let name = unsafe { take_c(name) }.trim().to_string();
        if is_reserved_group(&name) {
            return Err("that group no longer exists".into());
        }
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        if !store.session_group_exists(&name) {
            return Err("that group no longer exists".into());
        }
        store.remove_group(&name);
        store.save().map_err(|e| format!("save failed: {e}"))
    })();
    match result {
        Ok(()) => to_c(serde_json::json!({"ok": true}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Remember a group's collapsed state (`collapsed` != 0 folds it).
#[no_mangle]
pub extern "C" fn xenterm_group_collapse(name: *const c_char, collapsed: u8) -> *mut c_char {
    let result = (|| -> Result<(), String> {
        let name = unsafe { take_c(name) }.trim().to_string();
        if is_reserved_group(&name) {
            return Err("that group no longer exists".into());
        }
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        store.set_session_group_collapsed(&name, collapsed != 0);
        store.save().map_err(|e| format!("save failed: {e}"))
    })();
    match result {
        Ok(()) => to_c(serde_json::json!({"ok": true}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Move one saved session into `group` (empty = ungrouped).
#[no_mangle]
pub extern "C" fn xenterm_session_move_group(
    id: *const c_char,
    group: *const c_char,
) -> *mut c_char {
    let result = (|| -> Result<(), String> {
        let id = unsafe { take_c(id) };
        let group = unsafe { take_c(group) }.trim().to_string();
        if id.is_empty() {
            return Err("empty session id".into());
        }
        // An empty group means "ungrouped", which the UI offers explicitly.
        if !group.is_empty() && is_reserved_group(&group) {
            return Err("that group no longer exists".into());
        }
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        if !group.is_empty() && !store.session_group_exists(&group) {
            return Err("that group no longer exists".into());
        }
        let Some(mut session) = store.sessions().iter().find(|s| s.id == id).cloned() else {
            return Err("session no longer exists".into());
        };
        session.group = group;
        store
            .upsert_and_save(session)
            .map_err(|e| format!("save failed: {e}"))
    })();
    match result {
        Ok(()) => to_c(serde_json::json!({"ok": true}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Starts a saved session and returns `{"ok":true,"handle":N}` or
/// `{"ok":false,"error":"…"}`.
#[no_mangle]
pub extern "C" fn xenterm_session_start(id: *const c_char, cols: u32, rows: u32) -> *mut c_char {
    let result = (|| -> Result<u64, String> {
        let id = unsafe { take_c(id) };
        if id.is_empty() {
            return Err("empty session id".into());
        }
        let store = store()?;
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let runtime = runtime()?;
        let terminal = TerminalSession::start(
            runtime.handle(),
            &store,
            &id,
            cols.clamp(1, 1000) as u16,
            rows.clamp(1, 1000) as u16,
            4000,
        )?;
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut map) = sessions().lock() {
            map.insert(handle, terminal);
        }
        Ok(handle)
    })();
    match result {
        Ok(handle) => to_c(serde_json::json!({"ok": true, "handle": handle}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Starts a temporary session from JSON (local terminal / serial quick
/// connect) without writing it to the config store.
#[no_mangle]
pub extern "C" fn xenterm_session_start_adhoc(json: *const c_char) -> *mut c_char {
    let result = (|| -> Result<u64, String> {
        let raw = unsafe { take_c(json) };
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("invalid payload: {e}"))?;
        let text = |key: &str| {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let cols = value
            .get("cols")
            .and_then(|v| v.as_u64())
            .unwrap_or(80)
            .clamp(1, 1000) as u16;
        let rows = value
            .get("rows")
            .and_then(|v| v.as_u64())
            .unwrap_or(24)
            .clamp(1, 1000) as u16;
        let mut session = xenterm::config::Session::new_empty();
        let label = text("label");
        let address = text("address");
        session.name = if label.is_empty() {
            address.clone()
        } else {
            label
        };
        session.host = address.clone();
        session.port = value
            .get("port")
            .and_then(|v| v.as_u64())
            .unwrap_or(22)
            .clamp(1, 65535) as u16;
        session.user = text("username");
        session.kind = xenterm::config::SessionKind::from_str(&text("kind"));
        let password = text("password");
        if !password.is_empty() {
            session.password = xenterm::config::Secret::new(password);
        }
        session.serial_port = text("serial_port");
        if session.kind == xenterm::config::SessionKind::Serial && session.serial_port.is_empty() {
            session.serial_port = address;
        }
        if let Some(baud) = value.get("baud_rate").and_then(|v| v.as_u64()) {
            session.baud_rate = baud.clamp(50, 4_000_000) as u32;
        }
        if let Some(bits) = value.get("data_bits").and_then(|v| v.as_u64()) {
            session.data_bits = bits.clamp(5, 8) as u8;
        }
        if let Some(bits) = value.get("stop_bits").and_then(|v| v.as_u64()) {
            session.stop_bits = bits.clamp(1, 2) as u8;
        }
        let parity = text("parity");
        if !parity.is_empty() {
            session.parity = parity;
        }
        let flow = text("flow_control");
        if !flow.is_empty() {
            session.flow_control = flow;
        }
        let log_enabled = {
            let store = store()?;
            let store = store.lock().unwrap_or_else(|p| p.into_inner());
            store.session_logs()
        };
        let runtime = runtime()?;
        let terminal =
            TerminalSession::start_adhoc(runtime.handle(), session, log_enabled, cols, rows, 4000)?;
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut map) = sessions().lock() {
            map.insert(handle, terminal);
        }
        Ok(handle)
    })();
    match result {
        Ok(handle) => to_c(serde_json::json!({"ok": true, "handle": handle}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Cheap change counter for a running terminal; 0 when the handle is unknown.
#[no_mangle]
pub extern "C" fn xenterm_session_version(handle: u64) -> u64 {
    session(handle).map(|s| s.version()).unwrap_or(0)
}

/// Compact JSON screen snapshot (rows/runs/cursor/modes/prompt), or null when
/// the handle is unknown.
#[no_mangle]
pub extern "C" fn xenterm_session_snapshot(handle: u64) -> *mut c_char {
    match session(handle) {
        Some(session) => to_c(session.snapshot_json()),
        None => std::ptr::null_mut(),
    }
}

/// Enqueues raw bytes (encoded keys/paste). Returns 1 only when accepted;
/// 0 includes SSH authentication/startup and closing or closed sessions.
#[no_mangle]
pub extern "C" fn xenterm_session_input(handle: u64, bytes: *const u8, len: usize) -> i32 {
    if bytes.is_null() || len == 0 {
        return 0;
    }
    let slice = unsafe { std::slice::from_raw_parts(bytes, len) };
    match session(handle) {
        Some(session) => i32::from(session.send_input(slice)),
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn xenterm_session_resize(handle: u64, cols: u32, rows: u32) -> i32 {
    match session(handle) {
        Some(session) => {
            session.resize(cols.clamp(1, 1000) as u16, rows.clamp(1, 1000) as u16);
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn xenterm_session_close(handle: u64) -> i32 {
    // Cancelling a prompt may synchronously wake its receiver. Do not retain
    // the registry lock across close, even if that wake reenters the bridge.
    let removed = sessions().lock().ok().and_then(|mut map| map.remove(&handle));
    match removed {
        Some(session) => {
            session.close();
            1
        }
        None => 0,
    }
}

/// Returns 1 only when the matching prompt's oneshot accepted the answer.
/// This is not an authentication-success or receiver-processing acknowledgement.
#[no_mangle]
pub extern "C" fn xenterm_session_answer_prompt(
    handle: u64,
    prompt_id: u64,
    accept: i32,
    username: *const c_char,
    password: *const c_char,
    text: *const c_char,
) -> i32 {
    let opt = |ptr: *const c_char| -> Option<String> {
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { take_c(ptr) })
        }
    };
    let user = opt(username);
    let pass = opt(password);
    let text = opt(text);
    match session(handle) {
        Some(session) => i32::from(session.try_answer_prompt(
            prompt_id, accept != 0, user, pass, text,
        )),
        None => 0,
    }
}

/// Import hosts from OpenSSH config text (`{"text": "..."}`); an empty text
/// imports `~/.ssh/config`. Returns `{"ok", "added", "skipped", "message"}`.
#[no_mangle]
pub extern "C" fn xenterm_import_ssh_config(json: *const c_char) -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let raw = unsafe { take_c(json) };
        let text = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|value| {
                value
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        let summary = if text.trim().is_empty() {
            xenterm::frontend::import_ssh_config_default(&mut store)
        } else {
            xenterm::frontend::import_ssh_config_text(&mut store, &text)
        }
        .map_err(|error| format!("import failed: {error}"))?;
        Ok(serde_json::json!({
            "ok": true,
            "added": summary.added,
            "skipped": summary.skipped,
            "message": summary.message,
        })
        .to_string())
    })();
    match result {
        Ok(json) => to_c(json),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Saved quick commands (Snippets) as a JSON array.
#[no_mangle]
pub extern "C" fn xenterm_quick_commands_json() -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let store = store()?;
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let rows: Vec<serde_json::Value> = store
            .quick_commands()
            .iter()
            .enumerate()
            .map(|(index, cmd)| {
                serde_json::json!({
                    "index": index,
                    "name": cmd.name,
                    "command": cmd.command,
                    "group": cmd.group,
                    "send_enter": cmd.send_enter,
                })
            })
            .collect();
        serde_json::to_string(&rows).map_err(|e| e.to_string())
    })();
    match result {
        Ok(json) => to_c(json),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Create (`index` < 0 or out of range) or update one quick command.
/// Returns `{"ok":true,"index":N}` or `{"ok":false,"error"}`.
#[no_mangle]
pub extern "C" fn xenterm_quick_command_upsert(json: *const c_char) -> *mut c_char {
    let result = (|| -> Result<usize, String> {
        let raw = unsafe { take_c(json) };
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("invalid payload: {e}"))?;
        let index = value.get("index").and_then(|v| v.as_i64()).unwrap_or(-1);
        let name = value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let command = value
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let group = value
            .get("group")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let send_enter = value
            .get("send_enter")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if command.is_empty() {
            return Err("the command cannot be empty".into());
        }
        let name = if name.is_empty() {
            command.clone()
        } else {
            name
        };

        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        let mut commands = store.quick_commands().to_vec();
        let command = xenterm::config::QuickCommand {
            name,
            command,
            group,
            send_enter,
        };
        let position = if index >= 0 && (index as usize) < commands.len() {
            commands[index as usize] = command;
            index as usize
        } else {
            commands.push(command);
            commands.len() - 1
        };
        store.set_quick_commands(commands);
        store.save().map_err(|e| format!("save failed: {e}"))?;
        Ok(position)
    })();
    match result {
        Ok(index) => to_c(serde_json::json!({"ok": true, "index": index}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Remove one quick command by its current index.
#[no_mangle]
pub extern "C" fn xenterm_quick_command_delete(index: i64) -> *mut c_char {
    let result = (|| -> Result<(), String> {
        if index < 0 {
            return Err("invalid index".into());
        }
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        let mut commands = store.quick_commands().to_vec();
        let at = index as usize;
        if at >= commands.len() {
            return Err("quick command no longer exists".into());
        }
        commands.remove(at);
        store.set_quick_commands(commands);
        store.save().map_err(|e| format!("save failed: {e}"))?;
        Ok(())
    })();
    match result {
        Ok(()) => to_c(serde_json::json!({"ok": true}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Serial ports reported by the OS as `[{"name","kind"}]`.
#[no_mangle]
pub extern "C" fn xenterm_serial_ports_json() -> *mut c_char {
    let rows: Vec<serde_json::Value> = xenterm::frontend::serial_ports()
        .into_iter()
        .map(|port| serde_json::json!({ "name": port.name, "kind": port.kind }))
        .collect();
    match serde_json::to_string(&rows) {
        Ok(json) => to_c(json),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Remembered SSH host keys (TOFU store) as a JSON array.
#[no_mangle]
pub extern "C" fn xenterm_known_hosts_json() -> *mut c_char {
    if profile().is_err() {
        return std::ptr::null_mut();
    }
    let rows: Vec<serde_json::Value> = xenterm::frontend::known_hosts()
        .into_iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry.id,
                "key_type": entry.key_type,
                "fingerprint": entry.fingerprint,
            })
        })
        .collect();
    match serde_json::to_string(&rows) {
        Ok(json) => to_c(json),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Forgets one known-host entry by its `host:port` id.
#[no_mangle]
pub extern "C" fn xenterm_known_host_delete(id: *const c_char) -> *mut c_char {
    let id = unsafe { take_c(id) };
    let removed = match profile() {
        Ok(()) => xenterm::frontend::forget_known_host(&id),
        Err(error) => Err(error),
    };
    match removed {
        Ok(removed) => to_c(serde_json::json!({"ok": true, "removed": removed}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Portable profile export — the same JSON the CLI writes, with session
/// secrets obfuscated under the export key. `count` is the exported session
/// total so the UI can confirm what a file contains.
#[no_mangle]
pub extern "C" fn xenterm_profile_export() -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let store = store()?;
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let (json, count) = store.export_json().map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "ok": true, "count": count, "json": json }).to_string())
    })();
    match result {
        Ok(json) => to_c(json),
        Err(error) => to_c(serde_json::json!({ "ok": false, "error": error }).to_string()),
    }
}

/// Imports a portable profile (xenterm/meatshell export, native sessions.json
/// or FinalShell JSON). Existing sessions are never overwritten. The payload
/// is `{"json": <export text>, "dry_run": bool}` — with `dry_run` the same
/// parsing and validation runs without touching the store, so the UI can show
/// what would happen before committing.
#[no_mangle]
pub extern "C" fn xenterm_profile_import(json: *const c_char) -> *mut c_char {
    let raw = unsafe { take_c(json) };
    let result = (|| -> Result<String, String> {
        let payload: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("invalid payload: {e}"))?;
        let text = payload
            .get("json")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let dry_run = payload
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        let summary = store
            .import_json_preview(text, dry_run)
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "ok": true,
            "added": summary.added,
            "skipped": summary.skipped,
            "warnings": summary.warnings,
        })
        .to_string())
    })();
    match result {
        Ok(json) => to_c(json),
        Err(error) => to_c(serde_json::json!({ "ok": false, "error": error }).to_string()),
    }
}

/// Live port-forward tunnels across every open session, tagged with the
/// owning session's name so the forwards screen can show what is running.
#[no_mangle]
pub extern "C" fn xenterm_runtime_tunnels_json() -> *mut c_char {
    let sessions = sessions();
    let sessions = sessions.lock().unwrap_or_else(|p| p.into_inner());
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for (handle, session) in sessions.iter() {
        let list: Vec<serde_json::Value> =
            serde_json::from_str(&session.tunnels_json()).unwrap_or_default();
        for mut tunnel in list {
            tunnel["session_handle"] = (*handle).into();
            rows.push(tunnel);
        }
    }
    to_c(serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into()))
}

/// Every saved SSH port-forward rule, annotated with its owning session.
#[no_mangle]
pub extern "C" fn xenterm_forwards_json() -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let store = store()?;
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let mut rows = Vec::new();
        for session in store.sessions() {
            for (index, forward) in session.forwards.iter().enumerate() {
                rows.push(serde_json::json!({
                    "session_id": session.id,
                    "session_name": if session.name.is_empty() { &session.host } else { &session.name },
                    "session_kind": match session.kind {
                        xenterm::config::SessionKind::Ssh => "ssh",
                        xenterm::config::SessionKind::Serial => "serial",
                        xenterm::config::SessionKind::Telnet => "telnet",
                        xenterm::config::SessionKind::Local => "local",
                    },
                    "index": index,
                    "kind": forward.kind,
                    "name": forward.name,
                    "bind_addr": forward.bind_addr,
                    "bind_port": forward.bind_port,
                    "host": forward.host,
                    "host_port": forward.host_port,
                }));
            }
        }
        serde_json::to_string(&rows).map_err(|e| e.to_string())
    })();
    match result {
        Ok(json) => to_c(json),
        Err(error) => to_c(serde_json::json!({"__error": error}).to_string()),
    }
}

/// Saved SSH sessions that reach a host through jump hosts.
#[no_mangle]
pub extern "C" fn xenterm_jump_chains_json() -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let store = store()?;
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let rows: Vec<serde_json::Value> = xenterm::frontend::jump_chains(&store)
            .into_iter()
            .map(|chain| {
                serde_json::json!({
                    "session_id": chain.session_id,
                    "session_name": chain.session_name,
                    "host": chain.host,
                    "jumps": chain.jumps,
                })
            })
            .collect();
        serde_json::to_string(&rows).map_err(|e| e.to_string())
    })();
    match result {
        Ok(json) => to_c(json),
        Err(error) => to_c(serde_json::json!({"__error": error}).to_string()),
    }
}

fn json_port(value: &serde_json::Value, key: &str) -> String {
    match value.get(key) {
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// Create (`index` < 0) or update one port-forward rule on its session.
/// Validation is the same rule set the desktop editor uses.
#[no_mangle]
pub extern "C" fn xenterm_forward_upsert(json: *const c_char) -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let raw = unsafe { take_c(json) };
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("invalid payload: {e}"))?;
        let session_id = value
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if session_id.is_empty() {
            return Err("no host selected".into());
        }
        let index = value.get("index").and_then(|v| v.as_i64()).unwrap_or(-1);
        let draft = xenterm::core::tunnel::TunnelDraft {
            kind: value
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("local")
                .to_string(),
            name: value
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            bind_addr: value
                .get("bind_addr")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            bind_port: json_port(&value, "bind_port"),
            host: value
                .get("host")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            host_port: json_port(&value, "host_port"),
        };
        let rule = xenterm::core::tunnel::validate(&[draft])
            .map_err(|error| error)?
            .into_iter()
            .next()
            .ok_or_else(|| "the rule is empty".to_string())?;

        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = store
            .sessions()
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .ok_or_else(|| "host no longer exists".to_string())?;
        let position = if index >= 0 && (index as usize) < session.forwards.len() {
            session.forwards[index as usize] = rule;
            index as usize
        } else {
            session.forwards.push(rule);
            session.forwards.len() - 1
        };
        store
            .upsert_and_save(session)
            .map_err(|e| format!("save failed: {e}"))?;
        Ok(
            serde_json::json!({"ok": true, "index": position, "session_id": session_id})
                .to_string(),
        )
    })();
    match result {
        Ok(json) => to_c(json),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Delete one port-forward rule from its session.
#[no_mangle]
pub extern "C" fn xenterm_forward_delete(json: *const c_char) -> *mut c_char {
    let result = (|| -> Result<(), String> {
        let raw = unsafe { take_c(json) };
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("invalid payload: {e}"))?;
        let session_id = value
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let index = value.get("index").and_then(|v| v.as_i64()).unwrap_or(-1);
        if session_id.is_empty() || index < 0 {
            return Err("invalid rule reference".into());
        }
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        let mut session = store
            .sessions()
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .ok_or_else(|| "host no longer exists".to_string())?;
        let at = index as usize;
        if at >= session.forwards.len() {
            return Err("the rule no longer exists".into());
        }
        session.forwards.remove(at);
        store
            .upsert_and_save(session)
            .map_err(|e| format!("save failed: {e}"))?;
        Ok(())
    })();
    match result {
        Ok(()) => to_c(serde_json::json!({"ok": true}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// SSH key credentials referenced by saved sessions (metadata only).
#[no_mangle]
pub extern "C" fn xenterm_keys_json() -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let store = store()?;
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let rows: Vec<serde_json::Value> = xenterm::frontend::key_inventory(&store)
            .into_iter()
            .map(|key| {
                serde_json::json!({
                    "session_id": key.session_id,
                    "session_name": key.session_name,
                    "host": key.host,
                    "user": key.user,
                    "source": key.source,
                    "path": key.path,
                    "full_path": key.full_path,
                    "key_type": key.key_type,
                    "fingerprint": key.fingerprint,
                    "status": key.status,
                })
            })
            .collect();
        serde_json::to_string(&rows).map_err(|e| e.to_string())
    })();
    match result {
        Ok(json) => to_c(json),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Repoints every session that uses a key path to a new one.
/// Payload: `{"old_path":"…","new_path":"…"}`.
#[no_mangle]
pub extern "C" fn xenterm_key_update(json: *const c_char) -> *mut c_char {
    let result = (|| -> Result<usize, String> {
        let raw = unsafe { take_c(json) };
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("invalid payload: {e}"))?;
        let old = value
            .get("old_path")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let new = value
            .get("new_path")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        xenterm::frontend::update_key_path(&mut store, old, new)
    })();
    match result {
        Ok(changed) => to_c(serde_json::json!({"ok": true, "changed": changed}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Drops a key reference from every session (the file is left on disk).
#[no_mangle]
pub extern "C" fn xenterm_key_delete(path: *const c_char) -> *mut c_char {
    let result = (|| -> Result<usize, String> {
        let path = unsafe { take_c(path) };
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        xenterm::frontend::remove_key(&mut store, &path)
    })();
    match result {
        Ok(changed) => to_c(serde_json::json!({"ok": true, "changed": changed}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Saved session transcripts (metadata only) as a JSON array.
#[no_mangle]
pub extern "C" fn xenterm_logs_json() -> *mut c_char {
    let rows: Vec<serde_json::Value> = xenterm::frontend::session_logs()
        .into_iter()
        .map(|log| {
            serde_json::json!({
                "file": log.file,
                "session_id": log.session_id,
                "name": log.name,
                "host": log.host,
                "user": log.user,
                "started_ms": log.started_ms,
                "size": log.size,
            })
        })
        .collect();
    match serde_json::to_string(&rows) {
        Ok(json) => to_c(json),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Reads one transcript: `{"ok":true,"text":"…","meta":{…},"ended_ms":…}`.
#[no_mangle]
pub extern "C" fn xenterm_log_read(name: *const c_char) -> *mut c_char {
    let name = unsafe { take_c(name) };
    match xenterm::frontend::read_session_log(&name) {
        Ok(value) => {
            let mut result = value;
            result["ok"] = serde_json::Value::Bool(true);
            to_c(result.to_string())
        }
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// Deletes one transcript.
#[no_mangle]
pub extern "C" fn xenterm_log_delete(name: *const c_char) -> *mut c_char {
    let name = unsafe { take_c(name) };
    match xenterm::frontend::delete_session_log(&name) {
        Ok(removed) => to_c(serde_json::json!({"ok": true, "removed": removed}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

/// The settings the Flutter UI can change, read from the core config.
#[no_mangle]
pub extern "C" fn xenterm_settings_json() -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let store = store()?;
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        Ok(serde_json::json!({
            "session_logs": store.session_logs(),
            "cursor_style": store.terminal_cursor_style(),
            "line_spacing": store.terminal_line_spacing(),
            "padding": store.terminal_padding(),
            "bold": store.terminal_bold(),
            "sftp_follow_cd": store.sftp_follow_cd(),
            "download_dir": store.download_dir(),
        })
        .to_string())
    })();
    match result {
        Ok(json) => to_c(json),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Applies any subset of the settings above and saves the store.
#[no_mangle]
pub extern "C" fn xenterm_settings_set(json: *const c_char) -> *mut c_char {
    let result = (|| -> Result<(), String> {
        let raw = unsafe { take_c(json) };
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("invalid payload: {e}"))?;
        let store = store()?;
        let mut store = store.lock().unwrap_or_else(|p| p.into_inner());
        let update = xenterm::config::SettingsUpdate {
            session_logs: value.get("session_logs").and_then(|v| v.as_bool()),
            cursor_style: value
                .get("cursor_style")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            line_spacing: value
                .get("line_spacing")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32),
            padding: value.get("padding").and_then(|v| v.as_bool()),
            bold: value.get("bold").and_then(|v| v.as_bool()),
            sftp_follow_cd: value.get("sftp_follow_cd").and_then(|v| v.as_bool()),
            download_dir: value
                .get("download_dir")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        };
        store
            .update_settings_and_save(update)
            .map_err(|e| format!("save failed: {e}"))?;
        Ok(())
    })();
    match result {
        Ok(()) => to_c(serde_json::json!({"ok": true}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

static SFTP_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<xenterm::frontend::SftpSession>>>> =
    OnceLock::new();

fn sftp_sessions() -> &'static Mutex<HashMap<u64, Arc<xenterm::frontend::SftpSession>>> {
    SFTP_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sftp(handle: u64) -> Option<Arc<xenterm::frontend::SftpSession>> {
    sftp_sessions().lock().ok()?.get(&handle).cloned()
}

/// Opens an SFTP channel for a saved session; returns `{"ok":true,"handle":N}`.
#[no_mangle]
pub extern "C" fn xenterm_sftp_start(id: *const c_char) -> *mut c_char {
    let result = (|| -> Result<u64, String> {
        let id = unsafe { take_c(id) };
        if id.is_empty() {
            return Err("empty session id".into());
        }
        let store = store()?;
        let store = store.lock().unwrap_or_else(|p| p.into_inner());
        let runtime = runtime()?;
        let session = xenterm::frontend::SftpSession::start(runtime.handle(), &store, &id)?;
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut map) = sftp_sessions().lock() {
            map.insert(handle, session);
        }
        Ok(handle)
    })();
    match result {
        Ok(handle) => to_c(serde_json::json!({"ok": true, "handle": handle}).to_string()),
        Err(error) => to_c(serde_json::json!({"ok": false, "error": error}).to_string()),
    }
}

#[no_mangle]
pub extern "C" fn xenterm_sftp_version(handle: u64) -> u64 {
    sftp(handle).map(|s| s.version()).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn xenterm_sftp_snapshot(handle: u64) -> *mut c_char {
    match sftp(handle) {
        Some(session) => to_c(session.snapshot_json()),
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn xenterm_sftp_list(handle: u64, path: *const c_char) -> i32 {
    match sftp(handle) {
        Some(session) => {
            session.list(unsafe { take_c(path) });
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn xenterm_sftp_toggle_tree(handle: u64, path: *const c_char) -> i32 {
    match sftp(handle) {
        Some(session) => {
            session.toggle_tree(unsafe { take_c(path) });
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn xenterm_sftp_refresh(handle: u64) -> i32 {
    match sftp(handle) {
        Some(session) => {
            session.refresh();
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn xenterm_sftp_download(
    handle: u64,
    remote: *const c_char,
    local_dir: *const c_char,
) -> i32 {
    match sftp(handle) {
        Some(session) => {
            session.download(unsafe { take_c(remote) }, unsafe { take_c(local_dir) });
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn xenterm_sftp_upload(handle: u64, local: *const c_char) -> i32 {
    match sftp(handle) {
        Some(session) => {
            session.upload(unsafe { take_c(local) });
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn xenterm_sftp_mkdir(handle: u64, name: *const c_char) -> i32 {
    match sftp(handle) {
        Some(session) => {
            session.mkdir(unsafe { take_c(name) });
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn xenterm_sftp_delete(handle: u64, path: *const c_char) -> i32 {
    match sftp(handle) {
        Some(session) => {
            session.delete(unsafe { take_c(path) });
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn xenterm_sftp_answer_prompt(
    handle: u64,
    prompt_id: u64,
    accept: i32,
    username: *const c_char,
    password: *const c_char,
    text: *const c_char,
) -> i32 {
    match sftp(handle) {
        Some(session) => {
            let username = unsafe { take_c(username) };
            let username = if username.is_empty() {
                None
            } else {
                Some(username)
            };
            let password = Some(unsafe { take_c(password) });
            let text = Some(unsafe { take_c(text) });
            session.answer_prompt(prompt_id, accept == 1, username, password, text);
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn xenterm_sftp_close(handle: u64) -> i32 {
    match sftp_sessions().lock() {
        Ok(mut map) => match map.remove(&handle) {
            Some(session) => {
                session.close();
                1
            }
            None => 0,
        },
        Err(_) => 0,
    }
}

/// Release a string this bridge returned.
#[no_mangle]
pub extern "C" fn xenterm_string_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(unsafe { CString::from_raw(ptr) });
    }
}

#[cfg(test)]
mod input_admission_tests {
    use super::*;

    #[test]
    fn ffi_rejects_ssh_input_before_worker_or_prompt_pump_runs() {
        // Never drive this current-thread runtime. The real SSH constructor is
        // exercised without running connection, auth, PTY or profile access.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut config = xenterm::config::Session::new_empty();
        config.kind = xenterm::config::SessionKind::Ssh;
        let terminal =
            TerminalSession::start_adhoc(runtime.handle(), config, false, 80, 24, 100).unwrap();
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
        sessions().lock().unwrap().insert(handle, terminal);
        let bytes = b"synthetic-startup-input";
        let accepted = xenterm_session_input(handle, bytes.as_ptr(), bytes.len());
        // Remove the registry entry even when the pre-fix assertion will fail.
        let terminal = sessions().lock().unwrap().remove(&handle).unwrap();
        terminal.close();
        drop(runtime);
        assert_eq!(accepted, 0);
    }

    #[test]
    fn ffi_prompt_answer_rejects_live_handle_without_prompt_and_removed_handle() {
        // This exercises the real FFI return value without polling any SSH
        // worker, reading a profile or claiming transport-cancellation coverage.
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let mut config = xenterm::config::Session::new_empty();
        config.kind = xenterm::config::SessionKind::Ssh;
        let terminal =
            TerminalSession::start_adhoc(runtime.handle(), config, false, 80, 24, 100).unwrap();
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
        sessions().lock().unwrap().insert(handle, terminal);
        let answer = || xenterm_session_answer_prompt(
            handle, 42, 1, std::ptr::null(), std::ptr::null(), std::ptr::null(),
        );
        let without_prompt = answer();
        let closed = xenterm_session_close(handle);
        let after_close = answer();
        let repeated_close = xenterm_session_close(handle);
        // Do not rely on the close function under test for fixture cleanup.
        // Release the registry lock before closing any leftover session.
        let leftover = sessions().lock().unwrap().remove(&handle);
        if let Some(terminal) = leftover {
            terminal.close();
        }
        drop(runtime);
        // Cleanup precedes assertions so the pre-fix failure does not retain
        // this synthetic registry entry or a dormant runtime.
        assert_eq!(without_prompt, 0);
        assert_eq!(closed, 1);
        assert_eq!(after_close, 0);
        assert_eq!(repeated_close, 0);
    }
}
