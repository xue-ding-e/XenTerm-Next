//! Exercise the production window path with GPUI's test platform, not macOS Dock
//! delivery. No profile is loaded/saved and no SSH/local session is started.
use super::*;
use gpui_kit::gpui::{EmptyView, QuitMode, TestAppContext, VisualTestContext};
use std::sync::{Arc, Mutex};

fn dependencies() -> (
    Arc<tokio::runtime::Runtime>,
    Rc<RefCell<crate::config::ConfigStore>>,
) {
    let store = crate::config::ConfigStore {
        path: std::env::temp_dir().join(format!("xenterm-reopen-{}.db", uuid::Uuid::new_v4())),
        backup_dir: None,
        cache: crate::config::ConfigFile::default(),
        key: [7; 32],
        keyring_enabled: false,
        saved_state: Mutex::new(crate::config::SavedState::default()).into(),
    };
    (
        Arc::new(tokio::runtime::Runtime::new().unwrap()),
        Rc::new(RefCell::new(store)),
    )
}

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        super::super::dialogs::init(cx);
        super::super::actions::init(cx);
        // Model macOS close-vs-quit semantics even when this test runs on Linux.
        cx.set_quit_mode(QuitMode::Explicit);
    });
}

fn close(handle: WindowHandle<Root>, cx: &mut TestAppContext) {
    cx.update(|cx| {
        handle
            .update(cx, |_, window, _| window.remove_window())
            .unwrap()
    });
    cx.run_until_parked();
}

/// Observe the real constructor without retaining its Shell across window close.
fn open_observed_main(
    main: &Cell<Option<WindowHandle<Root>>>,
    runtime: &Arc<tokio::runtime::Runtime>,
    store: &Rc<RefCell<crate::config::ConfigStore>>,
    cx: &mut TestAppContext,
) -> WeakEntity<Shell> {
    let observed = Rc::new(RefCell::new(None));
    let capture = observed.clone();
    let subscription = cx.update(|cx| {
        cx.observe_new::<Shell>(move |_, _, cx| {
            *capture.borrow_mut() = Some(cx.entity().downgrade());
        })
    });
    cx.update(|cx| show_main_window(main, runtime, store, cx).unwrap());
    drop(subscription);
    let shell = observed.borrow().clone().expect("production Shell constructed");
    shell
}

fn empty_tool(cx: &mut gpui_kit::App) -> WindowHandle<Root> {
    cx.open_window(WindowOptions::default(), |window, cx| {
        let view = cx.new(|_| EmptyView);
        cx.new(|cx| Root::new(view, window, cx))
    })
    .unwrap()
}

fn open_owned_tools(shell: &WeakEntity<Shell>, cx: &mut TestAppContext) {
    shell
        .update(cx, |shell, cx| {
            shell.open_process_window(cx);
            shell.open_system_info_window(cx);
            // The real history viewer starts disk reads. Test its registered
            // ownership slot with an inert Root, never the user's journal.
            shell.execution_log_window.window = Some(empty_tool(cx));
        })
        .unwrap();
}

#[gpui_kit::gpui::test]
fn closing_main_closes_its_tools_before_reopen(cx: &mut TestAppContext) {
    init(cx);
    let (runtime, store) = dependencies();
    let main = Cell::new(None);
    let shell = open_observed_main(&main, &runtime, &store, cx);
    open_owned_tools(&shell, cx);
    assert_eq!(cx.windows().len(), 4);
    let first = main.get().unwrap();
    close(first, cx);
    assert!(shell.upgrade().is_none());
    assert!(cx.windows().is_empty(), "orphan tools suppress the macOS Dock callback");
    cx.update(|cx| show_main_window(&main, &runtime, &store, cx).unwrap());
    assert_ne!(main.get(), Some(first));
    assert_eq!(cx.windows().len(), 1);
    close(main.get().unwrap(), cx);
}

#[gpui_kit::gpui::test]
fn closing_main_tolerates_stale_tools_and_preserves_unrelated_windows(cx: &mut TestAppContext) {
    init(cx);
    let (runtime, store) = dependencies();
    let main = Cell::new(None);
    let shell = open_observed_main(&main, &runtime, &store, cx);
    open_owned_tools(&shell, cx);
    let already_closed = shell
        .read_with(cx, |shell, _| shell.process_window.window.unwrap())
        .unwrap();
    close(already_closed, cx);
    let unrelated = cx.update(empty_tool);
    close(main.get().unwrap(), cx);
    assert!(shell.upgrade().is_none());
    assert_eq!(cx.windows().len(), 1);
    assert!(cx.update(|cx| unrelated.update(cx, |_, _, _| ()).is_ok()));
    close(unrelated, cx);
}

fn process_confirmation(
    shell: &WeakEntity<Shell>,
    runtime: &Arc<tokio::runtime::Runtime>,
    own_process: bool,
    cx: &mut TestAppContext,
) -> (
    VisualTestContext,
    SessionState,
    tokio::sync::mpsc::UnboundedReceiver<SessionCommand>,
) {
    let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
    let (tool, view, state) = shell
        .update(cx, |shell, cx| {
            shell.state.handles.borrow_mut().insert(
                "lifecycle-fixture".into(),
                crate::session::protocol::SessionHandle {
                    tab_id: "lifecycle-fixture".into(),
                    commands,
                    join: runtime.spawn(std::future::pending::<()>()),
                },
            );
            shell.open_process_window(cx);
            (
                shell.process_window.window.unwrap(),
                shell.process_window.view.clone().unwrap(),
                shell.state.clone(),
            )
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(tool.into(), cx);
    // WindowHandle<Root>::update mutably leases Root. Opening a dialog reads
    // Root again, so enter through the window context just like a UI event.
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.confirm_terminate_for_test(
                crate::resource::ProcRow {
                    tab_id: "lifecycle-fixture".into(),
                    pid: "42".into(),
                    user: if own_process { "fixture" } else { "root" }.into(),
                    cpu: "0".into(),
                    mem: "0".into(),
                    command: "inert-fixture".into(),
                    cpu_frac: 0.,
                    own_process,
                },
                window,
                cx,
            );
        })
        .unwrap();
    });
    visual.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
        assert!(window.has_active_dialog(cx));
    });
    (visual, state, receiver)
}

fn assert_only_close(receiver: &mut tokio::sync::mpsc::UnboundedReceiver<SessionCommand>) {
    assert!(matches!(receiver.try_recv(), Ok(SessionCommand::Close)));
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
}

fn confirm_process_dialog(cx: &mut VisualTestContext) {
    // Match the existing dialog tests: entrance animation uses real Instant,
    // so wait for stable observed geometry rather than a guessed click point.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut previous = None;
    let mut stable_samples = 0;
    loop {
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        let bounds = cx.debug_bounds("dialog-confirm");
        if bounds.is_some() && bounds == previous {
            stable_samples += 1;
            if stable_samples == 3 {
                cx.simulate_click(bounds.unwrap().center(), Default::default());
                return;
            }
        } else {
            stable_samples = 0;
        }
        assert!(std::time::Instant::now() < deadline, "process confirmation did not stabilize");
        previous = bounds;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[gpui_kit::gpui::test]
fn closing_main_cancels_unsubmitted_process_password_without_sending_kill(cx: &mut TestAppContext) {
    init(cx);
    let (runtime, store) = dependencies();
    let main = Cell::new(None);
    let shell = open_observed_main(&main, &runtime, &store, cx);
    // Keep only a weak password-field handle: retaining the dialog's entity in
    // the test would defeat the very lifecycle being checked.
    let password = Rc::new(RefCell::new(None));
    let capture = password.clone();
    let subscription = cx.update(|cx| {
        cx.observe_new::<gpui_kit::component::input::InputState>(move |_, _, cx| {
            *capture.borrow_mut() = Some(cx.entity().downgrade());
        })
    });
    let (mut visual, state, mut receiver) = process_confirmation(&shell, &runtime, false, cx);
    drop(subscription);
    let password = password.borrow().clone().expect("sudo password input created");
    visual.update(|window, cx| {
        password
            .update(cx, |input, cx| input.set_value("fixture-only-not-a-secret", window, cx))
            .unwrap();
    });
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    close(main.get().unwrap(), cx);
    assert!(cx.windows().is_empty());
    assert!(password.upgrade().is_none());
    assert!(state.handles.borrow().is_empty());
    assert_only_close(&mut receiver);
    cx.update(|cx| show_main_window(&main, &runtime, &store, cx).unwrap());
    assert_eq!(cx.windows().len(), 1);
    close(main.get().unwrap(), cx);
}

#[gpui_kit::gpui::test]
fn reopening_does_not_replay_an_already_submitted_process_kill(cx: &mut TestAppContext) {
    init(cx);
    let (runtime, store) = dependencies();
    let main = Cell::new(None);
    let shell = open_observed_main(&main, &runtime, &store, cx);
    let (mut visual, state, mut receiver) = process_confirmation(&shell, &runtime, true, cx);
    confirm_process_dialog(&mut visual);
    visual.run_until_parked();
    assert!(matches!(
        receiver.try_recv(),
        Ok(SessionCommand::KillProcess { pid: 42, root_password: None, .. })
    ));
    close(main.get().unwrap(), cx);
    assert!(state.handles.borrow().is_empty());
    cx.update(|cx| show_main_window(&main, &runtime, &store, cx).unwrap());
    assert_only_close(&mut receiver);
    close(main.get().unwrap(), cx);
}

#[gpui_kit::gpui::test]
fn repeated_reopen_activates_the_existing_main_window(cx: &mut TestAppContext) {
    init(cx);
    let (runtime, store) = dependencies();
    let main = Cell::new(None);
    cx.update(|cx| show_main_window(&main, &runtime, &store, cx).unwrap());
    let first = main.get().unwrap();
    for _ in 0..3 {
        cx.update(|cx| show_main_window(&main, &runtime, &store, cx).unwrap());
        assert_eq!(main.get(), Some(first));
        assert_eq!(cx.windows().len(), 1);
    }
    close(first, cx);
}

#[gpui_kit::gpui::test]
fn close_then_reopen_replaces_stale_handle_without_reloading_profile(cx: &mut TestAppContext) {
    init(cx);
    let (runtime, store) = dependencies();
    let main = Cell::new(None);
    cx.update(|cx| show_main_window(&main, &runtime, &store, cx).unwrap());
    let first = main.get().unwrap();
    // An in-memory edit must survive a window being destroyed/rebuilt. The file
    // deliberately does not exist, so loading it again would lose this edit.
    store.borrow_mut().set_sidebar_collapsed(true);
    close(first, cx);
    assert!(cx.windows().is_empty());
    cx.update(|cx| show_main_window(&main, &runtime, &store, cx).unwrap());
    assert_ne!(main.get(), Some(first));
    assert_eq!(cx.windows().len(), 1);
    assert_eq!(store.borrow().sidebar_collapsed(), Some(true));
    assert!(!store.borrow().path.exists());
    close(main.get().unwrap(), cx);
}

#[gpui_kit::gpui::test]
fn close_aborts_queued_auth_before_next_window_can_receive_it(cx: &mut TestAppContext) {
    init(cx);
    let (runtime, store) = dependencies();
    let main = Cell::new(None);
    cx.update(|cx| show_main_window(&main, &runtime, &store, cx).unwrap());
    let (reply, mut response) = tokio::sync::oneshot::channel();
    crate::session::enqueue_mfa(
        WINDOW_ID,
        "closed-window-fixture".into(),
        "fixture.invalid".into(),
        "fixture challenge".into(),
        false,
        crate::session::protocol::MfaResponder::new(reply),
    );
    close(main.get().unwrap(), cx);
    assert_eq!(response.try_recv().unwrap(), None);
    assert!(crate::session::mfa_prompt(WINDOW_ID).is_none());
    cx.update(|cx| show_main_window(&main, &runtime, &store, cx).unwrap());
    assert!(crate::session::mfa_prompt(WINDOW_ID).is_none());
    let (reply, mut new_response) = tokio::sync::oneshot::channel();
    crate::session::enqueue_mfa(
        WINDOW_ID,
        "reopened-window-fixture".into(),
        "fixture.invalid".into(),
        "new fixture challenge".into(),
        false,
        crate::session::protocol::MfaResponder::new(reply),
    );
    cx.run_until_parked();
    assert!(crate::session::mfa_prompt(WINDOW_ID).is_some());
    assert!(matches!(new_response.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
    close(main.get().unwrap(), cx);
    assert_eq!(new_response.try_recv().unwrap(), None);
}

#[gpui_kit::gpui::test]
fn shutdown_does_not_recreate_a_window(cx: &mut TestAppContext) {
    init(cx);
    let (runtime, store) = dependencies();
    let main = Cell::new(None);
    let shell = open_observed_main(&main, &runtime, &store, cx);
    open_owned_tools(&shell, cx);
    assert_eq!(cx.windows().len(), 4);
    // TestPlatform::quit is a no-op. Exercise GPUI shutdown itself rather than
    // claiming this proves Cmd-Q/Dock Quit delivery on native macOS.
    cx.update(|cx| cx.shutdown());
    assert!(cx.windows().is_empty());
    assert!(shell.upgrade().is_none());
}
