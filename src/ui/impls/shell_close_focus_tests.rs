//! Exercise the actual shell CloseTab key binding with synthetic session channels.
//! No terminal process, network connection, credentials, or user file is opened.

use super::*;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use std::sync::{Arc, Mutex};

fn fixture(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    // The nav rail has real keyboard focus targets under the Shell key context.
    // Focus one without invoking a click that could open another dialog.
    cx.update(|window, cx| window.focus_next(cx));
    draw(cx);
    (shell, cx)
}

/// The actual empty-window construction path, without a test-only focus assignment.
fn fixture_without_forced_focus(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::ui::actions::init(cx);
    });
    let store = crate::config::ConfigStore {
        path: std::env::temp_dir().join(format!("xenterm-close-focus-{}.db", uuid::Uuid::new_v4())),
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

#[gpui_kit::gpui::test]
fn close_split_keyboard_action_keeps_input_in_the_surviving_terminal(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let mut inboxes = Vec::new();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    shell.update(cx, |shell, cx| {
        let mut profile = crate::config::Session::new_empty();
        profile.id = "blocked-close-fixture".into();
        profile.host = "127.0.0.1".into();
        profile.port = 0;
        // Invalid explicit jump resolution fails before starting a transport.
        profile.jump_session_ids = vec!["missing-fixture-hop".into()];
        shell.state.store.borrow_mut().upsert(profile);
        shell.state.store.borrow_mut().set_sidebar_collapsed(true);
        let state = shell.state.clone();
        shell.pages.terminal.update(cx, |page, cx| {
            for id in ["left-fixture", "right-fixture"] {
                page.open_session_tab(id, "blocked-close-fixture", cx);
                assert!(!state.handles.borrow().contains_key(id));
                let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
                state.handles.borrow_mut().insert(
                    id.into(),
                    crate::session::protocol::SessionHandle {
                        tab_id: id.into(),
                        commands,
                        join: runtime.spawn(std::future::pending::<()>()),
                    },
                );
                inboxes.push(receiver);
            }
            page.set_active_tab(Some("left-fixture".into()), cx);
        });
    });
    for _ in 0..3 {
        draw(cx);
    }
    let area = cx.debug_bounds("terminal-pane-area").unwrap();
    cx.simulate_click(area.center(), Default::default());
    draw(cx);
    cx.simulate_keystrokes("ctrl-shift-e");
    for _ in 0..3 {
        draw(cx);
    }
    let area = cx.debug_bounds("terminal-pane-area").unwrap();
    cx.simulate_click(
        gpui_kit::point(area.right() - area.size.width / 4., area.center().y),
        Default::default(),
    );
    draw(cx);
    cx.simulate_input("before-close");
    assert_eq!(raw_input(&mut inboxes[1]), b"before-close");
    cx.simulate_keystrokes("ctrl-shift-w");
    for _ in 0..3 {
        draw(cx);
    }
    shell.read_with(cx, |shell, cx| {
        assert_eq!(
            shell.pages.terminal.read(cx).active_tab_id().as_deref(),
            Some("left-fixture")
        );
        assert!(!shell.pages.terminal.read(cx).has_tab("right-fixture"));
    });
    // Deliberately no click, focus call, or selection after Ctrl+Shift+W.
    cx.simulate_input("echo shell-close-keeps-focus");
    cx.simulate_keystrokes("enter");
    let input = raw_input(&mut inboxes[0]);
    assert!(
        String::from_utf8_lossy(&input).contains("echo shell-close-keeps-focus"),
        "real Shell CloseTab action must preserve keyboard input; got {input:?}"
    );
    assert!(raw_input(&mut inboxes[1]).is_empty());
}

fn raw_input(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<crate::session::protocol::SessionCommand>,
) -> Vec<u8> {
    let mut input = Vec::new();
    while let Ok(command) = receiver.try_recv() {
        if let crate::session::protocol::SessionCommand::RawInput(bytes) = command {
            input.extend(bytes);
        }
    }
    input
}

// These fixtures use the same Root, Shell, keymap and tab buttons as the app.
// Invalid jump resolution stops before transport creation; only a synthetic
// command receiver observes what the focused TerminalView actually sends.
fn add_entry_profile(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) {
    shell.update(cx, |shell, _| {
        let mut profile = crate::config::Session::new_empty();
        profile.id = id.into();
        profile.name = id.into();
        profile.host = "127.0.0.1".into();
        profile.port = 0;
        profile.jump_session_ids = vec!["missing-fixture-hop".into()];
        shell.state.store.borrow_mut().upsert(profile);
        shell.state.store.borrow_mut().set_sidebar_collapsed(true);
    });
}

fn entry_inbox(
    shell: &Entity<Shell>,
    id: &str,
    runtime: &tokio::runtime::Runtime,
    cx: &mut VisualTestContext,
) -> tokio::sync::mpsc::UnboundedReceiver<SessionCommand> {
    let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
    shell.update(cx, |shell, _| {
        assert!(!shell.state.handles.borrow().contains_key(id));
        shell.state.handles.borrow_mut().insert(
            id.into(),
            crate::session::protocol::SessionHandle {
                tab_id: id.into(),
                commands,
                join: runtime.spawn(std::future::pending::<()>()),
            },
        );
    });
    receiver
}

fn open_entry_tab(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) {
    add_entry_profile(shell, id, cx);
    shell.update(cx, |shell, cx| {
        shell
            .pages
            .terminal
            .update(cx, |page, cx| page.open_session_tab(id, id, cx));
    });
    draw(cx);
}

fn click_entry_element(id: &str, cx: &mut VisualTestContext) {
    use gpui_kit::test::TestWindowExt as _;
    let bounds = cx.update(|window, _| window.find(SharedString::from(id.to_owned())).bounds());
    cx.simulate_click(bounds.center(), Default::default());
    draw(cx);
}

fn assert_entry_input(
    inbox: &mut tokio::sync::mpsc::UnboundedReceiver<SessionCommand>,
    cx: &mut VisualTestContext,
) {
    // No focus assignment or terminal-body click after the navigation action.
    cx.simulate_input("echo entry-focus");
    assert_eq!(raw_input(inbox), b"echo entry-focus");
}

#[gpui_kit::gpui::test]
fn entry_quick_connect_return_focuses_new_terminal_without_mouse(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    add_entry_profile(&shell, "entry-new-terminal", cx);
    cx.simulate_keystrokes("ctrl-k");
    draw(cx);
    cx.simulate_input("entry-new-terminal");
    draw(cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.overlay, Overlay::None);
        assert_eq!(shell.pages.active, PageId::Terminal);
        assert_eq!(
            shell.pages.terminal.read(cx).active_tab_id().as_deref(),
            Some("entry-new-terminal")
        );
    });
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    let mut inbox = entry_inbox(&shell, "entry-new-terminal", &runtime, cx);
    draw(cx);
    assert_entry_input(&mut inbox, cx);
}

#[gpui_kit::gpui::test]
fn entry_quick_connect_return_reuses_and_focuses_existing_terminal(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    open_entry_tab(&shell, "entry-existing-terminal", cx);
    let mut inbox = entry_inbox(&shell, "entry-existing-terminal", &runtime, cx);
    cx.update(|window, cx| window.focus_next(cx));
    cx.simulate_keystrokes("ctrl-k");
    draw(cx);
    cx.simulate_input("entry-existing-terminal");
    draw(cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.pages.terminal.read(cx).tabs.len(), 1);
        assert!(!shell.state.handles.borrow()["entry-existing-terminal"]
            .join
            .is_finished());
    });
    assert_entry_input(&mut inbox, cx);
}

#[gpui_kit::gpui::test]
fn entry_tab_chip_click_focuses_selected_terminal_without_body_click(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    open_entry_tab(&shell, "entry-left", cx);
    let mut left = entry_inbox(&shell, "entry-left", &runtime, cx);
    open_entry_tab(&shell, "entry-right", cx);
    let mut right = entry_inbox(&shell, "entry-right", &runtime, cx);
    // Plain divs are not recorded by the toolkit observer. The observed X
    // button anchors a point in this same chip's title, outside the button.
    use gpui_kit::test::TestWindowExt as _;
    let close = cx.update(|window, _| window.find("close-entry-left").bounds());
    cx.simulate_click(
        gpui_kit::point(close.left() - px(20.), close.center().y),
        Default::default(),
    );
    draw(cx);
    assert_entry_input(&mut left, cx);
    assert!(raw_input(&mut right).is_empty());
}

#[gpui_kit::gpui::test]
fn entry_tab_x_does_not_reactivate_closed_tab_and_focuses_survivor(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    open_entry_tab(&shell, "entry-left", cx);
    let mut left = entry_inbox(&shell, "entry-left", &runtime, cx);
    open_entry_tab(&shell, "entry-right", cx);
    let mut right = entry_inbox(&shell, "entry-right", &runtime, cx);
    click_entry_element("close-entry-right", cx);
    shell.read_with(cx, |shell, cx| {
        let page = shell.pages.terminal.read(cx);
        assert!(!page.has_tab("entry-right"));
        assert_eq!(page.active_tab_id().as_deref(), Some("entry-left"));
        assert!(!shell.state.handles.borrow().contains_key("entry-right"));
    });
    assert_entry_input(&mut left, cx);
    assert!(raw_input(&mut right).is_empty());
}

#[gpui_kit::gpui::test]
fn entry_palette_close_tab_return_focuses_survivor(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    open_entry_tab(&shell, "entry-left", cx);
    let mut left = entry_inbox(&shell, "entry-left", &runtime, cx);
    open_entry_tab(&shell, "entry-right", cx);
    let mut right = entry_inbox(&shell, "entry-right", &runtime, cx);
    cx.update(|window, cx| window.focus_next(cx));
    cx.simulate_keystrokes("ctrl-shift-p");
    draw(cx);
    cx.simulate_input(crate::i18n::t("关闭标签", "Close tab"));
    cx.simulate_keystrokes("enter");
    draw(cx);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.overlay, Overlay::None);
        assert_eq!(
            shell.pages.terminal.read(cx).active_tab_id().as_deref(),
            Some("entry-left")
        );
    });
    assert_entry_input(&mut left, cx);
    assert!(raw_input(&mut right).is_empty());
}

#[gpui_kit::gpui::test]
fn entry_quick_connect_cancel_keeps_previous_page_and_focus(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    open_entry_tab(&shell, "entry-cancel", cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_page(PageId::Settings, window, cx)
        });
    });
    draw(cx);
    cx.update(|window, cx| window.focus_next(cx));
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    cx.simulate_keystrokes("ctrl-k");
    draw(cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    cx.simulate_input("entry-cancel");
    draw(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    // Escape uses the toolkit's animated focus restoration.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !cx.update(|window, _| before.is_focused(window)) {
        assert!(
            std::time::Instant::now() < deadline,
            "cancel did not restore previous focus"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        draw(cx);
    }
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.pages.active, PageId::Settings);
        assert_eq!(shell.overlay, Overlay::None);
    });
}

#[gpui_kit::gpui::test]
fn entry_pending_connect_preserves_another_page_focus(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    add_entry_profile(&shell, "entry-background", cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_page(PageId::Settings, window, cx)
        });
    });
    draw(cx);
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    shell.update(cx, |shell, cx| {
        shell.pages.terminal.update(cx, |page, cx| {
            page.request(
                TerminalAction::Connect {
                    tab_id: "entry-background".into(),
                    session_id: "entry-background".into(),
                },
                cx,
            );
        });
    });
    for _ in 0..3 {
        draw(cx);
    }
    assert!(cx.update(|window, _| before.is_focused(window)));
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.pages.active, PageId::Settings);
        assert!(shell.pages.terminal.read(cx).has_tab("entry-background"));
    });
}

#[gpui_kit::gpui::test]
fn entry_pending_connect_does_not_take_modal_focus(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    add_entry_profile(&shell, "entry-under-dialog", cx);
    cx.update(|window, cx| {
        window.open_dialog(cx, |dialog, _, _| dialog.title("Keep this dialog focused"));
        shell.update(cx, |shell, cx| {
            shell.pages.terminal.update(cx, |page, cx| {
                page.request(
                    TerminalAction::Connect {
                        tab_id: "entry-under-dialog".into(),
                        session_id: "entry-under-dialog".into(),
                    },
                    cx,
                );
            });
        });
    });
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    for _ in 0..3 {
        draw(cx);
    }
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(cx.update(|window, _| before.is_focused(window)));
}

#[gpui_kit::gpui::test]
fn entry_closing_inactive_x_preserves_active_terminal(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    open_entry_tab(&shell, "entry-left", cx);
    let mut left = entry_inbox(&shell, "entry-left", &runtime, cx);
    open_entry_tab(&shell, "entry-right", cx);
    let mut right = entry_inbox(&shell, "entry-right", &runtime, cx);
    let area = cx.debug_bounds("terminal-pane-area").unwrap();
    cx.simulate_click(area.center(), Default::default());
    draw(cx);
    click_entry_element("close-entry-left", cx);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(
            shell.pages.terminal.read(cx).active_tab_id().as_deref(),
            Some("entry-right")
        );
    });
    assert_entry_input(&mut right, cx);
    assert!(raw_input(&mut left).is_empty());
}

#[path = "startup_shortcut_tests.rs"]
mod startup_shortcut_tests;
