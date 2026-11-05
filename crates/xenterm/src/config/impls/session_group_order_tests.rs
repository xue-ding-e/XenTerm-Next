//! Synthetic stores only: no global profile, keyring, or live credentials.
use super::*;

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn fixture() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = tests::temp_store();
    store.path = dir.path().join("sessions.db");
    store.cache.groups = names(&["Zulu", "Alpha"]);
    store.cache.collapsed_session_groups = Some(names(&["Zulu"]));
    store.save().unwrap();
    (dir, store)
}

fn reopen(store: &ConfigStore) -> ConfigStore {
    let (cache, fingerprint) = ConfigStore::read_cache_snapshot(&store.path, &store.key).unwrap();
    ConfigStore {
        path: store.path.clone(),
        backup_dir: None,
        key: store.key,
        keyring_enabled: false,
        saved_state: Mutex::new(SavedState {
            disk_fingerprint: fingerprint,
            ..SavedState::of_cache(&cache)
        })
        .into(),
        cache,
    }
}

#[test]
fn legacy_membership_and_saved_order_are_independent() {
    let (_dir, mut store) = fixture();
    assert!(serde_json::from_str::<ConfigFile>("{}")
        .unwrap()
        .session_group_order
        .is_none());
    store
        .cache
        .groups
        .extend(names(&["", " ", "DEFAULT", "system"]));
    for group in ["Implicit", "alpha", "Ä", "ä", "default", "system", ""] {
        let mut session = tests::sample_session(group);
        session.group = group.into();
        store.cache.sessions.push(session);
    }
    let alphabetical = names(&["Alpha", "Implicit", "Zulu", "Ä", "ä"]);
    assert_eq!(store.ordered_session_groups(), alphabetical);
    let sessions = serde_json::to_value(&store.cache.sessions).unwrap();
    let groups = store.cache.groups.clone();
    let collapsed = store.cache.collapsed_session_groups.clone();
    let wanted = names(&["Zulu", "ä", "Implicit", "Alpha", "Ä"]);
    store
        .reorder_session_groups_and_save(&alphabetical, &wanted)
        .unwrap();
    assert_eq!(store.ordered_session_groups(), wanted);
    assert_eq!(reopen(&store).ordered_session_groups(), wanted);
    assert_eq!(
        serde_json::to_value(&store.cache.sessions).unwrap(),
        sessions
    );
    assert_eq!(store.cache.groups, groups); // implicit groups stay implicit
    assert_eq!(store.cache.collapsed_session_groups, collapsed);
    store.add_group("Aardvark".into());
    store.add_group("Beta".into());
    assert_eq!(
        store.ordered_session_groups(),
        names(&["Zulu", "ä", "Implicit", "Alpha", "Ä", "Aardvark", "Beta"])
    );
}

#[test]
fn rename_delete_and_stale_persisted_names_preserve_remaining_positions() {
    let (_dir, mut store) = fixture();
    store
        .reorder_session_groups_and_save(&names(&["Alpha", "Zulu"]), &names(&["Zulu", "Alpha"]))
        .unwrap();
    store.rename_group("Zulu", "zULU".into());
    assert_eq!(store.ordered_session_groups(), names(&["zULU", "Alpha"]));
    assert_eq!(store.cache.collapsed_session_groups, Some(names(&["zULU"])));
    store.remove_group("Alpha");
    assert_eq!(store.cache.session_group_order, Some(names(&["zULU"])));
    store.cache.session_group_order = Some(names(&["missing", "system", "zULU", "zULU"]));
    store.add_group("Alpha".into());
    assert_eq!(store.ordered_session_groups(), names(&["zULU", "Alpha"]));
    store.save().unwrap();
    assert_eq!(
        reopen(&store).ordered_session_groups(),
        names(&["zULU", "Alpha"])
    );
}

#[test]
fn exact_permutation_and_expected_order_reject_invalid_or_stale_requests() {
    let (_dir, mut store) = fixture();
    let current = store.ordered_session_groups();
    let before = serde_json::to_value(&store.cache).unwrap();
    for invalid in [
        names(&[]),
        names(&["Alpha"]),
        names(&["Alpha", "Alpha"]),
        names(&["Zulu", "unknown"]),
        names(&["default", "Alpha"]),
        names(&["system", "Alpha"]),
        names(&["alpha", "Zulu"]),
    ] {
        assert!(store
            .reorder_session_groups_and_save(&current, &invalid)
            .is_err());
        assert_eq!(serde_json::to_value(&store.cache).unwrap(), before);
    }
    let reversed = names(&["Zulu", "Alpha"]);
    store
        .reorder_session_groups_and_save(&current, &reversed)
        .unwrap();
    assert!(store
        .reorder_session_groups_and_save(&current, &current)
        .is_err());
    store.add_group("Beta".into());
    assert!(store
        .reorder_session_groups_and_save(&reversed, &current)
        .is_err());
    assert_eq!(
        store.ordered_session_groups(),
        names(&["Zulu", "Alpha", "Beta"])
    );
}

#[test]
fn session_boundary_hop_uses_custom_group_order() {
    let (_dir, mut store) = fixture();
    let session = tests::sample_session("ungrouped");
    let id = session.id.clone();
    store.cache.sessions.push(session);
    store.cache.collapsed_session_groups = Some(Vec::new());
    store
        .reorder_session_groups_and_save(&names(&["Alpha", "Zulu"]), &names(&["Zulu", "Alpha"]))
        .unwrap();
    assert!(store.reorder_session(&id, 1));
    assert_eq!(store.cache.sessions[0].group, "Zulu");
    assert!(store.reorder_session(&id, 1));
    assert_eq!(store.cache.sessions[0].group, "Alpha");
}

#[test]
fn conflict_rolls_back_order_without_losing_pending_edits_or_bypassing_cas() {
    let (_dir, mut store) = fixture();
    let mut other = reopen(&store);
    other.set_language("en".into());
    other.save().unwrap();
    store
        .cache
        .command_history
        .push("synthetic pending history".into());
    let before = serde_json::to_value(&store.cache).unwrap();
    let baseline = store.saved_state.lock().unwrap().clone();
    let error = store
        .reorder_session_groups_and_save(&names(&["Alpha", "Zulu"]), &names(&["Zulu", "Alpha"]))
        .unwrap_err();
    assert!(error.downcast_ref::<ConfigurationChanged>().is_some());
    assert_eq!(serde_json::to_value(&store.cache).unwrap(), before);
    let after = store.saved_state.lock().unwrap().clone();
    assert_eq!(after.settings, baseline.settings);
    assert_eq!(after.disk_fingerprint, baseline.disk_fingerprint);
    assert!(after.error.is_some());
    assert_eq!(after.submitted, baseline.submitted + 1);
    assert!(store
        .save()
        .unwrap_err()
        .downcast_ref::<ConfigurationChanged>()
        .is_some());
    assert_eq!(reopen(&store).cache.language, "en");
    assert_eq!(reopen(&store).cache.session_group_order, None);
}

#[test]
fn storage_failure_restores_order_before_any_later_save() {
    let (_dir, mut store) = fixture();
    store
        .reorder_session_groups_and_save(&names(&["Alpha", "Zulu"]), &names(&["Zulu", "Alpha"]))
        .unwrap();
    store.set_language("en".into());
    let before = serde_json::to_value(&store.cache).unwrap();
    let queued_cache = store.cache.clone();
    let queued_sequence = {
        let mut saved = store.saved_state.lock().unwrap();
        saved.submitted += 1;
        saved.submitted
    };
    let held = store.path.with_extension("held");
    fs::rename(&store.path, &held).unwrap();
    fs::create_dir(&store.path).unwrap(); // deterministic open failure even as root
    assert!(store
        .reorder_session_groups_and_save(&names(&["Zulu", "Alpha"]), &names(&["Alpha", "Zulu"]))
        .is_err());
    assert_eq!(serde_json::to_value(&store.cache).unwrap(), before);
    fs::remove_dir(&store.path).unwrap();
    fs::rename(&held, &store.path).unwrap();
    let attempted = store.saved_state.lock().unwrap().attempted;
    assert!(attempted > queued_sequence);
    let disk_before = serde_json::to_value(&reopen(&store).cache).unwrap();
    ConfigStore::run_background_save(
        queued_cache,
        queued_sequence,
        store.saved_state.clone(),
        store.key,
        store.path.clone(),
        None,
        false,
    );
    assert_eq!(store.saved_state.lock().unwrap().attempted, attempted);
    assert_eq!(
        serde_json::to_value(&reopen(&store).cache).unwrap(),
        disk_before
    );
    store.save().unwrap();
    assert_eq!(serde_json::to_value(&reopen(&store).cache).unwrap(), before);
    assert!(store.persistence_error().is_none());
}

#[test]
fn ascii_aliases_deduplicate_even_with_unicode_sort_key_ties() {
    let (_dir, mut store) = fixture();
    store.cache.groups = names(&["K", "K", "k", "Ä", "ä"]);
    assert_eq!(store.ordered_session_groups(), names(&["K", "K", "Ä", "ä"]));
}

#[test]
fn session_aliases_share_siblings_cross_group_targets_and_source_folder() {
    let (_dir, mut store) = fixture();
    store.cache.collapsed_session_groups = Some(Vec::new());
    let mut first = tests::sample_session("first");
    first.group = "alpha".into();
    let first_id = first.id.clone();
    let mut second = tests::sample_session("second");
    second.group = "ALPHA".into();
    let second_id = second.id.clone();
    let mut last = tests::sample_session("last");
    last.group = "zulu".into();
    store.cache.sessions = vec![first, second, last];
    assert!(store.reorder_session(&first_id, 1));
    assert_eq!(store.cache.sessions[0].id, second_id);
    assert_eq!(store.cache.sessions[1].id, first_id);
    // The boundary hop lands before the differently-cased existing member.
    assert!(store.reorder_session(&first_id, 1));
    assert_eq!(store.cache.sessions[1].group, "Zulu");
    assert_eq!(store.cache.sessions[2].group, "zulu");
    // Emptying Alpha must not materialize a duplicate lowercase folder.
    assert!(store.reorder_session(&second_id, 1));
    assert_eq!(store.cache.groups, names(&["Zulu", "Alpha"]));
    assert_eq!(store.cache.sessions[0].group, "Zulu");
    // Moving up from the canonical target also finds the first alias member.
    assert!(store.reorder_session(&second_id, -1));
    assert_eq!(store.cache.sessions[0].group, "Alpha");
}

#[test]
fn non_ascii_groups_remain_distinct_when_moving_sessions() {
    let (_dir, mut store) = fixture();
    store.cache.groups = names(&["Ä", "ä"]);
    store.cache.collapsed_session_groups = Some(Vec::new());
    let mut first = tests::sample_session("upper");
    first.group = "Ä".into();
    let id = first.id.clone();
    let mut second = tests::sample_session("lower");
    second.group = "ä".into();
    store.cache.sessions = vec![first, second];
    assert!(store.reorder_session(&id, 1));
    assert_eq!(store.cache.sessions[0].id, id);
    assert_eq!(store.cache.sessions[0].group, "ä");
    assert_eq!(store.cache.sessions[1].name, "lower");
}

#[test]
fn collapsed_alias_is_skipped_and_can_be_expanded_by_canonical_name() {
    let (_dir, mut store) = fixture();
    store.cache.collapsed_session_groups = Some(names(&["alpha"]));
    let session = tests::sample_session("ungrouped");
    let id = session.id.clone();
    store.cache.sessions.push(session);
    assert!(store.reorder_session(&id, 1));
    assert_eq!(store.cache.sessions[0].group, "Zulu");
    store.set_session_group_collapsed("Alpha", false);
    assert_eq!(store.cache.collapsed_session_groups, Some(Vec::new()));
    assert!(store.reorder_session(&id, -1));
    assert_eq!(store.cache.sessions[0].group, "Alpha");
}

#[test]
fn rename_and_delete_update_all_ascii_aliases_but_preserve_non_ascii_groups() {
    let (_dir, mut store) = fixture();
    store.cache.groups = names(&["K", "k", "K"]);
    store.cache.session_group_order = Some(names(&["k", "K"]));
    store.cache.collapsed_session_groups = Some(names(&["k", "K", "K"]));
    for group in ["K", "k", "K"] {
        let mut session = tests::sample_session(group);
        session.group = group.into();
        store.cache.sessions.push(session);
    }
    store.rename_group("K", "Renamed".into());
    assert_eq!(store.cache.groups, names(&["Renamed", "K"]));
    assert_eq!(
        store.cache.session_group_order,
        Some(names(&["Renamed", "K"]))
    );
    assert_eq!(
        store.cache.collapsed_session_groups,
        Some(names(&["Renamed", "K"]))
    );
    assert_eq!(
        store
            .cache
            .sessions
            .iter()
            .map(|s| s.group.clone())
            .collect::<Vec<_>>(),
        names(&["Renamed", "Renamed", "K"])
    );
    // Deletion accepts an alias and removes all matching metadata and sessions.
    store.cache.sessions[0].group = "RENAMED".into();
    store.cache.groups.push("RENAMED".into());
    store
        .cache
        .session_group_order
        .as_mut()
        .unwrap()
        .push("RENAMED".into());
    store
        .cache
        .collapsed_session_groups
        .as_mut()
        .unwrap()
        .push("RENAMED".into());
    store.remove_group("renamed");
    assert_eq!(store.cache.groups, names(&["K"]));
    assert_eq!(store.cache.session_group_order, Some(names(&["K"])));
    assert_eq!(store.cache.collapsed_session_groups, Some(names(&["K"])));
    assert_eq!(
        store
            .cache
            .sessions
            .iter()
            .map(|s| s.group.clone())
            .collect::<Vec<_>>(),
        names(&["", "", "K"])
    );
    store.save().unwrap();
    assert_eq!(reopen(&store).ordered_session_groups(), names(&["K"]));
}
