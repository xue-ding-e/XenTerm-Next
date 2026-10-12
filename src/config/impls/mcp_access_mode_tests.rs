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
    assert!(!store.mcp_approval_enabled());
}

#[test]
fn restricted_access_restores_granular_rules_and_requires_approval() {
    let mut store = policy_store(ConfigFile {
        mcp_enabled: true,
        mcp_use_saved_credentials: true,
        mcp_allow_commands: false,
        mcp_allow_file_transfers: false,
        mcp_approval_enabled: false,
        ..ConfigFile::default()
    });
    let patterns = store.mcp_risky_patterns();
    store.set_mcp_access_mode(McpAccessMode::Full);
    assert!(store.mcp_allow_commands());
    store.set_mcp_access_mode(McpAccessMode::Restricted);
    assert!(store.mcp_enabled());
    assert!(store.mcp_use_saved_credentials());
    assert!(!store.mcp_allow_commands());
    assert!(!store.mcp_allow_file_transfers());
    assert!(store.mcp_approval_enabled());
    assert_eq!(store.mcp_risky_patterns(), patterns);
    assert!(!store.cache.mcp_approval_enabled);
}

#[test]
fn legacy_permissions_are_preserved_without_escalation() {
    let store = policy_store(ConfigFile {
        mcp_use_saved_credentials: true,
        mcp_allow_commands: false,
        mcp_allow_file_transfers: true,
        mcp_approval_enabled: false,
        ..ConfigFile::default()
    });
    assert_eq!(store.mcp_access_mode(), McpAccessMode::Restricted);
    assert!(store.mcp_use_saved_credentials());
    assert!(!store.mcp_allow_commands());
    assert!(store.mcp_allow_file_transfers());
    assert!(!store.mcp_approval_enabled());
    let encoded = serde_json::to_value(&store.cache).unwrap();
    assert!(encoded.get("mcp_access_mode").is_none());
}

#[test]
fn legacy_full_permissions_are_recognized() {
    let store = policy_store(ConfigFile {
        mcp_use_saved_credentials: true,
        mcp_allow_commands: true,
        mcp_allow_file_transfers: true,
        mcp_approval_enabled: false,
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
