//! Synthetic-only fixtures. No real host, credential, OS keyring or user profile.
use super::super::{AuthMethod, PortForward, SessionTrigger};
use super::*;

fn temp_store() -> ConfigStore {
    ConfigStore {
        path: std::env::temp_dir().join(format!("xenterm-import-{}.db", Uuid::new_v4())),
        backup_dir: None,
        cache: ConfigFile::default(),
        key: [7; 32],
        keyring_enabled: false,
        saved_state: std::sync::Mutex::new(SavedState::of_cache(&ConfigFile::default())).into(),
    }
}
fn session(id: &str) -> Session {
    Session {
        id: id.into(),
        name: format!("Synthetic {id}"),
        host: format!("{id}.example.invalid"),
        user: "fixture-user".into(),
        ..Session::new_empty()
    }
}
fn native(sessions: Vec<Session>) -> String {
    serde_json::json!({"sessions": sessions}).to_string()
}
fn snapshot(store: &ConfigStore) -> serde_json::Value {
    serde_json::to_value(&store.cache).unwrap()
}
fn disk(store: &ConfigStore) -> serde_json::Value {
    let conn = rusqlite::Connection::open(&store.path).unwrap();
    let (settings, sessions, history) = ConfigStore::read_disk_store(&conn).unwrap().unwrap();
    serde_json::json!({"settings": settings, "sessions": sessions, "history": history})
}
fn disk_session(store: &ConfigStore, index: usize) -> Session {
    serde_json::from_value(disk(store)["sessions"][index].clone()).unwrap()
}
fn cleanup(store: &ConfigStore) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", store.path.display()));
    }
}

#[test]
fn portable_roundtrip_retains_meatshell_format_credentials_fields_and_jump_references() {
    let mut source = temp_store();
    let outer = session("outer");
    let mut inner = session("inner");
    inner.jump_session_id = outer.id.clone();
    let mut target = session("target");
    target.jump_session_id = inner.id.clone();
    target.jump_session_ids = vec![outer.id.clone(), inner.id.clone()];
    target.password = Secret::new("synthetic-password");
    target.private_key_inline = Secret::new("synthetic-inline-key");
    target.private_key_path = "C:\\synthetic\\key-path".into();
    target.proxy = "socks5://fixture:synthetic-proxy-password@127.0.0.1:1080".into();
    target.triggers.push(SessionTrigger {
        expect: "synthetic prompt".into(),
        response: Secret::new("synthetic-trigger-response"),
        ..Default::default()
    });
    target.group = "Synthetic Group".into();
    target.note = "Synthetic note".into();
    source.cache.sessions = vec![target.clone(), inner, outer];
    let (export, count) = source.export_json().unwrap();
    assert_eq!(count, 3);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&export).unwrap()["meatshell_export"],
        1
    );
    assert!(export.contains("enc:exp:v1:"));
    for secret in [
        "synthetic-password",
        "synthetic-inline-key",
        "synthetic-trigger-response",
        "synthetic-proxy-password",
    ] {
        assert!(!export.contains(secret));
    }
    let mut destination = temp_store();
    destination.key = [9; 32];
    assert_eq!(destination.import_json(&export).unwrap(), (3, 0));
    let imported = &destination.cache.sessions;
    assert_ne!(imported[0].id, target.id);
    assert_eq!(imported[0].jump_session_id, imported[1].id);
    assert_eq!(
        imported[0].jump_session_ids,
        [imported[2].id.clone(), imported[1].id.clone()]
    );
    assert_eq!(imported[1].jump_session_id, imported[2].id);
    assert_eq!(
        destination.resolve_jump_chain(&imported[0]).unwrap().len(),
        2
    );
    let mut comparable = imported[0].clone();
    comparable.id = target.id.clone();
    comparable.jump_session_id = target.jump_session_id.clone();
    comparable.jump_session_ids = target.jump_session_ids.clone();
    assert_eq!(
        serde_json::to_value(comparable).unwrap(),
        serde_json::to_value(target).unwrap()
    );
    let stored = disk_session(&destination, 0);
    for (secret, plaintext) in [
        (&stored.password, "synthetic-password"),
        (&stored.private_key_inline, "synthetic-inline-key"),
        (&stored.triggers[0].response, "synthetic-trigger-response"),
    ] {
        assert_eq!(
            ConfigStore::try_decrypt(&destination.key, secret.as_str()).as_deref(),
            Some(plaintext)
        );
    }
    assert!(!disk(&destination)
        .to_string()
        .contains("synthetic-proxy-password"));
    assert_eq!(destination.import_json(&export).unwrap(), (0, 3));
    cleanup(&destination);
}

#[test]
fn portable_roundtrip_preserves_every_supported_session_field_across_keys_and_disk_reloads() {
    fn reload(store: &ConfigStore) -> ConfigStore {
        // Read the actual SQLite snapshot and decrypt it through the normal load path,
        // without changing global profile selection or consulting an OS keyring.
        let (cache, disk_fingerprint) =
            ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap();
        let saved_state = SavedState {
            disk_fingerprint,
            ..SavedState::of_cache(&cache)
        };
        ConfigStore {
            path: store.path.clone(),
            backup_dir: None,
            cache,
            key: store.key,
            keyring_enabled: false,
            saved_state: std::sync::Mutex::new(saved_state).into(),
        }
    }

    let mut source = temp_store();
    for (index, (id, kind, auth)) in [
        ("ssh-password", SessionKind::Ssh, AuthMethod::Password),
        ("ssh-key", SessionKind::Ssh, AuthMethod::Key),
        (
            "ssh-interactive",
            SessionKind::Ssh,
            AuthMethod::KeyboardInteractive,
        ),
        ("serial", SessionKind::Serial, AuthMethod::Password),
        ("telnet", SessionKind::Telnet, AuthMethod::Password),
        ("local", SessionKind::Local, AuthMethod::Password),
    ]
    .into_iter()
    .enumerate()
    {
        // Spell out every field, including transport-specific metadata on other
        // kinds: adding a Session field must force this coverage fixture to evolve.
        source.cache.sessions.push(Session {
            id: id.into(),
            name: format!("Synthetic 配置 {id}"),
            host: format!("{id}.example.invalid"),
            port: 2200 + index as u16,
            user: format!("fixture-{id}"),
            auth,
            password: Secret::new(format!("synthetic-password-{id}")),
            private_key_path: format!("C:\\synthetic\\{id}.key"),
            private_key_inline: Secret::new(format!("synthetic-inline-key-{id}")),
            allow_secret_reveal: true,
            proxy: format!("socks5h://fixture:synthetic-proxy-{id}%40:p@ss@127.0.0.1:1080"),
            jump_session_id: if id == "ssh-password" {
                "ssh-interactive".into()
            } else if id == "ssh-interactive" {
                "ssh-key".into()
            } else {
                String::new()
            },
            jump_session_ids: if id == "ssh-password" {
                vec!["ssh-key".into(), "ssh-interactive".into()]
            } else {
                Vec::new()
            },
            last_used: Some("2026-01-02T03:04:05Z".into()),
            group: format!("Synthetic 分组 {id}"),
            kind,
            local_distribution: format!("Synthetic-{id}"),
            local_working_dir: format!("/synthetic/{id}/workspace"),
            serial_port: format!("/dev/synthetic-tty-{index}"),
            baud_rate: 9_600,
            data_bits: 7,
            stop_bits: 2,
            parity: "even".into(),
            flow_control: "hardware".into(),
            encoding: "GB18030".into(),
            vt100_drawing: true,
            forwards: ["local", "remote", "dynamic"]
                .into_iter()
                .enumerate()
                .map(|(offset, kind)| PortForward {
                    kind: kind.into(),
                    name: format!("Synthetic {kind} tunnel"),
                    bind_addr: "127.0.0.2".into(),
                    bind_port: 10_800 + offset as u16,
                    host: format!("{kind}.example.invalid"),
                    host_port: 8_000 + offset as u16,
                })
                .collect(),
            triggers: vec![
                SessionTrigger {
                    expect: "Synthetic 提示:".into(),
                    response: Secret::new(format!("synthetic-trigger-{id}")),
                    append_enter: false,
                    repeat: true,
                },
                SessionTrigger {
                    expect: "Synthetic second prompt:".into(),
                    response: Secret::new(format!("synthetic-second-trigger-{id}")),
                    append_enter: true,
                    repeat: false,
                },
            ],
            disable_shell_integration: true,
            note: format!("Synthetic multiline note\n配置 {id}"),
        });
    }
    source.cache.language = "synthetic-source-setting".into();
    source.save().unwrap();
    let source = reload(&source);
    let (export, count) = source.export_json().unwrap();
    assert_eq!(count, 6);
    let exported: serde_json::Value = serde_json::from_str(&export).unwrap();
    assert_eq!(exported.as_object().unwrap().len(), 2);
    assert_eq!(exported["meatshell_export"], 1);
    assert!(!export.contains("synthetic-source-setting"));
    assert!(exported["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["last_used"].is_null()));

    let mut destination = temp_store();
    destination.key = [9; 32];
    destination.cache.language = "synthetic-destination-setting".into();
    destination.save().unwrap();
    let preview = destination.import_json_preview(&export, true).unwrap();
    assert_eq!((preview.added, preview.skipped), (6, 0));
    assert_eq!(preview.warnings.len(), 1);
    assert_eq!(preview.warnings[0].code, "local_permission_reset");
    assert_eq!(preview.warnings[0].field, "allow_secret_reveal");
    assert_eq!(preview.warnings[0].entries, 6);
    assert_eq!(
        destination.import_json_preview(&export, false).unwrap(),
        preview
    );

    let remapped: HashMap<_, _> = source
        .sessions()
        .iter()
        .zip(destination.sessions())
        .map(|(original, imported)| {
            assert_ne!(original.id, imported.id);
            (original.id.clone(), imported.id.clone())
        })
        .collect();
    let mut expected = source.sessions().to_vec();
    for item in &mut expected {
        item.id = remapped[&item.id].clone();
        if !item.jump_session_id.is_empty() {
            item.jump_session_id = remapped[&item.jump_session_id].clone();
        }
        for id in &mut item.jump_session_ids {
            *id = remapped[id].clone();
        }
        // Portable export intentionally drops machine-local recency.
        item.last_used = None;
        // A portable file cannot grant the local GUI permission to reveal secrets.
        item.allow_secret_reveal = false;
    }
    assert_eq!(
        serde_json::to_value(destination.sessions()).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    let disk_json = disk(&destination).to_string();
    for (index, original) in source.sessions().iter().enumerate() {
        let stored = disk_session(&destination, index);
        for (encrypted, plaintext) in [
            (&stored.password, original.password.as_str()),
            (
                &stored.private_key_inline,
                original.private_key_inline.as_str(),
            ),
            (
                &stored.triggers[0].response,
                original.triggers[0].response.as_str(),
            ),
            (
                &stored.triggers[1].response,
                original.triggers[1].response.as_str(),
            ),
        ] {
            assert!(!export.contains(plaintext));
            assert!(!disk_json.contains(plaintext));
            assert!(ConfigStore::try_decrypt(&source.key, encrypted.as_str()).is_none());
            assert_eq!(
                ConfigStore::try_decrypt(&destination.key, encrypted.as_str()).as_deref(),
                Some(plaintext)
            );
        }
        let proxy_password = crate::config::validation::split_proxy_url(&original.proxy)
            .auth
            .unwrap()
            .1;
        assert!(!export.contains(proxy_password));
        assert!(!disk_json.contains(proxy_password));
    }

    let mut reloaded = reload(&destination);
    assert_eq!(
        serde_json::to_value(reloaded.sessions()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(reloaded.cache.language, "synthetic-destination-setting");
    assert_eq!(
        reloaded
            .resolve_jump_chain(&reloaded.sessions()[0])
            .unwrap()
            .len(),
        2
    );
    // Local opt-in survives reimport even when the source carries a different
    // permission: this field is deliberately excluded from duplicate identity.
    reloaded.cache.sessions[0].allow_secret_reveal = true;
    reloaded.save().unwrap();
    let before_repeat = disk(&reloaded);
    assert_eq!(reloaded.import_json(&export).unwrap(), (0, 6));
    assert_eq!(disk(&reloaded), before_repeat);
    let (second_export, second_count) = reloaded.export_json().unwrap();
    assert_eq!(second_count, 6);
    assert_eq!(reloaded.import_json(&second_export).unwrap(), (0, 6));
    assert!(reloaded.sessions()[0].allow_secret_reveal);
    cleanup(&source);
    cleanup(&reloaded);
}

#[test]
fn public_meatshell_v1_fixture_imports_without_reencoding_the_source() {
    // Produced by the public MeatShell CLI/MCP regression fixture, fixed nonce.
    let mut imported = session("legacy");
    imported.password = Secret::new(
        "enc:exp:v1:AAAAAAAAAAAAAAAAFeRqZmIDNa2U57LDoinkeMgDVXvorTTv3qrwXh1pSN4M7bdXvSrE",
    );
    let raw = serde_json::json!({"meatshell_export": 1, "sessions": [imported]}).to_string();
    let mut store = temp_store();
    assert_eq!(store.import_json(&raw).unwrap(), (1, 0));
    assert!(!store.sessions()[0].password.is_empty());
    assert!(!store.sessions()[0].password.as_str().starts_with("enc:"));
    cleanup(&store);
}

#[test]
fn duplicate_hops_are_remapped_and_distinct_aliases_never_overwrite() {
    let mut store = temp_store();
    let existing = session("existing");
    store.cache.sessions.push(existing.clone());
    store.save().unwrap();
    let original_row = disk(&store)["sessions"][0].clone();
    let mut duplicate = existing.clone();
    duplicate.id = "source-hop".into();
    let mut target = session("target");
    target.jump_session_id = duplicate.id.clone();
    target.jump_session_ids = vec![duplicate.id.clone()];
    let mut alias = existing.clone();
    alias.name = "Intentional alias".into();
    alias.password = Secret::new("synthetic-other-password");
    let raw = native(vec![target, duplicate, alias]);
    assert_eq!(store.import_json(&raw).unwrap(), (2, 1));
    assert_eq!(disk(&store)["sessions"][0], original_row);
    assert_eq!(store.sessions()[1].jump_session_id, "existing");
    assert_eq!(store.sessions()[1].jump_session_ids, ["existing"]);
    assert_ne!(store.sessions()[2].id, "existing");
    let before = disk(&store);
    assert_eq!(store.import_json(&raw).unwrap(), (0, 3));
    assert_eq!(disk(&store), before);
    cleanup(&store);
}

#[test]
fn intra_batch_equivalent_sessions_share_a_remapped_id() {
    let mut store = temp_store();
    let first = session("first");
    let mut duplicate = first.clone();
    duplicate.id = "duplicate".into();
    let mut target = session("target");
    target.jump_session_ids = vec![duplicate.id.clone()];
    assert_eq!(
        store
            .import_json(&native(vec![target, first, duplicate]))
            .unwrap(),
        (2, 1)
    );
    assert_eq!(
        store.sessions()[0].jump_session_ids,
        [store.sessions()[1].id.clone()]
    );
    cleanup(&store);
}

#[test]
fn native_import_ignores_global_settings_even_if_they_are_malformed() {
    let mut store = temp_store();
    store.cache.wallpaper = "synthetic-destination-wallpaper".into();
    store.cache.mcp_enabled = false;
    store.cache.mcp_allow_commands = false;
    store.cache.webdav_password = Secret::new("synthetic-existing-webdav-secret");
    store.cache.sessions.push(session("existing"));
    store.save().unwrap();
    let before = snapshot(&store);
    let disk_before = disk(&store);
    let raw = serde_json::json!({"sessions": [session("new")], "wallpaper": 42,
        "mcp_enabled": true, "mcp_allow_commands": true, "webdav_password": "enc:v1:foreign"})
    .to_string();
    let preview = store.import_json_preview(&raw, true).unwrap();
    assert_eq!((preview.added, preview.skipped), (1, 0));
    assert_eq!(preview.warnings.len(), 1);
    assert_eq!(preview.warnings[0].code, "global_settings_ignored");
    assert_eq!(preview.warnings[0].field, "settings");
    assert_eq!(preview.warnings[0].entries, 4);
    assert_eq!(snapshot(&store), before);
    assert_eq!(disk(&store), disk_before);
    assert_eq!(store.import_json_preview(&raw, false).unwrap(), preview);
    let mut after = snapshot(&store);
    after["sessions"] = before["sessions"].clone();
    assert_eq!(after, before);
    assert_eq!(disk(&store)["settings"], disk_before["settings"]);
    cleanup(&store);
}

#[test]
fn preview_is_repeatable_and_never_changes_cache_disk_or_saved_state() {
    let mut store = temp_store();
    store.cache.sessions.push(session("existing"));
    store.save().unwrap();
    let before = snapshot(&store);
    let disk_before = disk(&store);
    let saved_before = store.saved_state.lock().unwrap().sessions.clone();
    for _ in 0..2 {
        assert_eq!(
            store
                .import_json_preview(&native(vec![session("existing"), session("new")]), true)
                .unwrap(),
            ImportSummary {
                added: 1,
                skipped: 1,
                warnings: Vec::new()
            }
        );
        assert_eq!(snapshot(&store), before);
        assert_eq!(disk(&store), disk_before);
        assert_eq!(store.saved_state.lock().unwrap().sessions, saved_before);
    }
    cleanup(&store);
    let mut fresh = temp_store();
    assert_eq!(
        fresh
            .import_json_preview(&native(vec![session("fresh")]), true)
            .unwrap()
            .added,
        1
    );
    assert!(!fresh.path.exists());
}

#[test]
fn invalid_secrets_reject_the_entire_batch_and_redact_error_chains() {
    for field in ["password", "private_key_inline", "trigger", "proxy"] {
        for secret in [
            "enc:v1:synthetic-corrupt",
            "enc:exp:v1:synthetic-corrupt",
            "enc:future:synthetic",
            "keyring:v1",
        ] {
            let mut store = temp_store();
            store.cache.sessions.push(session("existing"));
            store.save().unwrap();
            let before = snapshot(&store);
            let disk_before = disk(&store);
            let mut invalid = session("existing");
            match field {
                "password" => invalid.password = Secret::new(secret),
                "private_key_inline" => invalid.private_key_inline = Secret::new(secret),
                "proxy" => invalid.proxy = format!("socks5://fixture:{secret}@127.0.0.1:1080"),
                _ => invalid.triggers.push(SessionTrigger {
                    response: Secret::new(secret),
                    ..Default::default()
                }),
            }
            for dry_run in [true, false] {
                let error = store
                    .import_json_preview(&native(vec![session("valid"), invalid.clone()]), dry_run)
                    .unwrap_err();
                assert!(!format!("{error:#}").contains(secret));
                assert_eq!(snapshot(&store), before);
                assert_eq!(disk(&store), disk_before);
            }
            cleanup(&store);
        }
    }
}

#[test]
fn native_matching_key_works_and_foreign_key_is_rejected_atomically() {
    let mut store = temp_store();
    let mut imported = session("native");
    imported.password =
        Secret::new(ConfigStore::encrypt(&store.key, "synthetic-local-password").unwrap());
    assert_eq!(store.import_json(&native(vec![imported])).unwrap(), (1, 0));
    assert_eq!(
        store.sessions()[0].password.as_str(),
        "synthetic-local-password"
    );
    let before = disk(&store);
    let mut foreign = session("foreign");
    foreign.password =
        Secret::new(ConfigStore::encrypt(&[99; 32], "synthetic-foreign-password").unwrap());
    assert!(store.import_json(&native(vec![foreign])).is_err());
    assert_eq!(disk(&store), before);
    cleanup(&store);
}

#[test]
fn sqlite_failure_on_second_insert_rolls_back_every_imported_row_and_cache() {
    let mut store = temp_store();
    store.cache.sessions.push(session("existing"));
    store.save().unwrap();
    let before = snapshot(&store);
    let disk_before = disk(&store);
    let conn = rusqlite::Connection::open(&store.path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_second BEFORE INSERT ON sessions WHEN NEW.ordinal = 2 BEGIN SELECT RAISE(ABORT, 'synthetic transaction failure'); END;").unwrap();
    assert!(store
        .import_json(&native(vec![session("first"), session("second")]))
        .is_err());
    assert_eq!(snapshot(&store), before);
    assert_eq!(disk(&store), disk_before);
    cleanup(&store);
}

#[test]
fn external_writer_or_unsaved_edits_are_rejected_without_overwriting_either_side() {
    let mut store = temp_store();
    store.cache.sessions.push(session("existing"));
    store.save().unwrap();
    let before = snapshot(&store);
    let mut external = temp_store();
    external.path = store.path.clone();
    external.cache = store.cache.clone();
    *external.saved_state.lock().unwrap() = store.saved_state.lock().unwrap().clone();
    external.cache.sessions.push(session("external"));
    external.save().unwrap();
    let externally_written = disk(&store);
    assert!(store
        .import_json(&native(vec![session("new")]))
        .unwrap_err()
        .to_string()
        .contains("changed"));
    assert_eq!(snapshot(&store), before);
    assert_eq!(disk(&store), externally_written);
    cleanup(&store);
    let mut store = temp_store();
    store.cache.sessions.push(session("existing"));
    store.save().unwrap();
    let disk_before = disk(&store);
    store.cache.wallpaper = "unsaved".into();
    assert!(store.import_json(&native(vec![session("new")])).is_err());
    assert_eq!(disk(&store), disk_before);
    assert_eq!(store.cache.wallpaper, "unsaved");
    cleanup(&store);
}

#[test]
fn invalid_graphs_ids_hosts_and_paths_are_rejected_before_writing() {
    let mut cases = Vec::new();
    let mut s = session("missing");
    s.jump_session_id = "absent".into();
    cases.push(vec![s]);
    let mut s = session("self");
    s.jump_session_ids = vec!["self".into()];
    cases.push(vec![s]);
    let mut a = session("a");
    let mut b = session("b");
    a.jump_session_id = b.id.clone();
    b.jump_session_id = a.id.clone();
    cases.push(vec![a, b]);
    cases.push(vec![session("duplicate"), session("duplicate")]);
    let mut s = session("empty");
    s.id.clear();
    cases.push(vec![s]);
    let mut nonssh = session("nonssh");
    nonssh.kind = SessionKind::Telnet;
    let mut target = session("target");
    target.jump_session_ids = vec![nonssh.id.clone()];
    cases.push(vec![target, nonssh]);
    let mut hops: Vec<Session> = (0..18).map(|n| session(&format!("hop-{n}"))).collect();
    hops[0].jump_session_ids = hops[1..].iter().map(|s| s.id.clone()).collect();
    cases.push(hops);
    for host in ["", "bad host", "host\r\ninjected", "https://not-a-host"] {
        let mut s = session("host");
        s.host = host.into();
        cases.push(vec![s]);
    }
    let mut s = session("port");
    s.port = 0;
    cases.push(vec![s]);
    let mut s = session("path");
    s.private_key_path = "synthetic\0path".into();
    cases.push(vec![s]);
    for batch in cases {
        let mut store = temp_store();
        let before = snapshot(&store);
        assert!(store.import_json(&native(batch)).is_err());
        assert_eq!(snapshot(&store), before);
        assert!(!store.path.exists());
    }
}

#[test]
fn parse_errors_never_repeat_untrusted_input_and_version_is_checked() {
    let sentinel = "SYNTHETIC_SECRET_MUST_NOT_APPEAR";
    let mut bad = serde_json::to_value(session("bad")).unwrap();
    bad["kind"] = serde_json::json!(sentinel);
    let inputs = [
        serde_json::json!({"meatshell_export":1,"sessions":[bad]}).to_string(),
        serde_json::json!({"meatshell_export":sentinel,"sessions":[]}).to_string(),
        format!("{{\"{sentinel}\": invalid-json}}"),
        r#"{"meatshell_export":2,"sessions":[]}"#.into(),
        r#"{"meatshell_export":1}"#.into(),
        "{}".into(),
        r#"{"sessions":null}"#.into(),
    ];
    for input in inputs {
        let mut store = temp_store();
        let e = store.import_json(&input).unwrap_err();
        assert!(!format!("{e:#}").contains(sentinel));
        assert!(!format!("{e:?}").contains(sentinel));
        assert!(!store.path.exists());
    }
    let mut store = temp_store();
    assert_eq!(store.import_json(r#"{"sessions":[]}"#).unwrap(), (0, 0));
    assert!(!store.path.exists());
}

#[test]
fn files_must_be_regular_bounded_utf8_json() {
    let mut store = temp_store();
    let path = store.path.with_extension("json");
    fs::write(&path, native(vec![session("file")])).unwrap();
    assert_eq!(store.import_from_preview(&path, true).unwrap().added, 1);
    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len((MAX_IMPORT_BYTES + 1) as u64).unwrap();
    drop(file);
    assert!(store
        .import_from(&path)
        .unwrap_err()
        .to_string()
        .contains("16 MiB"));
    assert!(store
        .import_json(&" ".repeat(MAX_IMPORT_BYTES + 1))
        .is_err());
    fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(store.import_from(&path).is_err());
    fs::remove_file(&path).unwrap();
    assert!(store.import_from(&path).is_err());
    fs::create_dir(&path).unwrap();
    assert!(store.import_from(&path).is_err());
    fs::remove_dir(&path).unwrap();
    assert!(!store.path.exists());
}

#[test]
fn invalid_finalshell_batch_is_atomic_and_redacted() {
    let mut store = temp_store();
    let raw = r#"[{"conection_type":100,"host":"valid.example.invalid","password":"AwcLDRETFx1OXQgZJNatCplesw+x/P04"},{"conection_type":100,"host":"SYNTHETIC_SECRET_IN_HOST","password":"not base64"}]"#;
    let e = store.import_json(raw).unwrap_err();
    assert!(!format!("{e:#}").contains("SYNTHETIC_SECRET"));
    assert!(store.sessions().is_empty());
    assert!(!store.path.exists());
}

#[test]
fn decoded_literal_prefixes_survive_import_reload_and_repeated_saves() {
    for literal in [
        "enc:v1:synthetic-literal",
        "enc:exp:v1:synthetic-literal",
        "keyring:v1",
    ] {
        let mut source = temp_store();
        let mut s = session("literal");
        s.password = Secret::new(literal);
        s.private_key_inline = Secret::new(literal);
        s.triggers.push(SessionTrigger {
            response: Secret::new(literal),
            ..Default::default()
        });
        s.proxy = format!("socks5://fixture:{literal}@127.0.0.1:1080");
        source.cache.sessions.push(s);
        let (raw, _) = source.export_json().unwrap();
        let mut destination = temp_store();
        destination.key = [9; 32];
        destination.import_json(&raw).unwrap();
        for _ in 0..2 {
            let mut stored = disk_session(&destination, 0);
            assert!(!disk(&destination).to_string().contains(literal));
            ConfigStore::session_from_disk_form(&mut stored, &destination.key);
            assert_eq!(stored.password.as_str(), literal);
            assert!(!stored.password.is_local_ciphertext());
            assert_eq!(stored.private_key_inline.as_str(), literal);
            assert_eq!(stored.triggers[0].response.as_str(), literal);
            assert!(stored.proxy.contains(literal));
            destination.cache.sessions[0] = stored;
            destination.cache.sessions[0].last_used = Some(Uuid::new_v4().to_string());
            destination.save().unwrap();
        }
        cleanup(&destination);
    }
}

#[test]
fn unrelated_existing_invalid_routes_do_not_prevent_valid_imports() {
    let mut store = temp_store();
    let mut existing = session("existing");
    existing.jump_session_id = "missing".into();
    store.cache.sessions.push(existing);
    store.save().unwrap();
    assert_eq!(
        store.import_json(&native(vec![session("new")])).unwrap(),
        (1, 0)
    );
    cleanup(&store);
}

#[test]
fn reserved_groups_match_editor_semantics_and_stay_idempotent() {
    let mut store = temp_store();
    let mut imported = session("group");
    imported.group = "system".into();
    let raw = native(vec![imported]);
    assert_eq!(store.import_json(&raw).unwrap(), (1, 0));
    assert!(store.sessions()[0].group.is_empty());
    assert_eq!(store.import_json(&raw).unwrap(), (0, 1));
    cleanup(&store);
}

#[test]
#[cfg(not(feature = "desktop"))]
fn headless_master_key_remains_file_backed_and_survives_restarts() {
    let dir = std::env::temp_dir().join(format!("xenterm-headless-key-{}", Uuid::new_v4()));
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("secret.key"), [42u8; 32]).unwrap();
    for portable in [false, true, false] {
        assert_eq!(
            ConfigStore::resolve_master_key(&dir, portable).unwrap(),
            [42u8; 32]
        );
        assert_eq!(fs::read(dir.join("secret.key")).unwrap(), [42u8; 32]);
    }
    let mut stored = session("keyring-placeholder");
    stored.password = Secret::new(ConfigStore::KEYRING_MARKER);
    ConfigStore::session_from_disk_form(&mut stored, &[42u8; 32]);
    assert!(stored.password.is_empty());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn absolute_dns_hostnames_remain_compatible_without_accepting_empty_labels() {
    let mut store = temp_store();
    let mut imported = session("absolute-dns");
    imported.host = "fixture.example.invalid.".into();
    assert_eq!(store.import_json(&native(vec![imported])).unwrap(), (1, 0));
    assert_eq!(store.sessions()[0].host, "fixture.example.invalid.");
    let before = disk(&store);
    let mut invalid = session("invalid-dns");
    invalid.host = "fixture.example.invalid..".into();
    assert!(store.import_json(&native(vec![invalid])).is_err());
    assert_eq!(disk(&store), before);
    cleanup(&store);
}

#[test]
fn compatibility_warnings_are_explicit_non_sensitive_and_preview_matches_apply() {
    let mut imported = serde_json::to_value(session("legacy-options")).unwrap();
    let sentinel = "SYNTHETIC_PRIVATE_VALUE_NEVER_ECHOED";
    imported["session_log"] = serde_json::json!("on");
    imported["allow_secret_reveal"] = serde_json::json!(true);
    imported["rdp_domain"] = serde_json::json!(sentinel);
    imported[sentinel] = serde_json::json!(sentinel);
    let raw = serde_json::json!({"meatshell_export": 1, "sessions": [imported]}).to_string();
    let mut store = temp_store();
    let preview = store.import_json_preview(&raw, true).unwrap();
    assert_eq!(preview.warnings.len(), 4);
    assert!(!serde_json::to_string(&preview).unwrap().contains(sentinel));
    assert_eq!(preview.warnings[0].field, "session_log");
    assert_eq!(preview.warnings[1].field, "allow_secret_reveal");
    assert!(!store.path.exists());
    let applied = store.import_json_preview(&raw, false).unwrap();
    assert_eq!(applied, preview);
    let stored = serde_json::to_value(&store.sessions()[0]).unwrap();
    assert_eq!(stored["allow_secret_reveal"], false);
    assert_eq!(applied.warnings[1].code, "local_permission_reset");
    assert!(stored.get("session_log").is_none());
    let repeated = store.import_json_preview(&raw, false).unwrap();
    assert_eq!((repeated.added, repeated.skipped), (0, 1));
    assert_eq!(repeated.warnings, applied.warnings);
    cleanup(&store);
}

#[test]
fn nested_unknown_fields_and_global_settings_warn_without_echoing_private_names_or_values() {
    let sentinel = "SYNTHETIC_PRIVATE_UNKNOWN_FIELD_AND_VALUE";
    let mut with_nested_fields = serde_json::to_value(session("nested-options")).unwrap();
    with_nested_fields["forwards"] = serde_json::json!([
        {"kind": "local", "bind_port": 1080, "host": "target.example.invalid", "host_port": 22},
        {"kind": "dynamic", "bind_port": 1081}
    ]);
    with_nested_fields["forwards"][0][sentinel] = serde_json::json!(sentinel);
    with_nested_fields["forwards"][1][sentinel] = serde_json::json!(sentinel);
    with_nested_fields["triggers"] = serde_json::json!([
        {"expect": "Synthetic prompt", "response": "synthetic-trigger-response"}
    ]);
    with_nested_fields["triggers"][0][sentinel] = serde_json::json!(sentinel);
    let mut with_session_field = serde_json::to_value(session("top-level-options")).unwrap();
    with_session_field[sentinel] = serde_json::json!(sentinel);
    let mut payload = serde_json::json!({
        "meatshell_export": 1,
        "sessions": [with_nested_fields, with_session_field, session("known-options")],
        "quick_commands": [{"name": sentinel, "command": sentinel}]
    });
    payload[sentinel] = serde_json::json!(sentinel);
    let raw = payload.to_string();
    let mut store = temp_store();
    let before = snapshot(&store);
    let preview = store.import_json_preview(&raw, true).unwrap();
    assert_eq!((preview.added, preview.skipped), (3, 0));
    assert_eq!(preview.warnings.len(), 2);
    let global = preview
        .warnings
        .iter()
        .find(|w| w.code == "global_settings_ignored")
        .unwrap();
    assert_eq!(global.field, "settings");
    assert_eq!(global.entries, 2);
    let unknown = preview
        .warnings
        .iter()
        .find(|w| w.code == "unknown_session_fields")
        .unwrap();
    assert_eq!(unknown.field, "unknown");
    // Count affected sessions, not unknown keys or nested objects. Multiple
    // unknown forwarding/trigger fields in one session still count only once.
    assert_eq!(unknown.entries, 2);
    assert!(!serde_json::to_string(&preview).unwrap().contains(sentinel));
    assert_eq!(snapshot(&store), before);
    assert!(!store.path.exists());

    let applied = store.import_json_preview(&raw, false).unwrap();
    assert_eq!(applied, preview);
    assert!(!snapshot(&store).to_string().contains(sentinel));
    let nested = &store.sessions()[0];
    assert_eq!(nested.forwards.len(), 2);
    assert_eq!(nested.forwards[0].host, "target.example.invalid");
    assert_eq!(nested.forwards[1].kind, "dynamic");
    assert_eq!(
        nested.triggers[0].response.as_str(),
        "synthetic-trigger-response"
    );
    assert!(store.cache.quick_commands.is_empty());
    let repeated = store.import_json_preview(&raw, false).unwrap();
    assert_eq!((repeated.added, repeated.skipped), (0, 3));
    assert_eq!(repeated.warnings, preview.warnings);
    cleanup(&store);
}

#[test]
fn unsupported_transports_reject_the_whole_batch_without_echoing_values() {
    for kind in ["rdp", "SYNTHETIC_PRIVATE_KIND"] {
        let mut unsupported = serde_json::to_value(session("unsupported")).unwrap();
        unsupported["kind"] = serde_json::json!(kind);
        let raw = serde_json::json!({"sessions": [session("valid"), unsupported]}).to_string();
        let mut store = temp_store();
        let error = store.import_json(&raw).unwrap_err().to_string();
        assert!(error.contains("supported kinds"));
        assert!(!error.contains("SYNTHETIC_PRIVATE_KIND"));
        assert!(store.sessions().is_empty());
        assert!(!store.path.exists());
    }
}

fn profile_files(directory: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_string_lossy().to_string(),
                fs::read(path).unwrap(),
            )
        })
        .collect()
}

#[test]
fn explicit_profile_preflight_rejects_missing_foreign_keys_and_keyring_without_changes() {
    for case in [
        "missing-key",
        "wrong-key",
        "keyring",
        "json-keyring",
        "proxy-key",
    ] {
        let directory = std::env::temp_dir().join(format!("xenterm-preflight-{}", Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let mut store = temp_store();
        store.path = directory.join("sessions.db");
        let mut saved = session("preflight");
        saved.password = Secret::new("synthetic-secret");
        if case == "proxy-key" {
            saved.password = Secret::default();
            saved.proxy = "socks5://fixture:synthetic-proxy-secret@127.0.0.1:1080".into();
        }
        store.cache.sessions.push(saved);
        store.save().unwrap();
        if case == "wrong-key" {
            fs::write(directory.join("secret.key"), [99; 32]).unwrap();
        }
        if case == "keyring" {
            let conn = rusqlite::Connection::open(&store.path).unwrap();
            let mut value = serde_json::to_value(&store.sessions()[0]).unwrap();
            value["password"] = serde_json::json!(ConfigStore::KEYRING_MARKER);
            conn.execute("UPDATE sessions SET data=?1", [value.to_string()])
                .unwrap();
        }
        if case == "json-keyring" {
            cleanup(&store);
            let mut value = serde_json::to_value(&store.cache).unwrap();
            value["sessions"][0]["password"] = serde_json::json!(ConfigStore::KEYRING_MARKER);
            fs::write(directory.join("sessions.json"), value.to_string()).unwrap();
        }
        let before = profile_files(&directory);
        let error = ConfigStore::preflight_explicit_profile(&directory).unwrap_err();
        assert!(error.to_string().contains("portable export"));
        assert!(!format!("{error:#}").contains("synthetic-secret"));
        assert_eq!(profile_files(&directory), before);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn explicit_profile_preflight_reads_live_wal_without_touching_source_sidecars() {
    let directory = std::env::temp_dir().join(format!("xenterm-preflight-wal-{}", Uuid::new_v4()));
    fs::create_dir(&directory).unwrap();
    let mut store = temp_store();
    store.path = directory.join("sessions.db");
    store.cache.sessions.push(session("wal"));
    store.save().unwrap();
    let conn = rusqlite::Connection::open(&store.path).unwrap();
    let mut value = serde_json::to_value(&store.sessions()[0]).unwrap();
    value["password"] = serde_json::json!(ConfigStore::KEYRING_MARKER);
    conn.execute("UPDATE sessions SET data=?1", [value.to_string()])
        .unwrap();
    assert!(directory.join("sessions.db-wal").exists());
    let before = profile_files(&directory);
    assert!(ConfigStore::preflight_explicit_profile(&directory).is_err());
    assert_eq!(profile_files(&directory), before);
    drop(conn);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn explicit_profile_preflight_allows_plaintext_legacy_and_matching_local_ciphertext() {
    let directory = std::env::temp_dir().join(format!("xenterm-preflight-ok-{}", Uuid::new_v4()));
    fs::create_dir(&directory).unwrap();
    let mut imported = session("plaintext");
    imported.password = Secret::new("synthetic-legacy-secret");
    fs::write(
        directory.join("sessions.json"),
        native(vec![imported.clone()]),
    )
    .unwrap();
    let before = profile_files(&directory);
    ConfigStore::preflight_explicit_profile(&directory).unwrap();
    assert_eq!(profile_files(&directory), before);
    assert!(!directory.join("secret.key").exists());
    let mut store = temp_store();
    store.path = directory.join("sessions.db");
    store.cache.sessions.push(imported);
    store.save().unwrap();
    fs::write(directory.join("secret.key"), store.key).unwrap();
    let before = profile_files(&directory);
    ConfigStore::preflight_explicit_profile(&directory).unwrap();
    assert_eq!(profile_files(&directory), before);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn proxy_forms_share_connection_mapping_storage_export_and_preflight_semantics() {
    for scheme in ["", "socks5://", "socks5h://", "http://", "SOCKS5://"] {
        for password in [
            "synthetic-proxy-secret",
            "synthetic%40%3A%25secret",
            "synthetic:p@ss%word",
            "enc:v1:synthetic-literal",
        ] {
            let proxy = format!("{scheme}fixture:{password}@127.0.0.1:1080");
            let parsed = crate::config::validation::split_proxy_url(&proxy);
            assert_eq!(parsed.auth.unwrap().1, password);
            let directory =
                std::env::temp_dir().join(format!("xenterm-proxy-forms-{}", Uuid::new_v4()));
            fs::create_dir(&directory).unwrap();
            let mut source = temp_store();
            source.path = directory.join("sessions.db");
            let mut imported = session("proxy-forms");
            imported.proxy = proxy.clone();
            source.cache.sessions.push(imported);
            source.save().unwrap();
            let mut stored = disk_session(&source, 0);
            assert!(!stored.proxy.contains(password));
            assert!(stored
                .proxy
                .starts_with(&format!("{scheme}fixture:enc:v1:")));
            let before = profile_files(&directory);
            assert!(ConfigStore::preflight_explicit_profile(&directory).is_err());
            assert_eq!(profile_files(&directory), before);
            fs::write(directory.join("secret.key"), source.key).unwrap();
            ConfigStore::preflight_explicit_profile(&directory).unwrap();
            ConfigStore::session_from_disk_form(&mut stored, &source.key);
            assert_eq!(stored.proxy, proxy);
            let (export, _) = source.export_json().unwrap();
            assert!(!export.contains(password));
            let mut destination = temp_store();
            destination.key = [9; 32];
            assert_eq!(destination.import_json(&export).unwrap(), (1, 0));
            assert_eq!(destination.sessions()[0].proxy, proxy);
            assert!(!disk(&destination).to_string().contains(password));
            let mut reloaded = disk_session(&destination, 0);
            ConfigStore::session_from_disk_form(&mut reloaded, &destination.key);
            assert_eq!(reloaded.proxy, proxy);
            assert_eq!(
                crate::config::validation::split_proxy_url(&reloaded.proxy)
                    .auth
                    .unwrap()
                    .1,
                password
            );
            cleanup(&destination);
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

#[test]
fn proxy_password_mapping_preserves_empty_and_missing_auth() {
    for proxy in [
        "127.0.0.1:1080",
        "fixture@127.0.0.1:1080",
        "fixture:@127.0.0.1:1080",
        "http://fixture:@127.0.0.1:1080",
    ] {
        assert!(ConfigStore::map_proxy_password(proxy, |_| panic!("no password to map")).is_none());
        let mut imported = session("empty-proxy-password");
        imported.proxy = proxy.into();
        let mut store = temp_store();
        store.import_json(&native(vec![imported])).unwrap();
        assert_eq!(store.sessions()[0].proxy, proxy);
        assert_eq!(disk_session(&store, 0).proxy, proxy);
        cleanup(&store);
    }
}

#[test]
fn preflight_checks_retained_json_after_an_interrupted_empty_database_migration() {
    let directory =
        std::env::temp_dir().join(format!("xenterm-empty-migration-{}", Uuid::new_v4()));
    fs::create_dir_all(&directory).unwrap();
    let database = directory.join("sessions.db");
    drop(ConfigStore::open_db(&database).unwrap());
    let mut original = session("retained");
    original.password =
        Secret::new(ConfigStore::encrypt(&[7; 32], "fixture retained secret").unwrap());
    fs::write(directory.join("sessions.json"), native(vec![original])).unwrap();
    assert!(ConfigStore::preflight_explicit_profile(&directory).is_err());
    assert!(!directory.join("secret.key").exists());
    fs::write(directory.join("secret.key"), [7; 32]).unwrap();
    ConfigStore::preflight_explicit_profile(&directory).unwrap();
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn a_fresh_dirty_profile_cannot_be_implicitly_saved_by_import() {
    for change in ["settings", "history", "session"] {
        let mut store = temp_store();
        match change {
            "settings" => store.cache.wallpaper = "fixture unsaved setting".into(),
            "history" => store
                .cache
                .command_history
                .push("fixture unsaved history".into()),
            "session" => store.cache.sessions.push(session("fixture-unsaved")),
            _ => unreachable!(),
        }
        let before = snapshot(&store);
        for payload in [native(vec![session("incoming")]), native(vec![])] {
            let error = store.import_json(&payload).unwrap_err();
            assert!(error.to_string().contains("pending configuration changes"));
            assert_eq!(snapshot(&store), before);
            assert!(!store.path.exists());
        }
        cleanup(&store);
    }
}

#[test]
fn explicit_profile_rejects_pending_desktop_recovery_without_side_effects() {
    let directory =
        std::env::temp_dir().join(format!("xenterm-pending-recovery-{}", Uuid::new_v4()));
    fs::create_dir_all(&directory).unwrap();
    let journal = directory.join("sessions.db.credential-journal");
    fs::write(&journal, b"synthetic pending encrypted recovery").unwrap();
    let error = ConfigStore::preflight_explicit_profile(&directory).unwrap_err();
    assert!(error.to_string().contains("original desktop"));
    assert_eq!(
        fs::read(&journal).unwrap(),
        b"synthetic pending encrypted recovery"
    );
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn imported_reveal_permission_is_reset_without_duplicate_or_erasing_local_consent() {
    let mut store = temp_store();
    let mut imported = session("reveal-import");
    imported.allow_secret_reveal = true;
    imported.password = Secret::new("synthetic-reveal-import-password");
    let raw = native(vec![imported]);
    let preview = store.import_json_preview(&raw, true).unwrap();
    assert_eq!(preview.warnings.len(), 1);
    assert_eq!(preview.warnings[0].code, "local_permission_reset");
    assert_eq!(store.import_json_preview(&raw, false).unwrap(), preview);
    assert!(!store.sessions()[0].allow_secret_reveal);
    let mut locally_enabled = store.sessions()[0].clone();
    locally_enabled.allow_secret_reveal = true;
    store.upsert_and_save(locally_enabled).unwrap();
    let repeated = store.import_json_preview(&raw, false).unwrap();
    assert_eq!((repeated.added, repeated.skipped), (0, 1));
    assert!(store.sessions()[0].allow_secret_reveal);
    cleanup(&store);
}

#[test]
fn migration_preserves_ids_routes_and_duplicate_aliases() {
    let outer = session("migration-outer");
    let mut target = session("migration-target");
    target.jump_session_ids = vec![outer.id.clone()];
    target.password = Secret::new("synthetic migration secret");
    let mut store = temp_store();
    let raw = native(vec![target.clone(), outer.clone()]);
    let preview = store.import_json_with_ids(&raw, true, true).unwrap();
    assert_eq!(preview.added, 2);
    assert!(!store.path.exists());
    store.import_json_with_ids(&raw, false, true).unwrap();
    assert_eq!(store.sessions()[0].id, target.id);
    assert_eq!(store.sessions()[0].jump_session_ids, vec![outer.id]);
    assert_eq!(
        store
            .import_json_with_ids(&raw, false, true)
            .unwrap()
            .skipped,
        2
    );
    target.name = "conflicting config".into();
    let before = snapshot(&store);
    assert!(store
        .import_json_with_ids(&native(vec![target]), false, true)
        .is_err());
    assert_eq!(snapshot(&store), before);
    cleanup(&store);
}

#[test]
fn native_snapshot_updates_with_matching_key_and_retains_extra_sessions() {
    let mut store = temp_store();
    store.upsert_and_save(session("snapshot-target")).unwrap();
    store.upsert_and_save(session("destination-only")).unwrap();
    let mut changed = session("snapshot-target");
    changed.password =
        Secret::new(ConfigStore::encrypt(&store.key, "synthetic new secret").unwrap());
    let input = store.path.with_extension("source.json");
    fs::write(&input, native(vec![changed.clone()])).unwrap();
    assert_eq!(store.sync_native_snapshot(&input).unwrap(), (1, 0));
    assert_eq!(store.sessions().len(), 2);
    assert_eq!(
        store.sessions()[0].password.as_str(),
        "synthetic new secret"
    );
    assert!(disk_session(&store, 0)
        .password
        .as_str()
        .starts_with("enc:v1:"));
    assert_eq!(store.sync_native_snapshot(&input).unwrap(), (0, 0));
    changed.password =
        Secret::new(ConfigStore::encrypt(&[8; 32], "foreign synthetic secret").unwrap());
    fs::write(&input, native(vec![changed])).unwrap();
    let before = disk(&store);
    assert!(store.sync_native_snapshot(&input).is_err());
    assert_eq!(disk(&store), before);
    fs::remove_file(input).unwrap();
    cleanup(&store);
}

/// Keep all native-sync fixtures on disk only for the duration of one operation.
fn sync_json(store: &mut ConfigStore, raw: &str, dry_run: bool) -> Result<(usize, usize)> {
    let input = tempfile::NamedTempFile::new().unwrap();
    fs::write(input.path(), raw).unwrap();
    store.sync_native_snapshot_preview(input.path(), dry_run)
}

fn saved_snapshot(store: &ConfigStore) -> serde_json::Value {
    let saved = store.saved_state.lock().unwrap();
    serde_json::json!({
        "settings": saved.settings, "order": saved.order, "sessions": saved.sessions,
        "history": saved.history, "fingerprint": saved.disk_fingerprint,
        "submitted": saved.submitted, "attempted": saved.attempted,
        "error": saved.error, "credentials_uncertain": saved.credentials_uncertain,
    })
}

fn raw_disk_snapshot(store: &ConfigStore) -> serde_json::Value {
    let conn = rusqlite::Connection::open(&store.path).unwrap();
    let rows = |query: &str, columns: usize| {
        conn.prepare(query)
            .unwrap()
            .query_map([], |row| {
                (0..columns)
                    .map(|column| row.get::<_, String>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    serde_json::json!({
        "meta": rows("SELECT key,value FROM meta ORDER BY key", 2),
        "sessions": rows("SELECT CAST(ordinal AS TEXT),id,data FROM sessions ORDER BY ordinal,id", 3),
        "history": rows("SELECT CAST(seq AS TEXT),command FROM command_history ORDER BY seq", 2),
    })
}

#[test]
fn both_portable_markers_validate_version_without_global_settings_warnings() {
    for marker in ["meatshell_export", "xenterm_export"] {
        let mut store = temp_store();
        let mut payload = serde_json::json!({"sessions": [session("portable-marker")]});
        payload[marker] = serde_json::json!(1);
        let preview = store
            .import_json_preview(&payload.to_string(), true)
            .unwrap();
        assert_eq!(preview.added, 1);
        assert!(preview.warnings.is_empty());
        assert_eq!(
            sync_json(&mut store, &payload.to_string(), true).unwrap(),
            (0, 1)
        );
        for version in [
            serde_json::json!(0),
            serde_json::json!(2),
            serde_json::json!("1"),
            serde_json::Value::Null,
        ] {
            payload[marker] = version;
            for dry_run in [true, false] {
                assert!(store
                    .import_json_preview(&payload.to_string(), dry_run)
                    .is_err());
                assert!(sync_json(&mut store, &payload.to_string(), dry_run).is_err());
                assert!(store.sessions().is_empty());
                assert!(!store.path.exists());
            }
        }
    }
    let mut store = temp_store();
    // A valid marker cannot conceal an incompatible second marker.
    let conflicting = r#"{"meatshell_export":1,"xenterm_export":2,"sessions":[]}"#;
    assert!(store.import_json_preview(conflicting, true).is_err());
    assert!(sync_json(&mut store, conflicting, true).is_err());
    for missing_sessions in [r#"{"meatshell_export":1}"#, r#"{"xenterm_export":1}"#] {
        assert!(store.import_json_preview(missing_sessions, true).is_err());
        assert!(sync_json(&mut store, missing_sessions, true).is_err());
    }
}

#[test]
fn preserve_ids_keeps_equal_aliases_and_local_reveal_consent_but_rejects_id_conflicts() {
    let mut store = temp_store();
    let mut local = session("preserved");
    local.allow_secret_reveal = true;
    local.last_used = Some("2026-01-02T03:04:05Z".into());
    store.upsert_and_save(local.clone()).unwrap();
    let mut equivalent = local.clone();
    equivalent.allow_secret_reveal = false;
    equivalent.last_used = None;
    let mut alias = equivalent.clone();
    alias.id = "equal-alias".into();
    let payload = native(vec![equivalent.clone(), alias]);
    let preview = store.import_json_with_ids(&payload, true, true).unwrap();
    assert_eq!((preview.added, preview.skipped), (1, 1));
    assert_eq!(
        store.import_json_with_ids(&payload, false, true).unwrap(),
        preview
    );
    assert_eq!(
        store
            .sessions()
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["preserved", "equal-alias"]
    );
    assert_eq!(
        serde_json::to_value(&store.sessions()[0]).unwrap(),
        serde_json::to_value(local).unwrap()
    );
    assert!(!store.sessions()[1].allow_secret_reveal);
    let before = snapshot(&store);
    let disk_before = raw_disk_snapshot(&store);
    equivalent.password = Secret::new("synthetic changed credential");
    for dry_run in [true, false] {
        assert!(store
            .import_json_with_ids(
                &native(vec![session("valid-new"), equivalent.clone()]),
                dry_run,
                true
            )
            .is_err());
        assert_eq!(snapshot(&store), before);
        assert_eq!(raw_disk_snapshot(&store), disk_before);
    }
    cleanup(&store);
}

#[test]
fn native_sync_preserves_destination_rows_settings_history_order_and_local_consent() {
    let mut store = temp_store();
    let mut enabled = session("locally-enabled");
    enabled.allow_secret_reveal = true;
    let disabled = session("locally-disabled");
    let mut extra = session("destination-only");
    extra.password = Secret::new("synthetic destination-only password");
    store.cache.sessions = vec![enabled.clone(), disabled.clone(), extra];
    store.cache.language = "synthetic-local-setting".into();
    store.cache.webdav_password = Secret::new("synthetic-local-webdav");
    store.cache.command_history = vec!["synthetic local command".into()];
    store.save().unwrap();
    let before = raw_disk_snapshot(&store);
    enabled.name = "Synthetic remote edit".into();
    enabled.allow_secret_reveal = false;
    let mut disabled = disabled;
    disabled.allow_secret_reveal = true;
    disabled.password = Secret::new("synthetic remote credential");
    let mut added = session("source-only");
    added.allow_secret_reveal = true;
    added.jump_session_id = enabled.id.clone();
    let payload = serde_json::json!({
        "sessions": [added, disabled, enabled],
        "language": {"malformed": "ignored"},
        "command_history": ["synthetic remote history"],
        "webdav_password": "enc:future:ignored source setting",
        "groups": ["synthetic ignored group"],
    })
    .to_string();
    assert_eq!(sync_json(&mut store, &payload, false).unwrap(), (2, 1));
    assert_eq!(
        store
            .sessions()
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        [
            "locally-enabled",
            "locally-disabled",
            "destination-only",
            "source-only"
        ]
    );
    assert!(store.sessions()[0].allow_secret_reveal);
    assert!(!store.sessions()[1].allow_secret_reveal);
    assert!(!store.sessions()[3].allow_secret_reveal);
    let after = raw_disk_snapshot(&store);
    assert_eq!(after["sessions"][2], before["sessions"][2]);
    assert_eq!(after["history"], before["history"]);
    let without_revision = |value: &serde_json::Value| {
        value["meta"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row[0] != "write_revision")
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(without_revision(&after), without_revision(&before));
    assert!(!after.to_string().contains("synthetic remote credential"));
    assert_eq!(sync_json(&mut store, &payload, false).unwrap(), (0, 0));
    assert_eq!(
        sync_json(&mut store, &native(vec![]), false).unwrap(),
        (0, 0)
    );
    assert_eq!(raw_disk_snapshot(&store), after);
    assert_eq!(store.sessions().len(), 4);
    cleanup(&store);
}

#[test]
fn native_sync_preview_changes_no_profile_files_cache_or_saved_state() {
    for existing in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = temp_store();
        store.path = directory.path().join("sessions.db");
        if existing {
            store.upsert_and_save(session("existing")).unwrap();
        }
        let before = snapshot(&store);
        let saved_before = saved_snapshot(&store);
        let files_before = profile_files(directory.path());
        let mut changed = session("existing");
        changed.password =
            Secret::new(ConfigStore::encrypt(&store.key, "synthetic preview password").unwrap());
        let payload = native(vec![changed, session("new")]);
        for _ in 0..2 {
            assert_eq!(
                sync_json(&mut store, &payload, true).unwrap(),
                if existing { (1, 1) } else { (0, 2) }
            );
            assert_eq!(snapshot(&store), before);
            assert_eq!(saved_snapshot(&store), saved_before);
            assert_eq!(profile_files(directory.path()), files_before);
        }
    }
}

#[test]
fn native_sync_rejects_malformed_ids_paths_users_serial_and_network_endpoints_atomically() {
    let sentinel = "SYNTHETIC_PRIVATE_INPUT";
    let mut invalid = Vec::new();
    for field in [
        "private_key_path",
        "local_working_dir",
        "serial_port",
        "user",
    ] {
        let mut row = serde_json::to_value(session("invalid")).unwrap();
        row[field] = serde_json::json!(format!("{sentinel}\u{0}suffix"));
        invalid.push(row);
    }
    for host in [
        "",
        "bad host",
        "host\r\ninjected",
        "https://not-a-host",
        "a..b",
    ] {
        let mut row = serde_json::to_value(session("invalid")).unwrap();
        row["host"] = serde_json::json!(host);
        invalid.push(row);
    }
    for (field, value) in [
        ("port", serde_json::json!(0)),
        ("id", serde_json::json!(" ")),
        ("kind", serde_json::json!(sentinel)),
    ] {
        let mut row = serde_json::to_value(session("invalid")).unwrap();
        row[field] = value;
        invalid.push(row);
    }
    for port in ["", " \t", "COM3\n", "/dev/tty\0USB0"] {
        let mut row = serde_json::to_value(session("invalid")).unwrap();
        row["kind"] = serde_json::json!("serial");
        row["serial_port"] = serde_json::json!(port);
        invalid.push(row);
    }
    let mut store = temp_store();
    store.upsert_and_save(session("existing")).unwrap();
    let before = snapshot(&store);
    let disk_before = raw_disk_snapshot(&store);
    let saved_before = saved_snapshot(&store);
    let mut changed = session("existing");
    changed.name = "Synthetic valid first edit".into();
    let mut batches: Vec<_> = invalid
        .into_iter()
        .map(|bad| serde_json::json!({"sessions": [changed.clone(), bad]}).to_string())
        .collect();
    batches.push(native(vec![changed.clone(), changed]));
    for payload in batches {
        for dry_run in [true, false] {
            let error = sync_json(&mut store, &payload, dry_run).unwrap_err();
            assert!(!format!("{error:#}").contains(sentinel));
            assert_eq!(snapshot(&store), before);
            assert_eq!(raw_disk_snapshot(&store), disk_before);
            assert_eq!(saved_snapshot(&store), saved_before);
        }
    }
    cleanup(&store);
}

#[test]
fn native_sync_rejects_wrong_keys_and_unresolved_credentials_in_every_secret_field() {
    let mut store = temp_store();
    store.upsert_and_save(session("existing")).unwrap();
    let before = snapshot(&store);
    let disk_before = raw_disk_snapshot(&store);
    let saved_before = saved_snapshot(&store);
    let wrong_key = ConfigStore::encrypt(&[99; 32], "synthetic foreign password").unwrap();
    for field in ["password", "private_key_inline", "trigger", "proxy"] {
        for secret in [
            wrong_key.as_str(),
            "enc:v1:synthetic-corrupt",
            "enc:exp:v1:synthetic-corrupt",
            "enc:future:synthetic",
            "keyring:v1",
        ] {
            let mut invalid = session("existing");
            match field {
                "password" => invalid.password = Secret::new(secret),
                "private_key_inline" => invalid.private_key_inline = Secret::new(secret),
                "proxy" => invalid.proxy = format!("http://fixture:{secret}@127.0.0.1:1080"),
                _ => invalid.triggers.push(SessionTrigger {
                    response: Secret::new(secret),
                    ..Default::default()
                }),
            }
            for dry_run in [true, false] {
                let error = sync_json(
                    &mut store,
                    &native(vec![session("valid-new"), invalid.clone()]),
                    dry_run,
                )
                .unwrap_err();
                assert!(!format!("{error:#}").contains(secret));
                assert_eq!(snapshot(&store), before);
                assert_eq!(raw_disk_snapshot(&store), disk_before);
                assert_eq!(saved_snapshot(&store), saved_before);
            }
        }
    }
    cleanup(&store);
}

#[test]
fn native_sync_validates_the_merged_graph_and_redacts_unknown_jump_ids() {
    let mut store = temp_store();
    let hop = session("existing-hop");
    let mut target = session("destination-target");
    target.jump_session_id = hop.id.clone();
    store.cache.sessions = vec![hop.clone(), target];
    store.save().unwrap();
    let before = snapshot(&store);
    let disk_before = raw_disk_snapshot(&store);
    let saved_before = saved_snapshot(&store);
    let mut cases = Vec::new();
    let mut unknown = session("unknown");
    unknown.jump_session_id = "SYNTHETIC_PRIVATE_UNKNOWN_ID".into();
    cases.push(vec![unknown]);
    let mut changed_hop = hop;
    changed_hop.kind = SessionKind::Telnet;
    cases.push(vec![changed_hop]);
    let mut cycle_a = session("cycle-a");
    let mut cycle_b = session("cycle-b");
    cycle_a.jump_session_id = cycle_b.id.clone();
    cycle_b.jump_session_id = cycle_a.id.clone();
    cases.push(vec![cycle_a, cycle_b]);
    for ids in [
        vec!["self"],
        vec!["existing-hop", "existing-hop"],
        vec![""],
        vec!["SYNTHETIC_PRIVATE_UNKNOWN_ID"],
    ] {
        let mut row = session("self");
        row.jump_session_ids = ids.into_iter().map(String::from).collect();
        cases.push(vec![row]);
    }
    let mut too_long: Vec<_> = (0..18).map(|i| session(&format!("hop-{i}"))).collect();
    too_long[0].jump_session_ids = too_long[1..].iter().map(|s| s.id.clone()).collect();
    cases.push(too_long);
    for batch in cases {
        for dry_run in [true, false] {
            let error = sync_json(&mut store, &native(batch.clone()), dry_run).unwrap_err();
            assert!(error.to_string().contains("SSH jump chain"));
            assert!(!format!("{error:#}").contains("SYNTHETIC_PRIVATE_UNKNOWN_ID"));
            assert_eq!(snapshot(&store), before);
            assert_eq!(raw_disk_snapshot(&store), disk_before);
            assert_eq!(saved_snapshot(&store), saved_before);
        }
    }
    // New rows may reference a destination-only hop, and forward references work.
    let mut new_target = session("new-target");
    new_target.jump_session_ids = vec!["existing-hop".into(), "new-hop".into()];
    assert_eq!(
        sync_json(
            &mut store,
            &native(vec![new_target, session("new-hop")]),
            false
        )
        .unwrap(),
        (0, 2)
    );
    cleanup(&store);
}

#[test]
fn native_sync_sql_failure_rolls_back_updates_inserts_and_baseline_then_can_retry() {
    let mut store = temp_store();
    store.upsert_and_save(session("existing")).unwrap();
    let before = snapshot(&store);
    let disk_before = raw_disk_snapshot(&store);
    let saved_before = saved_snapshot(&store);
    let conn = rusqlite::Connection::open(&store.path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_native_new BEFORE INSERT ON sessions WHEN NEW.id = 'new' BEGIN SELECT RAISE(ABORT, 'synthetic native transaction failure'); END;").unwrap();
    let mut changed = session("existing");
    changed.password = Secret::new("synthetic new password");
    let payload = native(vec![changed, session("new")]);
    assert!(sync_json(&mut store, &payload, false).is_err());
    assert_eq!(snapshot(&store), before);
    assert_eq!(raw_disk_snapshot(&store), disk_before);
    let saved_after = saved_snapshot(&store);
    for key in ["sessions", "order", "settings", "history", "fingerprint"] {
        assert_eq!(saved_after[key], saved_before[key]);
    }
    assert!(store.persistence_error().is_some());
    conn.execute_batch("DROP TRIGGER fail_native_new;").unwrap();
    assert_eq!(sync_json(&mut store, &payload, false).unwrap(), (1, 1));
    assert!(store.persistence_error().is_none());
    drop(conn);
    cleanup(&store);
}

#[test]
fn native_sync_rejects_stale_writers_even_for_unchanged_or_empty_snapshots() {
    for operation in ["update", "noop", "empty"] {
        let mut store = temp_store();
        store.upsert_and_save(session("existing")).unwrap();
        let before = snapshot(&store);
        let mut external = temp_store();
        external.path = store.path.clone();
        external.cache = store.cache.clone();
        *external.saved_state.lock().unwrap() = store.saved_state.lock().unwrap().clone();
        external.upsert_and_save(session("external-new")).unwrap();
        let externally_written = raw_disk_snapshot(&store);
        let mut incoming = session("existing");
        if operation == "update" {
            incoming.name = "Synthetic update".into();
        }
        let payload = native(if operation == "empty" {
            vec![]
        } else {
            vec![incoming]
        });
        let error = sync_json(&mut store, &payload, false).unwrap_err();
        assert!(error
            .downcast_ref::<super::super::ConfigurationChanged>()
            .is_some());
        assert_eq!(snapshot(&store), before);
        assert_eq!(raw_disk_snapshot(&store), externally_written);
        cleanup(&store);
    }
}

#[test]
fn native_sync_rejects_pending_edits_and_uncertain_credentials_without_implicit_save() {
    for existing in [false, true] {
        for pending in ["settings", "history", "session", "uncertain"] {
            let mut store = temp_store();
            if existing {
                store.upsert_and_save(session("existing")).unwrap();
            }
            let disk_before = existing.then(|| raw_disk_snapshot(&store));
            match pending {
                "settings" => store.cache.language = "unsaved synthetic setting".into(),
                "history" => store
                    .cache
                    .command_history
                    .push("unsaved synthetic command".into()),
                "session" => store.cache.sessions.push(session("unsaved")),
                _ => store.saved_state.lock().unwrap().credentials_uncertain = true,
            }
            let before = snapshot(&store);
            for payload in [native(vec![session("new")]), native(vec![])] {
                assert!(sync_json(&mut store, &payload, false).is_err());
                assert_eq!(snapshot(&store), before);
                if let Some(ref disk_before) = disk_before {
                    assert_eq!(&raw_disk_snapshot(&store), disk_before);
                } else {
                    assert!(!store.path.exists());
                }
            }
            cleanup(&store);
        }
    }
}

#[test]
fn native_sync_retains_portable_credential_compatibility_and_cross_platform_path_references() {
    let mut source = temp_store();
    let mut secured = session("secured");
    secured.host = "secured.example.invalid.".into();
    secured.password = Secret::new("synthetic matching password");
    secured.private_key_inline = Secret::new("synthetic matching inline key");
    secured.private_key_path = "C:\\synthetic\\not-mounted.key".into();
    secured.local_working_dir = "/synthetic/not-mounted/workspace".into();
    secured.proxy = "socks5h://fixture:synthetic matching proxy@127.0.0.1:1080".into();
    secured.triggers.push(SessionTrigger {
        response: Secret::new("synthetic matching trigger"),
        ..Default::default()
    });
    source.cache.sessions.push(secured);
    for port in ["COM3", "/dev/synthetic-ttyUSB0"] {
        let mut serial = session(if port == "COM3" {
            "windows-serial"
        } else {
            "unix-serial"
        });
        serial.kind = SessionKind::Serial;
        serial.serial_port = port.into();
        source.cache.sessions.push(serial);
    }
    let expected: Vec<_> = source
        .cache
        .sessions
        .iter()
        .map(|s| serde_json::to_value(s).unwrap())
        .collect();
    for portable in [false, true] {
        let raw = if portable {
            source.export_json().unwrap().0
        } else {
            let mut rows = source.cache.sessions.clone();
            for row in &mut rows {
                ConfigStore::session_to_disk_form(row, &source.key, false).unwrap();
            }
            native(rows)
        };
        let mut destination = temp_store();
        if portable {
            destination.key = [99; 32];
        }
        assert_eq!(sync_json(&mut destination, &raw, true).unwrap(), (0, 3));
        assert_eq!(sync_json(&mut destination, &raw, false).unwrap(), (0, 3));
        assert_eq!(
            destination
                .cache
                .sessions
                .iter()
                .map(|s| serde_json::to_value(s).unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        let stored = raw_disk_snapshot(&destination).to_string();
        for secret in [
            "synthetic matching password",
            "synthetic matching inline key",
            "synthetic matching proxy",
            "synthetic matching trigger",
        ] {
            assert!(!stored.contains(secret));
        }
        let (reloaded, _) =
            ConfigStore::read_cache_snapshot(&destination.path, &destination.key).unwrap();
        assert_eq!(
            reloaded
                .sessions
                .iter()
                .map(|s| serde_json::to_value(s).unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(sync_json(&mut destination, &raw, false).unwrap(), (0, 0));
        cleanup(&destination);
    }
}

#[test]
fn import_modes_reject_reserved_and_control_ids_before_credentials_or_any_profile_mutation() {
    use super::super::tests::{fake_keyring, KEYRING_TESTS};
    let _serial = KEYRING_TESTS.lock().unwrap();
    fake_keyring::install();
    fake_keyring::clear();
    let directory = tempfile::tempdir().unwrap();
    let mut store = temp_store();
    store.path = directory.path().join("sessions.db");
    store.keyring_enabled = false;
    let master =
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, store.key);
    ConfigStore::master_key_entry()
        .unwrap()
        .set_password(&master)
        .unwrap();
    let mut existing = session("existing");
    existing.password = Secret::new("synthetic existing password");
    store.upsert_and_save(existing).unwrap();
    let before = snapshot(&store);
    let disk_before = raw_disk_snapshot(&store);
    let saved_before = saved_snapshot(&store);
    fake_keyring::reset_writes();
    for id in [
        "master-key",
        "MASTER-KEY",
        " Master-Key ",
        "master-key.xenterm\0suffix",
        "ordinary\0suffix",
        "ordinary\nline",
        "ordinary\u{1}separator",
        "ordinary\u{7f}suffix",
        "ordinary\u{85}suffix",
    ] {
        for password in [
            "",
            "synthetic incoming password",
            "enc:future:must-not-decode",
        ] {
            let mut invalid = session("invalid");
            invalid.id = id.into();
            invalid.password = Secret::new(password);
            let payload = native(vec![session("valid-new"), invalid]);
            for dry_run in [true, false] {
                for mode in ["append", "preserve", "native"] {
                    let result = match mode {
                        "native" => sync_json(&mut store, &payload, dry_run).map(|_| ()),
                        "preserve" => store
                            .import_json_with_ids(&payload, dry_run, true)
                            .map(|_| ()),
                        _ => store.import_json_preview(&payload, dry_run).map(|_| ()),
                    };
                    let error = result.unwrap_err();
                    assert!(error.to_string().contains("invalid or reserved session ID"));
                    assert!(!format!("{error:#}").contains(password) || password.is_empty());
                    assert_eq!(snapshot(&store), before);
                    assert_eq!(raw_disk_snapshot(&store), disk_before);
                    assert_eq!(saved_snapshot(&store), saved_before);
                    assert_eq!(fake_keyring::write_count(), 0);
                    assert_eq!(
                        fake_keyring::get(
                            ConfigStore::KEYRING_SERVICE,
                            ConfigStore::MASTER_KEY_ACCOUNT
                        )
                        .as_deref(),
                        Some(master.as_str())
                    );
                }
            }
        }
    }
    fake_keyring::clear();
}

/// Simulate loading a legacy SQL layout. Headless tests intentionally bypass
/// real OS-keyring reads; restore synthetic plaintext from the fixture's cache.
fn use_legacy_ordinals(store: &mut ConfigStore, ordinals: [i64; 2]) {
    let previous = store.cache.sessions.clone();
    {
        let conn = rusqlite::Connection::open(&store.path).unwrap();
        for (row, ordinal) in previous.iter().zip(ordinals) {
            conn.execute(
                "UPDATE sessions SET ordinal=?1 WHERE id=?2",
                rusqlite::params![ordinal, row.id],
            )
            .unwrap();
        }
    }
    let (mut cache, disk_fingerprint) =
        ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap();
    if store.keyring_enabled {
        for row in &mut cache.sessions {
            row.password = previous
                .iter()
                .find(|old| old.id == row.id)
                .unwrap()
                .password
                .clone();
        }
    }
    *store.saved_state.lock().unwrap() = SavedState {
        disk_fingerprint,
        ..SavedState::of_cache(&cache)
    };
    store.cache = cache;
}

#[test]
fn native_sync_preserves_sparse_tied_and_negative_ordinals_in_local_profiles() {
    for ordinals in [[10, 20], [10, 10], [-20, -10], [-10, -10], [i64::MIN, -1]] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = temp_store();
        store.path = directory.path().join("sessions.db");
        let mut a = session("ordinal-a");
        a.password = Secret::new("synthetic untouched ordinal password");
        let b = session("ordinal-b");
        store.cache.sessions = vec![a, b];
        store.save().unwrap();
        use_legacy_ordinals(&mut store, ordinals);
        let before = raw_disk_snapshot(&store);
        let mut changed = store.sessions()[1].clone();
        changed.name = "Synthetic changed ordinal row".into();
        changed.password = Secret::new("synthetic changed ordinal password");
        let payload = native(vec![changed, session("ordinal-c")]);
        assert_eq!(sync_json(&mut store, &payload, false).unwrap(), (1, 1));
        let after = raw_disk_snapshot(&store);
        assert_eq!(after["sessions"][0], before["sessions"][0]);
        assert_eq!(
            after["sessions"][1][0],
            serde_json::json!(ordinals[1].to_string())
        );
        assert_eq!(
            after["sessions"][2][0],
            serde_json::json!((ordinals[1] + 1).to_string())
        );
        let (loaded, _) = ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap();
        assert_eq!(
            loaded
                .sessions
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["ordinal-a", "ordinal-b", "ordinal-c"]
        );
        assert_eq!(
            store
                .sessions()
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["ordinal-a", "ordinal-b", "ordinal-c"]
        );
        assert_eq!(sync_json(&mut store, &payload, false).unwrap(), (0, 0));
        assert_eq!(raw_disk_snapshot(&store), after);
        // A regular partial save also preserves legacy ordinals.
        let mut edited = store.sessions()[1].clone();
        edited.note = "Synthetic ordinary partial edit".into();
        store.upsert_and_save(edited).unwrap();
        let partial = raw_disk_snapshot(&store);
        assert_eq!(partial["sessions"][0], before["sessions"][0]);
        assert_eq!(partial["sessions"][1][0], after["sessions"][1][0]);
        // Explicit display reordering still rebuilds canonical ordinals.
        store.cache.sessions.swap(0, 1);
        store.save().unwrap();
        let (reordered, _) = ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap();
        assert_eq!(
            reordered
                .sessions
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["ordinal-b", "ordinal-a", "ordinal-c"]
        );
        assert_eq!(raw_disk_snapshot(&store)["sessions"][0][0], "0");
    }
}

#[test]
fn append_import_preserves_legacy_ordinals_and_appends_after_the_stored_maximum() {
    for preserve_ids in [false, true] {
        for ordinals in [[10, 20], [10, 10], [-20, -10], [-10, -10]] {
            let directory = tempfile::tempdir().unwrap();
            let mut store = temp_store();
            store.path = directory.path().join("sessions.db");
            store.cache.sessions = vec![session("ordinal-a"), session("ordinal-b")];
            store.save().unwrap();
            use_legacy_ordinals(&mut store, ordinals);
            let before = raw_disk_snapshot(&store);
            let result = store
                .import_json_with_ids(&native(vec![session("ordinal-c")]), false, preserve_ids)
                .unwrap();
            assert_eq!(result.added, 1);
            let after = raw_disk_snapshot(&store);
            assert_eq!(after["sessions"][0], before["sessions"][0]);
            assert_eq!(after["sessions"][1], before["sessions"][1]);
            assert_eq!(
                after["sessions"][2][0],
                serde_json::json!((ordinals[1] + 1).to_string())
            );
            let (loaded, _) = ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap();
            assert_eq!(
                loaded
                    .sessions
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>(),
                store
                    .sessions()
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
            );
            if preserve_ids {
                assert_eq!(loaded.sessions[2].id, "ordinal-c");
            }
        }
    }
}

#[test]
fn native_sync_ordinal_overflow_rolls_back_prior_updates_and_new_rows() {
    for maximum in [i64::MAX - 1, i64::MAX] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = temp_store();
        store.path = directory.path().join("sessions.db");
        let mut a = session("ordinal-a");
        a.password = Secret::new("synthetic original ordinal password");
        store.cache.sessions = vec![a, session("ordinal-b")];
        store.save().unwrap();
        use_legacy_ordinals(&mut store, [10, maximum]);
        let before = snapshot(&store);
        let disk_before = raw_disk_snapshot(&store);
        let baseline = saved_snapshot(&store);
        let mut changed = store.sessions()[0].clone();
        changed.password = Secret::new("synthetic changed ordinal password");
        let mut first = session("ordinal-c");
        first.password = Secret::new("synthetic added ordinal password");
        let payload = native(vec![changed, first, session("ordinal-d")]);
        assert!(sync_json(&mut store, &payload, false).is_err());
        assert_eq!(snapshot(&store), before);
        assert_eq!(raw_disk_snapshot(&store), disk_before);
        let after = saved_snapshot(&store);
        for field in ["sessions", "order", "settings", "history", "fingerprint"] {
            assert_eq!(after[field], baseline[field]);
        }
        assert!(!store.saved_state.lock().unwrap().credentials_uncertain);
        // Updating an existing row at the maximum ordinal remains legal.
        let mut update_only = store.sessions()[1].clone();
        update_only.note = "Synthetic update at ordinal limit".into();
        assert_eq!(
            sync_json(&mut store, &native(vec![update_only]), false).unwrap(),
            (1, 0)
        );
        assert_eq!(
            raw_disk_snapshot(&store)["sessions"][1][0],
            serde_json::json!(maximum.to_string())
        );
    }
}

#[test]
fn append_import_ordinal_overflow_is_atomic_for_remapped_and_preserved_ids() {
    for preserve_ids in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = temp_store();
        store.path = directory.path().join("sessions.db");
        store.cache.sessions = vec![session("ordinal-a"), session("ordinal-b")];
        store.save().unwrap();
        use_legacy_ordinals(&mut store, [10, i64::MAX - 1]);
        let before = snapshot(&store);
        let disk_before = raw_disk_snapshot(&store);
        assert!(store
            .import_json_with_ids(
                &native(vec![session("ordinal-c"), session("ordinal-d")]),
                false,
                preserve_ids
            )
            .is_err());
        assert_eq!(snapshot(&store), before);
        assert_eq!(raw_disk_snapshot(&store), disk_before);
    }
}

#[test]
fn migration_modes_reject_keyring_profiles_before_source_reads_or_any_mutation() {
    use super::super::tests::{fake_keyring, KEYRING_TESTS};
    let _serial = KEYRING_TESTS.lock().unwrap();
    fake_keyring::install();
    for existing in [false, true] {
        fake_keyring::clear();
        let directory = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let mut store = temp_store();
        store.path = directory.path().join("sessions.db");
        store.keyring_enabled = true;
        let master =
            base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, store.key);
        ConfigStore::master_key_entry()
            .unwrap()
            .set_password(&master)
            .unwrap();
        if existing {
            let mut row = session("existing");
            row.password = Secret::new("synthetic original desktop password");
            store.upsert_and_save(row).unwrap();
        }
        let before = snapshot(&store);
        let saved_before = saved_snapshot(&store);
        let files_before = profile_files(directory.path());
        fake_keyring::reset_writes();
        let input = source.path().join("input.json");
        let mut reserved = session("incoming");
        reserved.id = ConfigStore::MASTER_KEY_ACCOUNT.into();
        let mut reserved_with_password = reserved.clone();
        reserved_with_password.password = Secret::new("synthetic incoming password");
        for payload in [
            native(vec![session("valid-new")]),
            native(vec![]),
            "not JSON".into(),
            native(vec![reserved]),
            native(vec![reserved_with_password]),
        ] {
            fs::write(&input, &payload).unwrap();
            for dry_run in [true, false] {
                // The same fixed admission error for missing files/directories
                // establishes that migration never reads the source in this mode.
                for path in [
                    &input,
                    &source.path().join("absent.json"),
                    &source.path().to_path_buf(),
                ] {
                    assert!(store
                        .import_from_preserving_ids(path, dry_run)
                        .unwrap_err()
                        .to_string()
                        .contains("OS-keyring profiles are not supported"));
                    assert!(store
                        .sync_native_snapshot_preview(path, dry_run)
                        .unwrap_err()
                        .to_string()
                        .contains("OS-keyring profiles are not supported"));
                }
                assert!(store
                    .import_json_with_ids(&payload, dry_run, true)
                    .unwrap_err()
                    .to_string()
                    .contains("OS-keyring profiles are not supported"));
                assert_eq!(snapshot(&store), before);
                assert_eq!(saved_snapshot(&store), saved_before);
                assert_eq!(profile_files(directory.path()), files_before);
                assert_eq!(fs::read_to_string(&input).unwrap(), payload);
                assert_eq!(fake_keyring::write_count(), 0);
                assert_eq!(
                    fake_keyring::get(
                        ConfigStore::KEYRING_SERVICE,
                        ConfigStore::MASTER_KEY_ACCOUNT
                    )
                    .as_deref(),
                    Some(master.as_str())
                );
                if existing {
                    assert_eq!(
                        fake_keyring::get(ConfigStore::KEYRING_SERVICE, "existing").as_deref(),
                        Some("synthetic original desktop password")
                    );
                }
            }
            assert!(store
                .sync_native_snapshot(&input)
                .unwrap_err()
                .to_string()
                .contains("OS-keyring profiles are not supported"));
        }
    }
    fake_keyring::clear();
}

#[test]
fn ordinary_keyring_partial_saves_preserve_legacy_ordinals_and_explicit_reorders_still_work() {
    use super::super::tests::{fake_keyring, KEYRING_TESTS};
    let _serial = KEYRING_TESTS.lock().unwrap();
    fake_keyring::install();
    for ordinals in [
        [10, 20],
        [10, 10],
        [-20, -10],
        [-10, -10],
        [i64::MIN, -1],
        [10, i64::MAX],
    ] {
        fake_keyring::clear();
        let directory = tempfile::tempdir().unwrap();
        let mut store = temp_store();
        store.path = directory.path().join("sessions.db");
        store.keyring_enabled = true;
        let mut a = session("ordinal-a");
        a.password = Secret::new("synthetic untouched desktop password");
        let mut b = session("ordinal-b");
        b.password = Secret::new("synthetic original desktop password");
        store.cache.sessions = vec![a, b];
        store.save().unwrap();
        use_legacy_ordinals(&mut store, ordinals);
        let before = raw_disk_snapshot(&store);
        let mut edited = store.sessions()[1].clone();
        edited.password = Secret::new("synthetic updated desktop password");
        fake_keyring::reset_writes();
        store.upsert_and_save(edited).unwrap();
        assert_eq!(fake_keyring::write_count(), 1);
        let after = raw_disk_snapshot(&store);
        assert_eq!(after["sessions"][0], before["sessions"][0]);
        assert_eq!(after["sessions"][1][0], before["sessions"][1][0]);
        assert_eq!(
            disk_session(&store, 1).password.as_str(),
            ConfigStore::KEYRING_MARKER
        );
        assert_eq!(
            fake_keyring::get(ConfigStore::KEYRING_SERVICE, "ordinal-a").as_deref(),
            Some("synthetic untouched desktop password")
        );
        assert_eq!(
            fake_keyring::get(ConfigStore::KEYRING_SERVICE, "ordinal-b").as_deref(),
            Some("synthetic updated desktop password")
        );
        let (loaded, _) = ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap();
        assert_eq!(
            loaded
                .sessions
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["ordinal-a", "ordinal-b"]
        );
        store.cache.sessions.swap(0, 1);
        store.save().unwrap();
        let (reordered, _) = ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap();
        assert_eq!(
            reordered
                .sessions
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["ordinal-b", "ordinal-a"]
        );
        assert_eq!(raw_disk_snapshot(&store)["sessions"][0][0], "0");
    }
    fake_keyring::clear();
}
