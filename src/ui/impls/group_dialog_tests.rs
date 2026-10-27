//! Real group-manager input and toolkit dialog dismissal, with an isolated store.
use super::*;
use gpui_kit::gpui::{EntityInputHandler as _, TestAppContext, VisualTestContext};
use gpui_kit::test::TestWindowExt as _;
use std::sync::{Arc, Mutex};

fn fixture(cx: &mut TestAppContext) -> (Entity<Shell>, tempfile::TempDir, &mut VisualTestContext) {
    let directory = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::ui::actions::init(cx);
    });
    let store = crate::config::ConfigStore {
        path: directory.path().join("synthetic-groups.db"),
        backup_dir: None,
        cache: crate::config::ConfigFile::default(),
        key: [7; 32],
        keyring_enabled: false,
        saved_state: Mutex::new(crate::config::SavedState::default()).into(),
    };
    let state = SessionState::new(
        Arc::new(tokio::runtime::Runtime::new().unwrap()),
        Arc::new(Mutex::new(std::collections::HashMap::new())),
        Rc::new(RefCell::new(store)),
    );
    let mut shell = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let window_width = Rc::new(Cell::new(1280.));
            let terminal = cx.new(|cx| {
                crate::ui::pages::TerminalPage::new(state.clone(), window_width.clone(), window, cx)
            });
            Shell {
                pages: Pages::new(terminal),
                window_width,
                webdav_task: None,
                persistence_poll: None,
                process_window: Detached::new(process_window_title),
                execution_log_window: crate::ui::ExecutionLogWindowHandle::new(),
                system_info_window: Detached::new(system_info_window_title),
                open_file: None,
                state,
                overlay: Overlay::None,
                persistence_warning: PersistenceWarning::default(),
                status: None,
                status_epoch: 0,
                status_expiry: None,
                quick_connect_pick: None,
                _quick_connect_subscription: None,
            }
        });
        shell = Some(view.clone());
        Root::new(view, window, cx)
    });
    draw(cx);
    // The nav rail has real keyboard focus targets under the Shell key context.
    // Focus one without invoking a click that could open another dialog.
    cx.update(|window, cx| window.focus_next(cx));
    draw(cx);
    (shell.unwrap(), directory, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn settle(cx: &mut VisualTestContext) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut previous = None;
    let mut stable = 0;
    loop {
        draw(cx);
        let card = cx.update(|window, _| window.within("dialog").find(0usize).bounds());
        if previous == Some(card) {
            stable += 1;
        } else {
            stable = 0;
        }
        if stable == 3 {
            break;
        }
        assert!(std::time::Instant::now() < end, "dialog did not settle");
        previous = Some(card);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn open_groups(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> gpui_kit::EntityId {
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_page(PageId::Sessions, window, cx);
            let manager = cx.new(|cx| GroupManagerView::new(shell.state.store.clone(), window, cx));
            shell.overlay = Overlay::Groups(manager.clone());
            Shell::overlay_dialog(manager, "Session groups".into(), 620., 520., window, cx);
            cx.notify();
        })
    });
    settle(cx);
    shell.read_with(cx, |shell, _| match &shell.overlay {
        Overlay::Groups(manager) => manager.entity_id(),
        other => panic!("group overlay missing: {other:?}"),
    })
}

fn focus_input_before(button: &'static str, expected: &str, cx: &mut VisualTestContext) {
    // Click the visible dialog field. Tab traversal from a removed inline
    // rename input can instead focus the background session search box.
    let bounds = cx.update(|window, _| window.within("dialog").find(button).bounds());
    cx.simulate_click(
        gpui_kit::point(bounds.left() - px(100.), bounds.center().y),
        Default::default(),
    );
    draw(cx);
    let value = cx.update(|window, cx| {
        window
            .focused_input(cx)
            .expect("focused dialog input")
            .value(cx)
            .to_string()
    });
    assert_eq!(
        value, expected,
        "pointer must focus the intended visible field"
    );
}

fn click(id: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx.update(|window, _| window.within("dialog").find(id).bounds());
    cx.simulate_click(bounds.center(), Default::default());
    draw(cx);
}

fn assert_closed(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for _ in 0..3 {
        draw(cx);
    }
    assert!(
        !cx.update(|window, cx| window.has_active_dialog(cx)),
        "toolkit dialog must close"
    );
    assert!(
        shell.read_with(cx, |shell, _| matches!(shell.overlay, Overlay::None)),
        "dismissed group manager must release its overlay"
    );
}

fn draft_then_close(cx: &mut TestAppContext, pointer: bool) {
    let (shell, _directory, cx) = fixture(cx);
    let first = open_groups(&shell, cx);
    focus_input_before("group-add", "", cx);
    cx.simulate_input("QA-Unsubmitted");
    draw(cx);
    if pointer {
        click("close", cx);
    } else {
        cx.simulate_keystrokes("escape");
    }
    assert_closed(&shell, cx);
    assert!(shell.read_with(cx, |shell, _| shell
        .state
        .store
        .borrow()
        .groups()
        .is_empty()));
    assert_ne!(open_groups(&shell, cx), first);
    focus_input_before("group-add", "", cx);
    if pointer {
        click("close", cx);
    } else {
        cx.simulate_keystrokes("escape");
    }
    assert_closed(&shell, cx);
}

#[gpui_kit::gpui::test]
fn nonempty_creation_draft_closes_with_the_outer_close_button(cx: &mut TestAppContext) {
    draft_then_close(cx, true);
}

#[gpui_kit::gpui::test]
fn nonempty_creation_draft_closes_with_escape(cx: &mut TestAppContext) {
    draft_then_close(cx, false);
}

fn save_then_close(cx: &mut TestAppContext, pointer: bool) {
    let (shell, _directory, cx) = fixture(cx);
    open_groups(&shell, cx);
    focus_input_before("group-add", "", cx);
    cx.simulate_input("QA-Group-A");
    draw(cx);
    click("group-add", cx);
    assert!(shell.read_with(cx, |shell, _| shell
        .state
        .store
        .borrow()
        .groups()
        .contains(&"QA-Group-A".to_string())));
    click("group-edit-QA-Group-A", cx);
    focus_input_before("group-save-QA-Group-A", "QA-Group-A", cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    cx.simulate_input("QA-Group-B");
    draw(cx);
    click("group-save-QA-Group-A", cx);
    focus_input_before("group-add", "", cx);
    cx.simulate_input("QA-Another-Unsubmitted");
    draw(cx);
    if pointer {
        click("close", cx);
    } else {
        cx.simulate_keystrokes("escape");
    }
    assert_closed(&shell, cx);
    shell.read_with(cx, |shell, _| {
        let store = shell.state.store.borrow();
        assert!(store.groups().contains(&"QA-Group-B".to_string()));
        assert!(!store.groups().contains(&"QA-Group-A".to_string()));
        assert!(!store
            .groups()
            .contains(&"QA-Another-Unsubmitted".to_string()));
    });
    open_groups(&shell, cx);
    focus_input_before("group-add", "", cx);
    click("close", cx);
    assert_closed(&shell, cx);
}

#[gpui_kit::gpui::test]
fn added_and_renamed_groups_survive_outer_close_without_saving_a_draft(cx: &mut TestAppContext) {
    save_then_close(cx, true);
}

#[gpui_kit::gpui::test]
fn added_and_renamed_groups_survive_escape_without_saving_a_draft(cx: &mut TestAppContext) {
    save_then_close(cx, false);
}
fn close_after_finishing_the_focused_rename(cx: &mut TestAppContext, save: bool) {
    let (shell, _directory, cx) = fixture(cx);
    open_groups(&shell, cx);
    focus_input_before("group-add", "", cx);
    cx.simulate_input("QA-Focused-A");
    draw(cx);
    click("group-add", cx);
    focus_input_before("group-add", "", cx);
    cx.simulate_input("QA-Preserved-Draft");
    draw(cx);
    click("group-edit-QA-Focused-A", cx);
    focus_input_before("group-save-QA-Focused-A", "QA-Focused-A", cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    cx.simulate_input("QA-Focused-B");
    draw(cx);
    click(
        if save {
            "group-save-QA-Focused-A"
        } else {
            "group-cancel-QA-Focused-A"
        },
        cx,
    );
    let retained_draft = cx.update(|window, cx| {
        window
            .focused_input(cx)
            .expect("focus handed to retained creation input")
            .value(cx)
            .to_string()
    });
    assert_eq!(retained_draft, "QA-Preserved-Draft");
    // Crucially, no click or Tab re-focuses the creation input before the
    // toolkit's close button dispatches Cancel through the current focus path.
    click("close", cx);
    assert_closed(&shell, cx);
    shell.read_with(cx, |shell, _| {
        let store = shell.state.store.borrow();
        assert!(store
            .groups()
            .contains(&if save { "QA-Focused-B" } else { "QA-Focused-A" }.to_string()));
        assert!(!store.groups().contains(&"QA-Preserved-Draft".to_string()));
    });
    open_groups(&shell, cx);
    focus_input_before("group-add", "", cx);
    click("close", cx);
    assert_closed(&shell, cx);
}

#[gpui_kit::gpui::test]
fn saving_the_focused_inline_rename_leaves_the_outer_close_button_working(cx: &mut TestAppContext) {
    close_after_finishing_the_focused_rename(cx, true);
}

#[gpui_kit::gpui::test]
fn cancelling_the_focused_inline_rename_leaves_the_outer_close_button_working(
    cx: &mut TestAppContext,
) {
    close_after_finishing_the_focused_rename(cx, false);
}

#[gpui_kit::gpui::test]
fn switching_from_a_focused_rename_keeps_the_dialog_close_button_working(cx: &mut TestAppContext) {
    let (shell, _directory, cx) = fixture(cx);
    open_groups(&shell, cx);
    for name in ["QA-Switch-A", "QA-Switch-B"] {
        focus_input_before("group-add", "", cx);
        cx.simulate_input(name);
        draw(cx);
        click("group-add", cx);
    }
    click("group-edit-QA-Switch-A", cx);
    focus_input_before("group-save-QA-Switch-A", "QA-Switch-A", cx);
    cx.simulate_input("-unsaved");
    draw(cx);
    // This destroys A's focused input without a Save/Cancel click.
    click("group-edit-QA-Switch-B", cx);
    // No intervening input focus operation may repair the close route.
    click("close", cx);
    assert_closed(&shell, cx);
    shell.read_with(cx, |shell, _| {
        let groups = shell.state.store.borrow().groups().to_vec();
        assert_eq!(groups.len(), 2);
        assert!(groups.contains(&"QA-Switch-A".to_string()));
        assert!(groups.contains(&"QA-Switch-B".to_string()));
    });
}

#[gpui_kit::gpui::test]
fn sessions_more_menu_opens_group_manager_without_an_extra_click(cx: &mut TestAppContext) {
    let (shell, _directory, cx) = fixture(cx);
    cx.update(|window, cx| shell.update(cx, |shell, cx| shell.open_page(PageId::Sessions, window, cx)));
    draw(cx);
    let bounds = cx.update(|window, _| window.find("list-more-trigger").bounds());
    cx.simulate_click(bounds.center(), Default::default());
    draw(cx);
    cx.simulate_keystrokes("down enter");
    draw(cx);
    assert!(shell.read_with(cx, |shell, _| matches!(shell.overlay, Overlay::Groups(_))),
        "the visible menu selection must open its manager without unrelated input");
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
}

#[gpui_kit::gpui::test]
fn group_dialog_cancels_an_underlying_sidebar_resize(cx: &mut TestAppContext) {
    let (shell, _directory, cx) = fixture(cx);
    cx.update(|window, cx| shell.update(cx, |shell, cx| shell.open_page(PageId::Sessions, window, cx)));
    draw(cx);
    let bounds = cx.debug_bounds("sessions-sidebar").unwrap();
    let start = gpui_kit::point(bounds.right() - px(1.0), bounds.top() + px(80.0));
    let end = gpui_kit::point(start.x + px(100.0), start.y);
    let before = shell.read_with(cx, |shell, _| shell.state.store.borrow().sessions_sidebar_width());
    cx.simulate_mouse_down(start, gpui_kit::MouseButton::Left, Default::default());
    draw(cx);
    open_groups(&shell, cx);
    cx.simulate_mouse_move(end, gpui_kit::MouseButton::Left, Default::default());
    cx.simulate_mouse_up(end, gpui_kit::MouseButton::Left, Default::default());
    draw(cx);
    assert_eq!(shell.read_with(cx, |shell, _| shell.state.store.borrow().sessions_sidebar_width()), before);
}

#[gpui_kit::gpui::test]
fn inline_rename_enter_saves_the_row_without_closing_the_group_manager(cx: &mut TestAppContext) {
    let (shell, _directory, cx) = fixture(cx);
    let manager = open_groups(&shell, cx);
    focus_input_before("group-add", "", cx);
    cx.simulate_input("QA-Enter-A");
    draw(cx);
    click("group-add", cx);
    click("group-edit-QA-Enter-A", cx);
    focus_input_before("group-save-QA-Enter-A", "QA-Enter-A", cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    cx.simulate_input("QA-Enter-B");
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(
        cx.update(|window, cx| window.has_active_dialog(cx)),
        "Enter in the inline rename must not confirm the outer dialog"
    );
    shell.read_with(cx, |shell, _| {
        assert!(matches!(&shell.overlay, Overlay::Groups(view) if view.entity_id() == manager));
        assert_eq!(
            shell.state.store.borrow().groups(),
            &["QA-Enter-B".to_string()]
        );
    });
    // Each new inline input must get the same handler, including a no-op rename.
    click("group-edit-QA-Enter-B", cx);
    focus_input_before("group-save-QA-Enter-B", "QA-Enter-B", cx);
    replace_focused_draft("QA-Enter-C", cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_same_group_dialog(&shell, manager, cx);
    click("group-edit-QA-Enter-C", cx);
    focus_input_before("group-save-QA-Enter-C", "QA-Enter-C", cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_same_group_dialog(&shell, manager, cx);
    assert_eq!(
        group_cache(&shell, cx)["groups"],
        serde_json::json!(["QA-Enter-C"])
    );
    click("close", cx);
    assert_closed(&shell, cx);
}

fn seed_rename_groups(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    shell.read_with(cx, |shell, _| {
        let mut store = shell.state.store.borrow_mut();
        store.add_group("QA-Enter-A".into());
        store.add_group("QA-Existing".into());
        for (id, group) in [
            ("rename-session", "QA-Enter-A"),
            ("other-session", "QA-Existing"),
        ] {
            let mut session = crate::config::Session::new_empty();
            session.id = id.into();
            session.name = id.into();
            session.group = group.into();
            // Only synthetic records: no connection, secret, or keyring access.
            store.cache.sessions.push(session);
        }
        store.cache.collapsed_session_groups = Some(vec!["QA-Enter-A".into(), "system".into()]);
        store.save().unwrap();
    });
}

fn begin_enter_rename(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> gpui_kit::EntityId {
    let manager = open_groups(shell, cx);
    focus_input_before("group-add", "", cx);
    cx.simulate_input("QA-Preserved-Draft");
    draw(cx);
    click("group-edit-QA-Enter-A", cx);
    focus_input_before("group-save-QA-Enter-A", "QA-Enter-A", cx);
    manager
}

fn replace_focused_draft(value: &str, cx: &mut VisualTestContext) {
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a backspace"
    } else {
        "ctrl-a backspace"
    });
    cx.simulate_input(value);
    draw(cx);
}

fn group_cache(shell: &Entity<Shell>, cx: &VisualTestContext) -> serde_json::Value {
    shell.read_with(cx, |shell, _| {
        serde_json::to_value(&shell.state.store.borrow().cache).unwrap()
    })
}

fn group_disk(directory: &tempfile::TempDir) -> serde_json::Value {
    let connection =
        rusqlite::Connection::open(directory.path().join("synthetic-groups.db")).unwrap();
    let settings: String = connection
        .query_row("SELECT value FROM meta WHERE key = 'settings'", [], |row| {
            row.get(0)
        })
        .unwrap();
    let mut result: serde_json::Value = serde_json::from_str(&settings).unwrap();
    let mut statement = connection
        .prepare("SELECT data FROM sessions ORDER BY ordinal, id")
        .unwrap();
    result["sessions"] = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str::<serde_json::Value>(&row.unwrap()).unwrap())
        .collect::<Vec<_>>()
        .into();
    result
}

fn focused_group_input(
    cx: &mut VisualTestContext,
) -> Entity<gpui_kit::component::input::InputState> {
    cx.update(|window, cx| {
        window
            .focused_input(cx)
            .expect("focused group input")
            .as_input()
            .expect("single-line input")
            .clone()
    })
}

fn assert_same_group_dialog(
    shell: &Entity<Shell>,
    manager: gpui_kit::EntityId,
    cx: &mut VisualTestContext,
) {
    assert!(
        cx.update(|window, cx| window.has_active_dialog(cx)),
        "the group manager must remain open"
    );
    assert!(shell.read_with(
        cx,
        |shell, _| matches!(&shell.overlay, Overlay::Groups(view) if view.entity_id() == manager)
    ));
}

#[gpui_kit::gpui::test]
fn inline_rename_enter_rejects_blank_duplicate_and_reserved_names_without_losing_the_draft(
    cx: &mut TestAppContext,
) {
    let (shell, directory, cx) = fixture(cx);
    seed_rename_groups(&shell, cx);
    let manager = begin_enter_rename(&shell, cx);
    let input = focused_group_input(cx);
    let before = group_cache(&shell, cx);
    let saved = group_disk(&directory);
    for draft in [
        "",
        "   ",
        "QA-Existing",
        " qa-existing ",
        "system",
        "DEFAULT",
    ] {
        replace_focused_draft(draft, cx);
        assert!(
            cx.debug_bounds("group-rename-error").is_none(),
            "editing clears a previous validation error"
        );
        for _ in 0..2 {
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert_same_group_dialog(&shell, manager, cx);
            assert_eq!(focused_group_input(cx).entity_id(), input.entity_id());
            assert_eq!(
                input.read_with(cx, |input, _| input.value().to_string()),
                draft
            );
            assert!(
                cx.debug_bounds("group-rename-error").is_some(),
                "invalid input needs visible feedback"
            );
            assert_eq!(group_cache(&shell, cx), before);
            assert_eq!(group_disk(&directory), saved);
        }
    }
    // A case-only self-rename is permitted by ConfigStore, unlike a collision.
    replace_focused_draft("qa-enter-a", cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_same_group_dialog(&shell, manager, cx);
    assert_eq!(
        group_cache(&shell, cx)["sessions"][0]["group"],
        "qa-enter-a"
    );
    assert_eq!(group_disk(&directory)["sessions"][0]["group"], "qa-enter-a");
    assert_eq!(
        focused_group_input(cx).read_with(cx, |input, _| input.value().to_string()),
        "QA-Preserved-Draft"
    );
    cx.simulate_keystrokes("escape");
    assert_closed(&shell, cx);
}

#[gpui_kit::gpui::test]
fn inline_rename_enter_save_failure_restores_all_group_links_and_can_retry(
    cx: &mut TestAppContext,
) {
    let (shell, directory, cx) = fixture(cx);
    seed_rename_groups(&shell, cx);
    let manager = begin_enter_rename(&shell, cx);
    let input = focused_group_input(cx);
    replace_focused_draft("QA-Enter-B", cx);
    let saved = group_disk(&directory);
    let blocker = directory.path().join("not-a-directory");
    std::fs::write(&blocker, "synthetic save failure").unwrap();
    let path = shell.read_with(cx, |shell, _| {
        let mut store = shell.state.store.borrow_mut();
        // An earlier, unrelated in-memory change must survive the rollback.
        store.cache.language = "en".into();
        std::mem::replace(&mut store.path, blocker.join("sessions.db"))
    });
    let before = group_cache(&shell, cx);
    for keyboard in [true, false] {
        if keyboard {
            cx.simulate_keystrokes("enter");
        } else {
            click("group-save-QA-Enter-A", cx);
        }
        draw(cx);
        assert_same_group_dialog(&shell, manager, cx);
        assert_eq!(focused_group_input(cx).entity_id(), input.entity_id());
        assert_eq!(
            input.read_with(cx, |input, _| input.value().to_string()),
            "QA-Enter-B"
        );
        assert!(cx.debug_bounds("group-rename-error").is_some());
        assert_eq!(group_cache(&shell, cx), before);
        assert_eq!(group_disk(&directory), saved);
        assert!(shell.read_with(cx, |shell, _| shell
            .state
            .store
            .borrow()
            .persistence_error()
            .is_some()));
    }
    shell.read_with(cx, |shell, _| shell.state.store.borrow_mut().path = path);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_same_group_dialog(&shell, manager, cx);
    let after = group_cache(&shell, cx);
    assert_eq!(
        after["groups"],
        serde_json::json!(["QA-Enter-B", "QA-Existing"])
    );
    assert_eq!(after["sessions"][0]["group"], "QA-Enter-B");
    assert_eq!(after["sessions"][1]["group"], "QA-Existing");
    assert_eq!(
        after["collapsed_session_groups"],
        serde_json::json!(["QA-Enter-B", "system"])
    );
    assert_eq!(after["language"], "en");
    let persisted = group_disk(&directory);
    for key in ["groups", "sessions", "collapsed_session_groups", "language"] {
        assert_eq!(
            persisted[key], after[key],
            "successful retry persists {key}"
        );
    }
    assert!(shell.read_with(cx, |shell, _| shell
        .state
        .store
        .borrow()
        .persistence_error()
        .is_none()));
    click("close", cx);
    assert_closed(&shell, cx);
}

#[gpui_kit::gpui::test]
fn inline_rename_enter_during_chinese_composition_waits_for_a_separate_submit(
    cx: &mut TestAppContext,
) {
    let (shell, directory, cx) = fixture(cx);
    seed_rename_groups(&shell, cx);
    let manager = begin_enter_rename(&shell, cx);
    let input = focused_group_input(cx);
    let before = group_cache(&shell, cx);
    let saved = group_disk(&directory);
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(Some(0..10), "中文分组", Some(4..4), window, cx);
            assert_eq!(input.marked_text_range(window, cx), Some(0..4));
        })
    });
    draw(cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_same_group_dialog(&shell, manager, cx);
    assert_eq!(focused_group_input(cx).entity_id(), input.entity_id());
    assert_eq!(group_cache(&shell, cx), before);
    assert_eq!(group_disk(&directory), saved);
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            assert_eq!(input.marked_text_range(window, cx), Some(0..4));
            // This is the native IME commit path, distinct from form submission.
            input.replace_text_in_range(None, "中文分组", window, cx);
            assert_eq!(input.marked_text_range(window, cx), None);
        })
    });
    draw(cx);
    assert_eq!(
        group_cache(&shell, cx),
        before,
        "committing the candidate alone must not save"
    );
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_same_group_dialog(&shell, manager, cx);
    assert_eq!(group_cache(&shell, cx)["sessions"][0]["group"], "中文分组");
    assert_eq!(group_disk(&directory)["sessions"][0]["group"], "中文分组");
    click("close", cx);
    assert_closed(&shell, cx);
}

#[gpui_kit::gpui::test]
fn inline_rename_enter_does_not_submit_a_row_when_the_creation_input_is_focused(
    cx: &mut TestAppContext,
) {
    let (shell, directory, cx) = fixture(cx);
    seed_rename_groups(&shell, cx);
    begin_enter_rename(&shell, cx);
    replace_focused_draft("QA-Not-Submitted", cx);
    let before = group_cache(&shell, cx);
    let saved = group_disk(&directory);
    focus_input_before("group-add", "QA-Preserved-Draft", cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(group_cache(&shell, cx), before);
    assert_eq!(group_disk(&directory), saved);
}

#[gpui_kit::gpui::test]
fn inline_rename_enter_scope_preserves_escape_and_outer_close_for_unsaved_edits(
    cx: &mut TestAppContext,
) {
    let (shell, directory, cx) = fixture(cx);
    seed_rename_groups(&shell, cx);
    let before = group_cache(&shell, cx);
    let saved = group_disk(&directory);
    for pointer in [false, true] {
        begin_enter_rename(&shell, cx);
        replace_focused_draft("QA-Not-Submitted", cx);
        if pointer {
            click("close", cx);
        } else {
            cx.simulate_keystrokes("escape");
        }
        assert_closed(&shell, cx);
        assert_eq!(group_cache(&shell, cx), before);
        assert_eq!(group_disk(&directory), saved);
    }
}

#[gpui_kit::gpui::test]
fn group_order_manager_buttons_share_sidebar_order_and_keep_reserved_identity(cx: &mut TestAppContext) {
    let (shell, _directory, cx) = fixture(cx);
    shell.update(cx, |shell, _| {
        let mut store = shell.state.store.borrow_mut();
        store.add_group("QA-Order-A".into());
        store.add_group("QA-Order-B".into());
        store.save().unwrap();
    });
    open_groups(&shell, cx);
    click("group-up-QA-Order-B", cx);
    let store = shell.read_with(cx, |shell, _| shell.state.store.clone());
    assert_eq!(store.borrow().ordered_session_groups(), vec!["system", "QA-Order-B", "QA-Order-A"]);
    click("group-up-QA-Order-B", cx);
    assert_eq!(store.borrow().ordered_session_groups(), vec!["QA-Order-B", "system", "QA-Order-A"]);
    click("group-up-QA-Order-B", cx); // Disabled at the boundary.
    assert_eq!(store.borrow().ordered_session_groups()[0], "QA-Order-B");
    click("group-delete-system", cx);
    click("group-edit-system", cx);
    assert_eq!(store.borrow().ordered_session_groups(), vec!["QA-Order-B", "system", "QA-Order-A"]);
    let rows = crate::ui::session_list::rows_for(&store, "");
    let headers: Vec<_> = rows.iter().filter(|row| !row.group_header.is_empty()).map(|row| row.group_header.as_str()).collect();
    assert_eq!(headers, vec!["QA-Order-B", "system", "QA-Order-A"]);
    click("group-down-QA-Order-B", cx);
    assert_eq!(store.borrow().ordered_session_groups(), vec!["system", "QA-Order-B", "QA-Order-A"]);
}

#[gpui_kit::gpui::test]
fn group_order_default_heading_moves_but_cannot_be_renamed_or_deleted(cx: &mut TestAppContext) {
    let (shell, _directory, cx) = fixture(cx);
    let store = shell.read_with(cx, |shell, _| shell.state.store.clone());
    let mut session = crate::config::Session::new_empty();
    session.id = "synthetic-ungrouped-order".into();
    session.name = "Synthetic ungrouped".into();
    session.group.clear();
    store.borrow_mut().cache.sessions.push(session);
    open_groups(&shell, cx);
    click("group-up-default", cx);
    assert_eq!(store.borrow().ordered_session_groups(), vec!["default", "system"]);
    click("group-delete-default", cx);
    click("group-edit-default", cx);
    assert_eq!(store.borrow().ordered_session_groups(), vec!["default", "system"]);
    assert!(store.borrow().sessions()[0].group.is_empty());
    assert_eq!(store.borrow().sessions()[0].id, "synthetic-ungrouped-order");
}
