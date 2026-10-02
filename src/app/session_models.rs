//! The session list's pure projection: grouping, search matching and the
//! built-in local shells.
//!
//! Nothing here names a toolkit type: the UI shell's session list renders these
//! functions' projection through `crate::core::SessionRow`.

use super::*;

fn serial_session_detail(session: &Session) -> String {
    if session.kind != SessionKind::Serial {
        return String::new();
    }
    let parity = match session.parity.as_str() {
        "odd" => "O",
        "even" => "E",
        _ => "N",
    };
    format!(
        "{} · {} baud · {}{}{}",
        session.serial_port, session.baud_rate, session.data_bits, parity, session.stop_bits
    )
}

/// The paste box's parser, which the UI shell shares: the shape of the text is a
/// rule rather than a drawing decision.

fn normalized_query(query: &str) -> String {
    query.trim().to_lowercase()
}

fn session_matches_normalized_query(session: &Session, query: &str) -> bool {
    query.is_empty()
        || session.name.to_lowercase().contains(query)
        || session.host.to_lowercase().contains(query)
}

#[cfg(test)]
fn session_matches_query(session: &Session, query: &str) -> bool {
    let query = normalized_query(query);
    session_matches_normalized_query(session, &query)
}

fn build_session_rows(
    sessions: &[Session],
    explicit_groups: &[String],
    collapsed_groups: Option<&[String]>,
    builtin_sessions: &[Session],
    query: &str,
) -> Vec<crate::core::SessionRow> {
    // Group sessions by their `group` (named groups alphabetically, ungrouped
    // last), then by name within each group, and tag the first row of every
    // group with a header so the welcome list can render a folder heading (#41).
    let query = normalized_query(query);
    let searching = !query.is_empty();
    let matches = |session: &Session| session_matches_normalized_query(session, &query);
    // No saved preference means every group starts expanded: a fresh window
    // that hides the sessions behind a fold is a window that looks empty.
    let group_is_collapsed = |group: &str| {
        !searching
            && collapsed_groups
                .map(|groups| groups.iter().any(|collapsed| collapsed == group))
                .unwrap_or(false)
    };

    // Ordered list of display groups:
    //  - "default" only when there are ungrouped sessions (group == "")
    //  - named groups: explicit folders (incl. empty ones) ∪ sessions' groups,
    //    de-duplicated, alphabetical.
    let has_default = sessions.iter().any(|session| {
        (session.group.is_empty() || is_reserved_session_group(session.group.trim()))
            && matches(session)
    });
    let mut named: Vec<String> = if searching {
        sessions
            .iter()
            .filter(|session| {
                !session.group.is_empty()
                    && !is_reserved_session_group(session.group.trim())
                    && matches(session)
            })
            .map(|session| session.group.clone())
            .collect()
    } else {
        named_display_groups(explicit_groups, sessions)
    };
    named.sort_by_key(|g| g.to_lowercase());
    named.dedup();

    let mut display_groups: Vec<String> = Vec::new();
    if has_default {
        display_groups.push("default".to_string());
    }
    display_groups.extend(named);

    // Placeholder row for an empty folder; id == "" marks it as a group header
    // with no session (used by the UI to gate the "delete group" action).
    let blank = |group: &str| crate::core::SessionRow {
        id: String::new(),
        name: String::new(),
        host: String::new(),
        serial_detail: String::new(),
        port: 0,
        user: String::new(),
        auth: String::new(),
        last_used: String::new(),
        group: group.to_string(),
        group_header: group.to_string(),
        collapsed: group_is_collapsed(group),
        builtin: false,
    };

    let mut rows: Vec<crate::core::SessionRow> = Vec::new();
    for (i, s) in builtin_sessions
        .iter()
        .filter(|session| matches(session))
        .enumerate()
    {
        rows.push(crate::core::SessionRow {
            id: s.id.clone(),
            name: s.name.clone(),
            host: s.host.clone(),
            serial_detail: String::new(),
            port: 0,
            user: s.user.clone(),
            auth: s.kind.as_str().to_string(),
            last_used: String::new(),
            group: "system".to_string(),
            group_header: if i == 0 {
                "system".to_string()
            } else {
                String::new()
            },
            collapsed: group_is_collapsed("system"),
            builtin: true,
        });
    }
    for group in &display_groups {
        let gs: Vec<&Session> = if group == "default" {
            sessions
                .iter()
                .filter(|session| {
                    (session.group.is_empty() || is_reserved_session_group(session.group.trim()))
                        && matches(session)
                })
                .collect()
        } else {
            sessions
                .iter()
                .filter(|session| &session.group == group && matches(session))
                .collect()
        };
        // No alphabetical sort: the stored Vec order is the user's manual
        // order, maintained by drag-to-reorder (same convention as quick
        // commands). New sessions land at the end of their group.
        if gs.is_empty() && !searching {
            rows.push(blank(group));
        } else {
            for (i, s) in gs.iter().enumerate() {
                rows.push(crate::core::SessionRow {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    host: s.host.clone(),
                    serial_detail: serial_session_detail(s),
                    port: s.port as i32,
                    user: s.user.clone(),
                    auth: s.auth.as_str().to_string(),
                    last_used: s.last_used.clone().unwrap_or_else(|| "never".to_string()),
                    group: group.clone(),
                    group_header: if i == 0 { group.clone() } else { String::new() },
                    collapsed: group_is_collapsed(group),
                    builtin: false,
                });
            }
        }
    }
    rows
}

/// Build the session-list rows for the UI shell's session list.
///
/// `build_session_rows` is private to this module, and `session_rows` is the way in for
/// the session list. The list needs the projection — the same grouping, the same search
/// matching, the same heading bookkeeping — and the whole point of
/// `crate::core::SessionRow` is that it can have it. One projection, one rendering, and
/// the grouping rules live in one place.
pub(crate) fn session_rows(
    sessions: &[Session],
    explicit_groups: &[String],
    collapsed_groups: Option<&[String]>,
    builtin_sessions: &[Session],
    query: &str,
) -> Vec<crate::core::SessionRow> {
    build_session_rows(
        sessions,
        explicit_groups,
        collapsed_groups,
        builtin_sessions,
        query,
    )
}

/// The built-in local shells shown alongside saved sessions.
///
/// PowerShell, CMD and any WSL distributions on Windows; `$SHELL` elsewhere. Always
/// present rather than user-created, which is why the list marks them and refuses to
/// edit or delete them. Crate-visible so no list can offer a different set: two lists
/// offering different built-ins would be two answers to "what can I open".
pub(crate) fn builtin_local_sessions(wsl_profiles: &[crate::config::WslProfile]) -> Vec<Session> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        out.push(builtin_local_session(
            "system:powershell",
            "PowerShell",
            "powershell",
        ));
        out.push(builtin_local_session("system:cmd", "CMD", "cmd"));
        if wsl_available() {
            if wsl_profiles.is_empty() {
                let mut session = builtin_local_session("system:wsl", "WSL", "wsl");
                session.local_working_dir = "~".to_string();
                out.push(session);
            } else {
                for profile in wsl_profiles {
                    let mut session = builtin_local_session(
                        &format!("system:wsl:{}", profile.id),
                        profile.name.clone(),
                        "wsl",
                    );
                    session.local_distribution = profile.distribution.clone();
                    session.local_working_dir = if profile.directory.trim().is_empty() {
                        "~".to_string()
                    } else {
                        profile.directory.clone()
                    };
                    out.push(session);
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let name = std::path::Path::new(&shell)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Shell")
            .to_string();
        out.push(builtin_local_session("system:shell", name, "shell"));
    }
    out
}

pub(super) fn builtin_local_session(id: &str, name: impl Into<String>, host: &str) -> Session {
    let mut s = Session::new_empty();
    s.id = id.to_string();
    s.name = name.into();
    s.host = host.to_string();
    s.user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    s.group = "system".to_string();
    s.kind = SessionKind::Local;
    s
}

#[cfg(windows)]
pub(super) fn wsl_available() -> bool {
    use std::os::windows::process::CommandExt;

    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::process::Command::new("wsl.exe")
            .arg("--status")
            .creation_flags(0x08000000)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

// ---------------------------------------------------------------------------
// The writes the session list asks for
// ---------------------------------------------------------------------------
//
// These are the store's own rules rather than the window's: which group a move
// may target, what a duplicate is, and what deleting means. The shell calls them
// and then does the part only a window can do — reloading a page, closing a tab,
// asking a question — so the two answers (does the list need redrawing, was the
// change refused) come back as `bool`/`ImportOutcome` instead of as a return from
// the middle of a UI method.

/// The group a move should put a session in, or `None` when the target is refused.
///
/// The trim is the store's spelling too: a group named only by whitespace is not a
/// group. An empty target means ungrouped. `system` belongs to the built-in local
/// shells and is refused because the menu is built from the store's own groups, so
/// a name that got in some other way must not reach this path.
pub(crate) fn target_group(group: &str) -> Option<String> {
    let group = group.trim();
    if is_reserved_session_group(group) {
        return None;
    }
    Some(group.to_string())
}

/// Put a session in a group, or back among the ungrouped ones, and save.
///
/// The group is set on the session and the session is written back: a session carries
/// its group by name, and there is no separate membership list to keep in step. The
/// write and the `save()` are one step because the saved file is the only state — a move
/// that never reached disk would come back in its old group on the next launch.
///
/// `false` means nothing was written and the caller must not reload or redraw: a
/// missing id and a refused group are both answers, not errors to report.
pub(crate) fn move_session(store: &mut ConfigStore, id: &str, group: &str) -> bool {
    let Some(mut session) = store.get(id).cloned() else {
        tracing::warn!("the list offered session {id} to move, which is not saved");
        return false;
    };
    let Some(target) = target_group(group) else {
        return false;
    };
    session.group = target;
    store.upsert(session);
    if let Err(error) = store.save() {
        tracing::warn!("could not save the moved session: {error:#}");
    }
    true
}

/// Copy a saved session under a new id, ready for the editor to open.
///
/// A duplicate gets a fresh id rather than sharing one: the id is what a tab and a
/// handle are keyed by, so two sessions with one id would be two rows that connect
/// to whichever the map happened to hold. The name is suffixed because two rows
/// reading the same is a list you cannot tell apart.
///
/// The copy is deliberately *not* saved here. It is opened in the editor instead, so
/// the name can be changed before it exists, and the editor's own save is what makes
/// the new id official.
pub(crate) fn duplicate_of(session: &Session) -> Session {
    let mut copy = session.clone();
    copy.id = uuid::Uuid::new_v4().to_string();
    copy.name = format!("{} copy", copy.name);
    copy
}

/// Delete a saved session's record and save the file.
///
/// This is only the store half. The question that precedes it (`window.prompt`), the
/// tab that ends with the session and the page reload are the window's, and stay
/// where they are: this module names no toolkit and a session must be deletable from
/// a caller with no window at all.
pub(crate) fn delete_session(store: &mut ConfigStore, id: &str) {
    store.remove(id);
    if let Err(error) = store.save() {
        tracing::warn!("could not save after deleting a session: {error:#}");
    }
}

/// What an import did, in the two pieces the caller needs.
///
/// `status` is the line the window puts on its status bar — the store's own wording
/// rather than the window's, because the count and the failure both come from here and
/// two translations of one number is two answers. `reload` says whether the session list
/// has to be rebuilt: a rejected file changes nothing on disk, so redrawing on it would
/// be a repaint of the same rows.
pub(crate) struct ImportOutcome {
    pub(crate) status: String,
    pub(crate) reload: bool,
}

/// Add the hosts in `hosts` that are not saved yet, and save if any were.
///
/// The rule about which hosts are new is `crate::core::ssh_import`'s, because more than
/// one path imports the same file: two answers to "is this already saved" is two imports
/// that disagree. The count returned is how many were genuinely added, which is what
/// both import paths report.
fn apply_hosts(store: &mut ConfigStore, hosts: &[crate::ssh::ImportedHost]) -> usize {
    let fresh = crate::core::ssh_import::sessions_to_add(store.sessions(), hosts);
    let added = fresh.len();
    for session in fresh {
        store.upsert(session);
    }
    if added > 0 {
        if let Err(error) = store.save() {
            tracing::warn!("could not save the imported sessions: {error:#}");
        }
    }
    added
}

/// Import the file the user picked, as the app's own export or as an OpenSSH config.
///
/// The app's export is tried first, and `import_from` returning `Err` is the *only*
/// gateway to the OpenSSH fallback: it is the store's way of saying "this file is not
/// one of mine", so swallowing the error would turn a JSON file with a typo in it into
/// an "unrecognised file" answer instead of the parse error the user needs to see. It
/// reads and saves the store itself, and only returns `Ok` when the file was genuinely
/// an export.
///
/// Note the exact wording: the read failure's colon is inside the translated prefix
/// ("read failed:"), which is what the status line has always shown.
pub(crate) fn import_picked_file(store: &mut ConfigStore, path: &std::path::Path) -> ImportOutcome {
    if let Ok((added, skipped)) = store.import_from(path) {
        return ImportOutcome {
            status: format!(
                "{} {added} · {} {skipped}",
                t("已导入", "imported"),
                t("已跳过", "skipped")
            ),
            reload: true,
        };
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            return ImportOutcome {
                status: format!(
                    "{} {}: {error}",
                    t("读取失败", "read failed:"),
                    path.display()
                ),
                reload: false,
            };
        }
    };

    // An OpenSSH config: the same parser the default path uses, pointed at the
    // picked file's text.
    let home = crate::ssh::ssh_config::home_dir().unwrap_or_default();
    let hosts = crate::ssh::ssh_config::parse_str(&text, &home);
    if hosts.is_empty() {
        return ImportOutcome {
            status: t(
                "无法识别的文件：不是 JSON 导出，也没有可导入的主机。",
                "Unrecognised file: not a JSON export and no hosts to import.",
            )
            .to_string(),
            reload: false,
        };
    }
    let added = apply_hosts(store, &hosts);
    ImportOutcome {
        status: if added > 0 {
            format!("{} {}", t("已导入", "imported"), added)
        } else {
            t("没有新主机可导入", "no new hosts to import").to_string()
        },
        reload: true,
    }
}

/// Add the hosts `~/.ssh/config` names, and say how many arrived.
///
/// The count goes on the status line rather than into a dialog, because an import that
/// found nothing is an ordinary answer, not a failure. A config file that is not there
/// at all is the one refusal, and it is also ordinary — plenty of machines have no
/// `~/.ssh/config`.
pub(crate) fn import_ssh_config(store: &mut ConfigStore) -> ImportOutcome {
    let hosts = crate::ssh::ssh_config::parse_default();
    if hosts.is_empty() {
        return ImportOutcome {
            status: t("未找到 ~/.ssh/config", "no ~/.ssh/config found").to_string(),
            reload: false,
        };
    }
    let added = apply_hosts(store, &hosts);
    ImportOutcome {
        status: if added > 0 {
            format!("{} {}", t("已导入", "imported"), added)
        } else {
            t("没有新主机可导入", "no new hosts to import").to_string()
        },
        reload: true,
    }
}

#[cfg(test)]
mod session_edit_tests {
    use super::*;

    #[test]
    fn a_group_that_can_be_a_target_is_trimmed_and_a_reserved_one_is_refused() {
        // Empty means ungrouped, which is the store's own spelling for "default".
        assert_eq!(target_group("  "), Some(String::new()));
        assert_eq!(target_group(" prod "), Some("prod".to_string()));
        // `system` belongs to the built-in local shells, so no menu may move a
        // saved session into it.
        assert_eq!(target_group("system"), None);
        assert_eq!(target_group(" system "), None);
    }

    #[test]
    fn a_duplicate_gets_a_fresh_id_and_a_name_that_can_be_told_apart() {
        let mut original = Session::new_empty();
        original.id = "keep-me".into();
        original.name = "Prod".into();

        let copy = duplicate_of(&original);

        assert_ne!(copy.id, original.id);
        assert_eq!(copy.name, "Prod copy");
        // The original is untouched: both rows would otherwise move together.
        assert_eq!(original.id, "keep-me");
        assert_eq!(original.name, "Prod");
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    fn session(id: &str, name: &str, host: &str, group: &str) -> Session {
        let mut value = Session::new_empty();
        value.id = id.into();
        value.name = name.into();
        value.host = host.into();
        value.group = group.into();
        value
    }

    #[test]
    fn session_search_matches_name_and_host_case_insensitively() {
        let value = session("1", "Prod API", "DB.EXAMPLE.COM", "prod");
        assert!(session_matches_query(&value, "  prod  "));
        assert!(session_matches_query(&value, "example.com"));
        assert!(!session_matches_query(&value, "staging"));
    }

    #[test]
    fn filtered_rows_hide_empty_groups_and_expand_matches() {
        let saved = vec![session("1", "Prod API", "10.0.0.8", "prod")];
        let builtins = vec![session("local", "Local terminal", "localhost", "system")];
        let groups = vec!["empty".to_string(), "prod".to_string()];
        let collapsed = vec!["prod".to_string(), "system".to_string()];

        let rows = build_session_rows(&saved, &groups, Some(&collapsed), &builtins, "prod");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name.as_str(), "Prod API");
        assert_eq!(rows[0].group_header.as_str(), "prod");
        assert!(!rows[0].collapsed);
    }

    #[test]
    fn filtered_rows_include_matching_builtin_sessions() {
        let builtins = vec![session("local", "Local terminal", "localhost", "system")];

        let rows = build_session_rows(
            &[],
            &[],
            Some(&["system".to_string()]),
            &builtins,
            "LOCALHOST",
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name.as_str(), "Local terminal");
        assert_eq!(rows[0].group_header.as_str(), "system");
        assert!(rows[0].builtin);
        assert!(!rows[0].collapsed);
    }

    #[test]
    fn filtered_rows_are_empty_when_nothing_matches() {
        let saved = vec![session("1", "Prod API", "10.0.0.8", "prod")];
        let builtins = vec![session("local", "Local terminal", "localhost", "system")];

        let rows = build_session_rows(&saved, &[], None, &builtins, "staging");

        assert!(rows.is_empty());
    }

    #[test]
    fn empty_query_restores_saved_groups_and_collapse_state() {
        let saved = vec![session("1", "Prod API", "10.0.0.8", "prod")];
        let groups = vec!["empty".to_string(), "prod".to_string()];
        let collapsed = vec!["prod".to_string()];

        let rows = build_session_rows(&saved, &groups, Some(&collapsed), &[], "");

        assert!(rows
            .iter()
            .any(|row| row.group.as_str() == "empty" && row.id.is_empty()));
        assert!(rows
            .iter()
            .any(|row| row.group.as_str() == "prod" && row.collapsed));
    }
}

#[cfg(test)]
mod serial_display_tests {
    use super::*;

    #[test]
    fn serial_rows_show_device_and_framing_instead_of_ssh_defaults() {
        for (device, baud, bits, parity, stops, expected) in [
            (
                "/dev/ttyUSB0",
                115200,
                8,
                "none",
                1,
                "/dev/ttyUSB0 · 115200 baud · 8N1",
            ),
            ("COM3", 9600, 7, "even", 2, "COM3 · 9600 baud · 7E2"),
            (
                "/dev/ttyS0",
                57600,
                8,
                "odd",
                1,
                "/dev/ttyS0 · 57600 baud · 8O1",
            ),
        ] {
            let mut session = Session::new_empty();
            session.kind = SessionKind::Serial;
            session.serial_port = device.into();
            session.baud_rate = baud;
            session.data_bits = bits;
            session.parity = parity.into();
            session.stop_bits = stops;
            let rows = build_session_rows(&[session], &[], None, &[], "");
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].serial_detail.as_str(), expected);
        }
    }

    #[test]
    fn network_rows_keep_their_address_fields() {
        for kind in [SessionKind::Ssh, SessionKind::Telnet] {
            let mut session = Session::new_empty();
            session.kind = kind;
            session.host = "example.com".into();
            session.port = 2222;
            session.user = "alice".into();
            let rows = build_session_rows(&[session], &[], None, &[], "");
            assert!(rows[0].serial_detail.is_empty());
            assert_eq!(rows[0].host.as_str(), "example.com");
            assert_eq!(rows[0].port, 2222);
            assert_eq!(rows[0].user.as_str(), "alice");
        }
    }
}
