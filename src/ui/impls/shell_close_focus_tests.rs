//! Exercise the actual shell CloseTab key binding with synthetic session channels.
//! No terminal process, network connection, credentials, or user file is opened.

use super::*;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use std::sync::{Arc, Mutex};

fn fixture(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
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
