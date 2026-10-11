//! Real SFTP row menus dispatch through Shell to the clipboard, without a transport.

use super::*;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use gpui_kit::test::TestWindowExt as _;
use std::sync::{Arc, Mutex};

fn fixture(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::ui::actions::init(cx);
    });
    let store = crate::config::ConfigStore {
        path: std::env::temp_dir().join(format!("xenterm-copy-path-{}.db", uuid::Uuid::new_v4())),
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
        profile.id = "blocked-copy-path-fixture".into();
        profile.host = "127.0.0.1".into();
        profile.port = 0;
        // Missing explicit jump resolution fails before any transport starts.
        profile.jump_session_ids = vec!["missing-copy-path-hop".into()];
        shell.state.store.borrow_mut().upsert(profile);
        shell.state.store.borrow_mut().set_sidebar_collapsed(true);
        shell
            .state
            .store
            .borrow_mut()
            .set_sftp_panel_dock("bottom".into());
        shell.pages.terminal.update(cx, |page, cx| {
            page.open_session_tab("copy-fixture", "blocked-copy-path-fixture", cx);
            page.set_sftp_collapsed(false, cx);
        });
        assert!(!shell.state.handles.borrow().contains_key("copy-fixture"));
        shell.state.sftp_handles.lock().unwrap().insert(
            "copy-fixture".into(),
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
            .insert("copy-fixture".into(), listing);
        cx.notify();
    });
    for _ in 0..3 {
        draw(cx);
    }
    (shell, cx, receiver)
}

fn choose_row_item(cx: &mut VisualTestContext, row: usize, down_steps: usize) {
    let selector: &'static str = Box::leak(format!("sftp-name-{row}").into_boxed_str());
    let bounds = cx
        .debug_bounds(selector)
        .expect("real SFTP row must be visible");
    cx.simulate_mouse_move(bounds.center(), None, Default::default());
    cx.simulate_mouse_down(
        bounds.center(),
        gpui_kit::MouseButton::Right,
        Default::default(),
    );
    cx.simulate_mouse_up(
        bounds.center(),
        gpui_kit::MouseButton::Right,
        Default::default(),
    );
    draw(cx);
    cx.simulate_keystrokes(&format!("{}enter", "down ".repeat(down_steps)));
    draw(cx);
}

#[gpui_kit::gpui::test]
fn directory_and_file_copy_path_menu_events_write_the_exact_clipboard_path(
    cx: &mut TestAppContext,
) {
    let (_shell, cx, mut commands) = panel_fixture(cx);
    while commands.try_recv().is_ok() {}
    for (row, steps, expected) in [
        (0, 2, "/synthetic/parent/嵌套 目录"),
        (1, 6, "/synthetic/parent/报告 2026.txt"),
    ] {
        cx.update(|_, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                "unchanged sentinel".into(),
            ))
        });
        choose_row_item(cx, row, steps);
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
                .as_deref(),
            Some(expected)
        );
        assert!(
            commands.try_recv().is_err(),
            "Copy path must not start navigation, download, or editing"
        );
    }
}

#[gpui_kit::gpui::test]
fn directory_open_and_file_editor_keep_their_existing_menu_positions(cx: &mut TestAppContext) {
    let (_shell, cx, mut commands) = panel_fixture(cx);
    while commands.try_recv().is_ok() {}
    // The directory keeps Open first; Copy path is its only added item.
    choose_row_item(cx, 0, 1);
    assert!(
        matches!(commands.try_recv(), Ok(crate::sftp::SftpCommand::ListDir(path)) if path == "/synthetic/parent/嵌套 目录")
    );
    // File-only actions retain their order and semantics after the shared
    // Copy path item is made reachable for directories.
    choose_row_item(cx, 1, 2);
    assert!(
        matches!(commands.try_recv(), Ok(crate::sftp::SftpCommand::OpenTemp { remote, edit: true }) if remote == "/synthetic/parent/报告 2026.txt")
    );
}
