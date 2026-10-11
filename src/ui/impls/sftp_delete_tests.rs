//! Confirm remote deletion through real dialog events, capturing commands without a transport.

use super::*;
use gpui_kit::gpui::{
    InputEvent as _, KeyDownEvent, KeyUpEvent, Keystroke, MouseButton, MouseDownEvent,
    MouseUpEvent, TestAppContext, VisualTestContext,
};
use gpui_kit::test::TestWindowExt as _;
use std::sync::{Arc, Mutex};

fn fixture(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::ui::actions::init(cx);
    });
    let store = crate::config::ConfigStore {
        path: std::env::temp_dir().join(format!(
            "xenterm-delete-confirm-{}.db",
            uuid::Uuid::new_v4()
        )),
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
        window.activate_window();
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
    (shell.unwrap(), cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn panel_fixture(
    cx: &mut TestAppContext,
) -> (
    Entity<Shell>,
    &mut VisualTestContext,
    tokio::sync::mpsc::UnboundedReceiver<crate::sftp::SftpCommand>,
) {
    let (shell, cx) = fixture(cx);
    let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    shell.update(cx, |shell, cx| {
        let mut profile = crate::config::Session::new_empty();
        profile.id = "blocked-delete-fixture".into();
        profile.name = "Synthetic delete session".into();
        profile.host = "127.0.0.1".into();
        profile.port = 0;
        // Missing explicit jump resolution fails before any transport starts.
        profile.jump_session_ids = vec!["missing-delete-hop".into()];
        shell.state.store.borrow_mut().upsert(profile);
        shell.state.store.borrow_mut().set_sidebar_collapsed(true);
        shell
            .state
            .store
            .borrow_mut()
            .set_sftp_panel_dock("bottom".into());
        shell.pages.terminal.update(cx, |page, cx| {
            page.open_session_tab("delete-fixture", "blocked-delete-fixture", cx);
            page.set_sftp_collapsed(false, cx);
        });
        assert!(!shell.state.handles.borrow().contains_key("delete-fixture"));
        shell.state.sftp_handles.lock().unwrap().insert(
            "delete-fixture".into(),
            crate::sftp::SftpHandle {
                commands,
                join: runtime.spawn(async {}),
            },
        );
        let entries = vec![
            crate::session::protocol::RemoteEntry {
                name: "嵌套 目录".into(),
                full_path: "/synthetic/parent/嵌套 目录".into(),
                is_dir: true,
                size: 0,
                modified: 0,
                mode: 0o755,
            },
            crate::session::protocol::RemoteEntry {
                name: "报告 2026.txt".into(),
                full_path: "/synthetic/parent/报告 2026.txt".into(),
                is_dir: false,
                size: 10,
                modified: 0,
                mode: 0o644,
            },
        ];
        let mut listing = crate::core::SftpListing::default();
        listing.load("/synthetic/parent".into(), &entries);
        shell
            .state
            .sftp_listings
            .lock()
            .unwrap()
            .insert("delete-fixture".into(), listing);
        cx.notify();
    });
    for _ in 0..3 {
        draw(cx);
    }
    (shell, cx, receiver)
}

fn click(id: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx.update(|window, _| window.find(id).bounds());
    cx.simulate_click(bounds.center(), Default::default());
    for _ in 0..3 {
        draw(cx);
    }
}

fn take_deletes(
    commands: &mut tokio::sync::mpsc::UnboundedReceiver<crate::sftp::SftpCommand>,
) -> Vec<String> {
    let mut deleted = Vec::new();
    while let Ok(command) = commands.try_recv() {
        if let crate::sftp::SftpCommand::Delete(path) = command {
            deleted.push(path);
        }
    }
    deleted
}

fn open_delete(
    cx: &mut VisualTestContext,
    commands: &mut tokio::sync::mpsc::UnboundedReceiver<crate::sftp::SftpCommand>,
) {
    click("sftp-tick-0", cx);
    click("sftp-tick-1", cx);
    click("sftp-delete-selected", cx);
    assert!(
        take_deletes(commands).is_empty(),
        "Trash must not send Delete before explicit confirmation"
    );
    assert!(
        cx.update(|window, cx| window.has_active_dialog(cx)),
        "Trash must open a confirmation"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut previous = None;
    let mut stable = 0;
    loop {
        draw(cx);
        let bounds = cx.update(|window, _| window.find("sftp-delete-confirm").bounds());
        if previous == Some(bounds) {
            stable += 1;
        } else {
            stable = 0;
        }
        if stable >= 3 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "delete confirmation must settle"
        );
        previous = Some(bounds);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[gpui_kit::gpui::test]
fn trash_cancel_escape_close_and_initial_enter_never_delete(cx: &mut TestAppContext) {
    for action in ["cancel", "escape", "close", "enter"] {
        let (_shell, cx, mut commands) = panel_fixture(cx);
        open_delete(cx, &mut commands);
        assert_eq!(
            cx.update(|window, _| window.find("sftp-delete-cancel").focused()),
            Some(true),
            "Cancel is initially focused"
        );
        match action {
            "cancel" => click("sftp-delete-cancel", cx),
            "close" => {
                let bounds = cx.update(|window, _| window.within("dialog").find("close").bounds());
                cx.simulate_click(bounds.center(), Default::default());
                for _ in 0..3 {
                    draw(cx);
                }
            }
            key => {
                cx.simulate_keystrokes(key);
                for _ in 0..3 {
                    draw(cx);
                }
            }
        }
        assert!(
            !cx.update(|window, cx| window.has_active_dialog(cx)),
            "{action} closes confirmation"
        );
        assert!(
            take_deletes(&mut commands).is_empty(),
            "{action} must not delete"
        );
    }
}

#[gpui_kit::gpui::test]
fn trash_confirmation_names_exact_targets_and_sends_each_once(cx: &mut TestAppContext) {
    let (_shell, cx, mut commands) = panel_fixture(cx);
    open_delete(cx, &mut commands);
    let summary = cx.update(|window, _| {
        window
            .find("sftp-delete-summary")
            .label()
            .unwrap()
            .to_string()
    });
    for expected in [
        "2",
        "Synthetic delete session",
        "/synthetic/parent",
        "嵌套 目录",
        "报告 2026.txt",
    ] {
        assert!(
            summary.contains(expected),
            "confirmation must name {expected}"
        );
    }
    let position = cx.update(|window, _| window.find("sftp-delete-confirm").bounds().center());
    // Dispatch two physical clicks before a new frame can remove the button.
    cx.update(|window, cx| {
        for _ in 0..2 {
            window.dispatch_event(
                MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Default::default(),
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseUpEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
        }
    });
    for _ in 0..3 {
        draw(cx);
    }
    assert_eq!(
        take_deletes(&mut commands),
        [
            "/synthetic/parent/嵌套 目录",
            "/synthetic/parent/报告 2026.txt"
        ]
    );
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    draw(cx);
    assert!(
        take_deletes(&mut commands).is_empty(),
        "confirmation must not repeat"
    );
}

#[gpui_kit::gpui::test]
fn trash_confirmation_aborts_after_selection_listing_navigation_or_tab_changes(
    cx: &mut TestAppContext,
) {
    for change in ["selection", "listing", "navigation", "tab"] {
        let (shell, cx, mut commands) = panel_fixture(cx);
        open_delete(cx, &mut commands);
        shell.update(cx, |shell, cx| {
            match change {
                "selection" => {
                    shell
                        .state
                        .sftp_listings
                        .lock()
                        .unwrap()
                        .get_mut("delete-fixture")
                        .unwrap()
                        .toggle_selected(0);
                    shell
                        .pages
                        .terminal
                        .update(cx, |page, cx| page.refresh_dock(cx));
                }
                "listing" => {
                    shell
                        .state
                        .sftp_listings
                        .lock()
                        .unwrap()
                        .get_mut("delete-fixture")
                        .unwrap()
                        .touch();
                }
                "navigation" => {
                    let panel = shell.pages.terminal.read(cx).dock_panel().clone();
                    panel.update(cx, |panel, cx| {
                        panel.set_path("/synthetic/other".into(), cx)
                    });
                }
                "tab" => {
                    shell
                        .pages
                        .terminal
                        .update(cx, |page, cx| page.set_active_tab(None, cx));
                }
                _ => unreachable!(),
            }
            cx.notify();
        });
        draw(cx);
        click("sftp-delete-confirm", cx);
        assert!(
            take_deletes(&mut commands).is_empty(),
            "{change} invalidates the destructive request"
        );
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }
}

#[gpui_kit::gpui::test]
fn trash_confirmation_cannot_cross_into_a_reconnected_transport(cx: &mut TestAppContext) {
    let (shell, cx, mut old_commands) = panel_fixture(cx);
    open_delete(cx, &mut old_commands);
    let (replacement, mut new_commands) = tokio::sync::mpsc::unbounded_channel();
    shell.update(cx, |shell, _| {
        shell
            .state
            .sftp_handles
            .lock()
            .unwrap()
            .get_mut("delete-fixture")
            .unwrap()
            .commands = replacement;
    });
    click("sftp-delete-confirm", cx);
    assert!(take_deletes(&mut old_commands).is_empty());
    assert!(take_deletes(&mut new_commands).is_empty());
}

#[gpui_kit::gpui::test]
fn trash_explicit_keyboard_activation_requires_focusing_delete(cx: &mut TestAppContext) {
    for key in ["enter", "space"] {
        let (_shell, cx, mut commands) = panel_fixture(cx);
        open_delete(cx, &mut commands);
        cx.simulate_keystrokes("tab");
        draw(cx);
        assert_eq!(
            cx.update(|window, _| window.find("sftp-delete-confirm").focused()),
            Some(true)
        );
        let keystroke = Keystroke::parse(key).unwrap();
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke });
        for _ in 0..3 {
            draw(cx);
        }
        assert_eq!(
            take_deletes(&mut commands),
            [
                "/synthetic/parent/嵌套 目录",
                "/synthetic/parent/报告 2026.txt"
            ],
            "explicit {key} on Delete must activate its own button"
        );
    }
}
