//! Real group-manager input and toolkit dialog dismissal, with an isolated store.
use super::*;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
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
                approval_queue: Vec::new(),
                approval_opened_at: std::collections::HashMap::new(),
                approval_reported: std::collections::HashSet::new(),
                approval_shown: std::collections::HashSet::new(),
                audit_last_prune_day: None,
                // No real approval queue or background persistence in this fixture.
                approval_poll: None,
                process_window: Detached::new(process_window_title),
                audit_window: crate::ui::AuditWindowHandle::new(),
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
                _root_subscription: None,
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
