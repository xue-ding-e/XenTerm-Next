//! Saved dynamic-route admission over a caller-provided configuration view.
//! No profile, key, trust file or network is opened here. This result is not
//! broker authority or a locked snapshot: a future trusted loader must acquire
//! and revalidate those inputs before constructing a broker route snapshot.
use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::config::{ConfigStore, Session};
use super::structs::Frontend;
use super::tools::{optional_u64, required_string};

// Intentionally neither Debug nor Serialize: this contains saved credentials.
pub(super) struct SavedDynamicRoute {
    pub(super) session: Session,
    pub(super) jumps: Vec<Session>,
    pub(super) index: usize,
}

/// The adapter checks argument shape and loads its selected profile (including
/// the MCP server on-switch) before calling this capability gate. Keep parsing
/// here so permission and route errors retain their existing priority.
/// Only saved rules are selected; GUI ephemeral forwards do not enter here.
pub(super) fn admit(store: &ConfigStore, frontend: Frontend, arguments: &Value)
    -> Result<SavedDynamicRoute> {
    if frontend.is_unattended() && (!store.mcp_use_saved_credentials() || !store.mcp_allow_commands()) {
        return Err(anyhow!("dynamic tunnels require saved-credential and arbitrary-command permissions"));
    }
    let id = required_string(arguments, "session_id")?;
    let session = store.get(id).cloned().ok_or_else(|| anyhow!("session not found"))?;
    if session.kind.as_str() != "ssh" { return Err(anyhow!("dynamic tunnels require an SSH session")); }
    let index = match optional_u64(arguments, "forward_index")? {
        Some(value) => usize::try_from(value).map_err(|_| anyhow!("forward_index is too large"))?,
        None => {
            let dynamic: Vec<_> = session.forwards.iter().enumerate().filter(|(_, rule)| rule.kind == "dynamic").collect();
            if dynamic.len() != 1 { return Err(anyhow!("select forward_index from the saved dynamic_forwards; expected exactly one rule when omitted")); }
            dynamic[0].0
        }
    };
    let jumps = store.resolve_jump_chain(&session)?;
    // Match the owner's previous validation order: jump errors precede bind
    // errors. The owner still validates at start; admission grants no lease.
    let rule = session.forwards.get(index)
        .ok_or_else(|| anyhow!("saved forward rule not found"))?;
    crate::tunnel::loopback_bind(rule)?;
    Ok(SavedDynamicRoute { session, jumps, index })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigFile, PortForward, SavedState, SessionKind};
    use serde_json::json;

    const MCP: Frontend = Frontend::Mcp { allow_config_import: false };
    const PERMISSION_ERROR: &str =
        "dynamic tunnels require saved-credential and arbitrary-command permissions";

    // Pure in-memory fixture: never load/save a profile, consult a keyring,
    // change the environment, parse a real key or open any network connection.
    fn store() -> ConfigStore {
        let mut session = Session::new_empty();
        session.id = "target".into();
        session.forwards = vec![dynamic("127.0.0.1")];
        ConfigStore {
            path: "unused-saved-route-fixture.db".into(),
            backup_dir: None,
            cache: ConfigFile { sessions: vec![session], ..ConfigFile::default() },
            key: [0; 32],
            keyring_enabled: false,
            saved_state: std::sync::Mutex::new(SavedState::default()).into(),
        }
    }

    fn dynamic(bind: &str) -> PortForward {
        PortForward { kind: "dynamic".into(), bind_addr: bind.into(), bind_port: 1080,
            ..Default::default() }
    }

    fn error(store: &ConfigStore, frontend: Frontend, arguments: Value) -> String {
        // Do not require Debug on a successful credential-bearing result.
        match admit(store, frontend, &arguments) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("expected saved-route admission to fail"),
        }
    }

    #[test]
    fn capability_gate_precedes_parsing_and_requires_both_permissions() {
        for (credentials, commands) in [(false, false), (false, true), (true, false)] {
            let mut store = store();
            store.set_mcp_use_saved_credentials(credentials);
            store.set_mcp_allow_commands(commands);
            assert_eq!(error(&store, MCP, json!({})), PERMISSION_ERROR);
            assert_eq!(error(&store, MCP, json!({"session_id": "target"})), PERMISSION_ERROR);
        }
    }

    #[test]
    fn capability_gate_does_not_replace_adapter_mcp_on_switch() {
        let mut store = store();
        assert!(!store.mcp_enabled());
        assert!(admit(&store, Frontend::Cli, &json!({"session_id": "target"})).is_ok());
        store.set_mcp_use_saved_credentials(true);
        store.set_mcp_allow_commands(true);
        // load_store, not this pure capability selector, enforces server enablement.
        assert!(admit(&store, MCP, &json!({"session_id": "target"})).is_ok());
    }

    #[test]
    fn session_errors_precede_index_errors() {
        let mut store = store();
        assert_eq!(error(&store, Frontend::Cli, json!({"forward_index": -1})),
            "missing or invalid string argument: session_id");
        assert_eq!(error(&store, Frontend::Cli, json!({"session_id": "missing", "forward_index": -1})),
            "session not found");
        store.cache.sessions[0].kind = SessionKind::Local;
        assert_eq!(error(&store, Frontend::Cli, json!({"session_id": "target", "forward_index": -1})),
            "dynamic tunnels require an SSH session");
    }

    #[test]
    fn omitted_or_null_index_selects_the_only_dynamic_rule() {
        let mut store = store();
        store.cache.sessions[0].forwards.insert(0, PortForward { kind: "local".into(), ..Default::default() });
        for arguments in [json!({"session_id": "target"}), json!({"session_id": "target", "forward_index": null})] {
            let route = admit(&store, Frontend::Cli, &arguments).unwrap();
            assert_eq!(route.index, 1);
            assert_eq!(route.session.id, "target");
            assert!(route.jumps.is_empty());
        }
    }

    #[test]
    fn omitted_index_rejects_zero_or_multiple_dynamic_rules() {
        let mut store = store();
        for rules in [vec![], vec![dynamic("127.0.0.1"), dynamic("::1")]] {
            store.cache.sessions[0].forwards = rules;
            assert_eq!(error(&store, Frontend::Cli, json!({"session_id": "target"})),
                "select forward_index from the saved dynamic_forwards; expected exactly one rule when omitted");
        }
    }

    #[test]
    fn explicit_index_preserves_strict_numeric_errors() {
        let store = store();
        for index in [json!(-1), json!("0"), json!(true), json!(0.5)] {
            assert_eq!(error(&store, Frontend::Cli, json!({"session_id": "target", "forward_index": index})),
                "invalid positive integer argument: forward_index");
        }
        assert_eq!(error(&store, Frontend::Cli, json!({"session_id": "target", "forward_index": 1})),
            "saved forward rule not found");
        assert!(admit(&store, Frontend::Cli, &json!({"session_id": "target", "forward_index": 0})).is_ok());
    }

    #[test]
    fn explicit_non_dynamic_rule_is_rejected() {
        let mut store = store();
        store.cache.sessions[0].forwards[0].kind = "local".into();
        assert_eq!(error(&store, Frontend::Cli, json!({"session_id": "target", "forward_index": 0})),
            "selected forward rule is not dynamic SOCKS5");
    }

    #[test]
    fn loopback_rules_reuse_the_owner_validation() {
        let mut store = store();
        for bind in ["", "localhost", "127.0.0.1", "::1", "[::1]"] {
            store.cache.sessions[0].forwards[0] = dynamic(bind);
            assert!(admit(&store, Frontend::Cli, &json!({"session_id": "target"})).is_ok());
        }
        for bind in ["0.0.0.0", "::", "192.0.2.1", "unresolved.invalid"] {
            let rule = dynamic(bind);
            let expected = crate::tunnel::loopback_bind(&rule).unwrap_err().to_string();
            store.cache.sessions[0].forwards[0] = rule;
            assert_eq!(error(&store, Frontend::Cli, json!({"session_id": "target"})), expected);
        }
    }

    #[test]
    fn jump_errors_precede_rule_bounds_and_bind_errors() {
        let mut store = store();
        store.cache.sessions[0].jump_session_id = "missing-jump".into();
        store.cache.sessions[0].forwards[0] = dynamic("0.0.0.0");
        for index in [0, 99] {
            assert_eq!(error(&store, Frontend::Cli, json!({"session_id": "target", "forward_index": index})),
                "SSH jump session not found: missing-jump");
        }
    }

    #[test]
    fn resolved_jumps_keep_immediate_first_order_and_do_not_read_keys() {
        let mut store = store();
        let mut first = Session::new_empty(); first.id = "first".into();
        let mut last = Session::new_empty(); last.id = "last".into();
        first.jump_session_id = last.id.clone();
        store.cache.sessions[0].jump_session_id = first.id.clone();
        store.cache.sessions[0].auth = crate::config::AuthMethod::Key;
        store.cache.sessions[0].private_key_path = "never-open-this-synthetic-key".into();
        store.cache.sessions.extend([first, last]);
        let route = admit(&store, Frontend::Cli, &json!({"session_id": "target"})).unwrap();
        assert_eq!(route.jumps.iter().map(|hop| hop.id.as_str()).collect::<Vec<_>>(), vec!["first", "last"]);
        assert_eq!(route.session.private_key_path, "never-open-this-synthetic-key");
    }
}
