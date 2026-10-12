use super::*;

fn policy_store(cache: ConfigFile) -> ConfigStore {
    ConfigStore {
        path: PathBuf::from("unused-policy-fixture.db"),
        backup_dir: None,
        cache,
        key: [0u8; 32],
        keyring_enabled: false,
        saved_state: std::sync::Mutex::new(SavedState::default()).into(),
    }
}

#[test]
fn full_access_opens_capabilities_without_enabling_the_server() {
    let mut store = policy_store(ConfigFile::default());
    assert!(!store.mcp_use_saved_credentials());
    assert!(!store.mcp_allow_commands());
    assert!(!store.mcp_allow_file_transfers());
    store.set_mcp_access_mode(McpAccessMode::Full);
    assert!(!store.mcp_enabled());
    assert!(store.mcp_use_saved_credentials());
    assert!(store.mcp_allow_commands());
    assert!(store.mcp_allow_file_transfers());
    assert!(!store.mcp_execution_log_enabled());
}

#[test]
fn restricted_access_restores_granular_rules_without_enabling_history() {
    let mut store = policy_store(ConfigFile {
        mcp_enabled: true,
        mcp_use_saved_credentials: true,
        mcp_allow_commands: false,
        mcp_allow_file_transfers: false,
        ..ConfigFile::default()
    });
    store.set_mcp_access_mode(McpAccessMode::Full);
    assert!(store.mcp_allow_commands());
    store.set_mcp_access_mode(McpAccessMode::Restricted);
    assert!(store.mcp_enabled());
    assert!(store.mcp_use_saved_credentials());
    assert!(!store.mcp_allow_commands());
    assert!(!store.mcp_allow_file_transfers());
}

#[test]
fn legacy_permissions_are_preserved_without_escalation() {
    let store = policy_store(ConfigFile {
        mcp_use_saved_credentials: true,
        mcp_allow_commands: false,
        mcp_allow_file_transfers: true,
        ..ConfigFile::default()
    });
    assert_eq!(store.mcp_access_mode(), McpAccessMode::Restricted);
    assert!(store.mcp_use_saved_credentials());
    assert!(!store.mcp_allow_commands());
    assert!(store.mcp_allow_file_transfers());
    let encoded = serde_json::to_value(&store.cache).unwrap();
    assert!(encoded.get("mcp_access_mode").is_none());
}

#[test]
fn legacy_full_permissions_are_recognized() {
    let store = policy_store(ConfigFile {
        mcp_use_saved_credentials: true,
        mcp_allow_commands: true,
        mcp_allow_file_transfers: true,
        ..ConfigFile::default()
    });
    assert_eq!(store.mcp_access_mode(), McpAccessMode::Full);
}

#[test]
fn explicit_mode_round_trips_and_invalid_values_are_rejected() {
    let cfg = ConfigFile {
        mcp_access_mode: Some(McpAccessMode::Full),
        ..ConfigFile::default()
    };
    let encoded = serde_json::to_value(&cfg).unwrap();
    assert_eq!(encoded["mcp_access_mode"], "full");
    let restored: ConfigFile = serde_json::from_value(encoded).unwrap();
    assert_eq!(restored.mcp_access_mode, Some(McpAccessMode::Full));
    assert!(serde_json::from_str::<ConfigFile>(r#"{"mcp_access_mode":"anything"}"#).is_err());
    assert!(McpAccessMode::parse("anything").is_none());
    assert_eq!(McpAccessMode::default(), McpAccessMode::Restricted);
}

#[test]
fn execution_history_defaults_off_and_old_approval_settings_are_ignored() {
    let cfg: ConfigFile = serde_json::from_str(r#"{
        "mcp_approval_enabled": true,
        "mcp_audit_retention_days": 365,
        "mcp_risky_patterns": ["fixture"],
        "mcp_approval_timeout_secs": 600
    }"#).unwrap();
    let mut store = policy_store(cfg);
    assert!(!store.mcp_execution_log_enabled());
    assert_eq!(store.mcp_execution_log_retention_days(), 7);
    store.set_mcp_access_mode(McpAccessMode::Full);
    assert!(!store.mcp_execution_log_enabled());
    store.set_mcp_execution_log_enabled(true);
    store.set_mcp_execution_log_retention_days(0);
    assert_eq!(store.mcp_execution_log_retention_days(), 1);
    store.set_mcp_execution_log_enabled(false);
    let saved = serde_json::to_value(&store.cache).unwrap();
    assert_eq!(saved["mcp_execution_log_enabled"], false);
    assert!(saved.get("mcp_approval_enabled").is_none());
    assert!(saved.get("mcp_audit_retention_days").is_none());
}

#[test]
fn fresh_execution_history_matches_missing_field_defaults() {
    let store = policy_store(fresh_config());
    let deserialized: ConfigFile = serde_json::from_str("{}").unwrap();
    assert!(!store.mcp_execution_log_enabled());
    assert_eq!(store.cache.mcp_execution_log_retention_days, 7);
    assert_eq!(store.mcp_execution_log_retention_days(), 7);
    assert_eq!(store.cache.mcp_execution_log_retention_days,
        deserialized.mcp_execution_log_retention_days);
    let restored: ConfigFile = serde_json::from_value(
        serde_json::to_value(&store.cache).unwrap()).unwrap();
    assert_eq!(policy_store(restored).mcp_execution_log_retention_days(), 7);
}
