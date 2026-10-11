//! Open the real terminal quick popover and activate its own commands.
//! The synthetic invalid jump profile never launches a transport or process.
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

fn terminal_fixture(
    cx: &mut TestAppContext,
) -> (Entity<Shell>, tempfile::TempDir, &mut VisualTestContext) {
    let (shell, directory, cx) = fixture(cx);
    shell.update(cx, |shell, cx| {
        let mut session = crate::config::Session::new_empty();
        session.id = "quick-popover-fixture".into();
        session.name = "Synthetic quick popover fixture".into();
        session.host = "127.0.0.1".into();
        session.port = 0;
        session.jump_session_ids = vec!["missing-quick-fixture-hop".into()];
        shell.state.store.borrow_mut().upsert(session);
        shell.pages.terminal.update(cx, |page, cx| {
            page.open_session_tab("quick-popover-tab", "quick-popover-fixture", cx);
        });
        assert!(shell.state.handles.borrow().is_empty());
        assert!(shell.state.sftp_handles.lock().unwrap().is_empty());
    });
    for _ in 0..3 {
        draw(cx);
    }
    (shell, directory, cx)
}

fn click(id: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx.update(|window, _| window.find(id).bounds());
    cx.simulate_click(bounds.center(), Default::default());
    for _ in 0..3 {
        draw(cx);
    }
}

fn popover_visible(cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| {
        window
            .try_find("quick-manage")
            .is_some_and(|node| node.visible())
    })
}

fn open_quick(cx: &mut VisualTestContext) {
    click("command-quick-trigger", cx);
    assert!(
        popover_visible(cx),
        "bolt must open the actual quick popover"
    );
}

#[gpui_kit::gpui::test]
fn manage_dismisses_the_source_popover_before_opening_its_dialog(cx: &mut TestAppContext) {
    let (shell, _directory, cx) = terminal_fixture(cx);
    open_quick(cx);
    click("quick-manage", cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(shell.read_with(cx, |shell, _| matches!(
        shell.overlay,
        Overlay::QuickManager(_)
    )));
    assert!(
        !popover_visible(cx),
        "the source popover must not cover the manager"
    );
}

#[gpui_kit::gpui::test]
fn the_quick_popover_header_close_dismisses_only_that_popover(cx: &mut TestAppContext) {
    let (shell, _directory, cx) = terminal_fixture(cx);
    open_quick(cx);
    click("quick-close", cx);
    assert!(!popover_visible(cx), "the popover's own X must close it");
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(shell.read_with(cx, |shell, _| matches!(shell.overlay, Overlay::None)));
    open_quick(cx);
    click("quick-close", cx);
    assert!(!popover_visible(cx));
}

#[gpui_kit::gpui::test]
fn managed_popup_can_reopen_and_dialog_input_never_saves_an_unsubmitted_draft(
    cx: &mut TestAppContext,
) {
    let (shell, _directory, cx) = terminal_fixture(cx);
    for _ in 0..2 {
        open_quick(cx);
        click("quick-manage", cx);
        assert!(!popover_visible(cx));
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.update(|window, cx| window.focus_next(cx));
        draw(cx);
        cx.simulate_input("QA-Unsubmitted-Command");
        let typed = cx.update(|window, cx| {
            window
                .focused_input(cx)
                .expect("manager input focused")
                .value(cx)
                .to_string()
        });
        assert_eq!(typed, "QA-Unsubmitted-Command");
        cx.simulate_keystrokes("escape");
        for _ in 0..3 {
            draw(cx);
        }
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(!popover_visible(cx));
        assert!(shell.read_with(cx, |shell, _| shell
            .state
            .store
            .borrow()
            .quick_commands()
            .is_empty()));
    }
}
