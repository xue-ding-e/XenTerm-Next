//! End-to-end built-in SFTP editor routing through the real Shell and event pump.
//!
//! Pointer/menu/keyboard events are real GPUI test events. Only the transport is
//! synthetic: invalid jump resolution prevents any SSH connection, and each
//! tab receives a separate in-memory SFTP command channel. Replies enter that
//! tab's production EventSink with its real connection generation, exactly as
//! the SFTP pump does. No credentials, host, network protocol, or user file is used.

use super::*;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use gpui_kit::test::TestWindowExt as _;
use std::sync::{Arc, Mutex};

const SOURCE: &str = "editor-source";
const OTHER: &str = "editor-other";
const REMOTE: &str = "/synthetic/共享路径/配置.txt";
const ORIGINAL: &str = "source host original\n第二行";
const EDITED: &str = "edited source host\n新内容";

type Inbox = tokio::sync::mpsc::UnboundedReceiver<crate::sftp::SftpCommand>;

struct Fixture {
    shell: Entity<Shell>,
    source_sink: Arc<dyn crate::core::EventSink>,
    source_generation: u64,
    source_commands: Inbox,
    other_commands: Inbox,
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
    for _ in 0..4 {
        draw(cx);
    }
}

fn fixture(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::ui::actions::init(cx);
    });
    let store = crate::config::ConfigStore {
        path: std::env::temp_dir().join(format!("xenterm-editor-route-{}.db", uuid::Uuid::new_v4())),
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
    let shell = shell.unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (source_tx, source_commands) = tokio::sync::mpsc::unbounded_channel();
    let (other_tx, other_commands) = tokio::sync::mpsc::unbounded_channel();
    let (source_sink, source_generation) = shell.update(cx, |shell, cx| {
        let mut profile = crate::config::Session::new_empty();
        profile.id = "blocked-editor-routing".into();
        profile.name = "Synthetic editor routing".into();
        profile.host = "127.0.0.1".into();
        profile.port = 0;
        profile.jump_session_ids = vec!["missing-editor-routing-hop".into()];
        shell.state.store.borrow_mut().upsert(profile);
        shell.state.store.borrow_mut().set_sidebar_collapsed(true);
        shell.state.store.borrow_mut().set_sftp_panel_dock("bottom".into());
        shell.pages.terminal.update(cx, |page, cx| {
            page.open_session_tab(SOURCE, "blocked-editor-routing", cx);
            page.open_session_tab(OTHER, "blocked-editor-routing", cx);
            page.set_active_tab(Some(SOURCE.into()), cx);
            page.set_sftp_collapsed(false, cx);
        });
        assert!(shell.state.handles.borrow().is_empty(), "the fixture must start no transport");
        for (id, commands) in [(SOURCE, source_tx), (OTHER, other_tx)] {
            shell.state.sftp_handles.lock().unwrap().insert(
                id.into(),
                crate::sftp::SftpHandle {
                    commands,
                    join: runtime.spawn(async {}),
                },
            );
        }
        let source_sink = shell.pages.terminal.read(cx).event_sink_for_test(SOURCE).unwrap();
        let generation = shell.state.statuses.lock().unwrap()[SOURCE].monitor_generation;
        cx.notify();
        (source_sink, generation)
    });
    // Let the intentionally blocked connection's Closed event drain first.
    settle(cx);
    source_sink.deliver_for_generation(
        SOURCE,
        source_generation,
        vec![crate::session::protocol::SessionEvent::SftpEntries {
            path: "/synthetic/共享路径".into(),
            entries: vec![crate::session::protocol::RemoteEntry {
                name: "配置.txt".into(),
                full_path: REMOTE.into(),
                is_dir: false,
                size: ORIGINAL.len() as u64,
                modified: 0,
                mode: 0o644,
            }],
        }],
    );
    settle(cx);
    assert!(cx.debug_bounds("sftp-name-0").is_some(), "source file row is visible");
    let mut fixture = Fixture {
        shell,
        source_sink,
        source_generation,
        source_commands,
        other_commands,
    };
    while fixture.source_commands.try_recv().is_ok() {}
    while fixture.other_commands.try_recv().is_ok() {}
    (fixture, cx)
}

fn active_tab(fixture: &Fixture, cx: &mut VisualTestContext) -> Option<String> {
    fixture.shell.read_with(cx, |shell, cx| shell.pages.terminal.read(cx).active_tab_id())
}

/// Use the observed X button to locate the same chip's title, not its close hitbox.
fn select_tab(id: &str, cx: &mut VisualTestContext) {
    let close_id = SharedString::from(format!("close-{id}"));
    let close = cx.update(|window, _| window.find(close_id).bounds());
    cx.simulate_click(
        gpui_kit::point(close.left() - px(20.), close.center().y),
        Default::default(),
    );
    settle(cx);
}

fn request_file(fixture: &mut Fixture, editable: bool, cx: &mut VisualTestContext) {
    assert_eq!(active_tab(fixture, cx).as_deref(), Some(SOURCE));
    let row = cx.debug_bounds("sftp-name-0").unwrap();
    cx.simulate_mouse_move(row.center(), None, Default::default());
    cx.simulate_mouse_down(row.center(), gpui_kit::MouseButton::Right, Default::default());
    cx.simulate_mouse_up(row.center(), gpui_kit::MouseButton::Right, Default::default());
    draw(cx);
    // One Copy-to-other-tab item precedes Open / system editor / default app / View / Edit.
    let steps = if editable { 6 } else { 5 };
    cx.simulate_keystrokes(&format!("{}enter", "down ".repeat(steps)));
    settle(cx);
    assert!(
        matches!(fixture.source_commands.try_recv(), Ok(crate::sftp::SftpCommand::ReadText { remote, edit }) if remote == REMOTE && edit == editable),
        "the real source menu must issue the expected ReadText before any reply"
    );
    assert!(fixture.source_commands.try_recv().is_err());
    assert!(fixture.other_commands.try_recv().is_err());
    assert!(fixture.shell.read_with(cx, |shell, _| shell.open_file.is_none()));
}

fn reply_from_source(fixture: &Fixture, editable: bool, cx: &mut VisualTestContext) {
    fixture.source_sink.deliver_for_generation(
        SOURCE,
        fixture.source_generation,
        vec![crate::session::protocol::SessionEvent::SftpFileText {
            path: REMOTE.into(),
            name: "配置.txt".into(),
            content: ORIGINAL.into(),
            edit: editable,
            error: String::new(),
        }],
    );
    settle(cx);
}

fn edit_and_save(fixture: &Fixture, cx: &mut VisualTestContext) {
    assert!(fixture.shell.read_with(cx, |shell, _| shell.open_file.is_some()),
        "source reply must have traversed the real event pump and opened the editor");
    let save = cx.debug_bounds("file-viewer-save").expect("real Save button");
    // The textarea fills the viewer body below its header. Click inside it, and
    // verify focus and contents before typing so a missed hit cannot look like a pass.
    cx.simulate_click(
        gpui_kit::point(save.left() - px(120.), save.bottom() + px(60.)),
        Default::default(),
    );
    draw(cx);
    assert_eq!(cx.update(|window, cx| window.focused_input(cx).unwrap().value(cx).to_string()), ORIGINAL);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" });
    cx.simulate_input(EDITED);
    assert_eq!(cx.update(|window, cx| window.focused_input(cx).unwrap().value(cx).to_string()), EDITED);
    let save = cx.debug_bounds("file-viewer-save").unwrap();
    cx.simulate_click(save.center(), Default::default());
    settle(cx);
}

fn assert_saved_only_to_source(fixture: &mut Fixture) {
    // Inspect the wrong destination first: a missing source write alone could
    // mean an inert Save button rather than the cross-session routing defect.
    let mut foreign_writes = Vec::new();
    while let Ok(command) = fixture.other_commands.try_recv() {
        if let crate::sftp::SftpCommand::WriteText { remote, content } = command {
            foreign_writes.push((remote, content));
        }
    }
    assert!(foreign_writes.is_empty(), "source editor sent WriteText to unrelated active tab: {foreign_writes:?}");
    assert!(matches!(fixture.source_commands.try_recv(), Ok(crate::sftp::SftpCommand::WriteText { remote, content }) if remote == REMOTE && content == EDITED));
    assert!(fixture.source_commands.try_recv().is_err(), "one click must write only once");
}

#[gpui_kit::gpui::test]
fn same_tab_edit_reply_typing_and_save_keep_the_source_destination(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    request_file(&mut fixture, true, cx);
    reply_from_source(&fixture, true, cx);
    edit_and_save(&fixture, cx);
    assert_saved_only_to_source(&mut fixture);
}

#[gpui_kit::gpui::test]
fn delayed_source_reply_after_switching_tabs_must_never_write_to_the_new_tab(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    request_file(&mut fixture, true, cx);
    select_tab(OTHER, cx);
    assert_eq!(active_tab(&fixture, cx).as_deref(), Some(OTHER));
    reply_from_source(&fixture, true, cx);
    assert_eq!(active_tab(&fixture, cx).as_deref(), Some(OTHER), "a late reply must not silently select its source");
    edit_and_save(&fixture, cx);
    assert_saved_only_to_source(&mut fixture);
}

#[gpui_kit::gpui::test]
fn closing_source_before_its_reply_cannot_open_or_save_a_stale_editor(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    request_file(&mut fixture, true, cx);
    let close = cx.update(|window, _| window.find("close-editor-source").bounds());
    cx.simulate_click(close.center(), Default::default());
    settle(cx);
    assert!(!fixture.shell.read_with(cx, |shell, cx| shell.pages.terminal.read(cx).has_tab(SOURCE)));
    assert_eq!(active_tab(&fixture, cx).as_deref(), Some(OTHER));
    reply_from_source(&fixture, true, cx);
    assert!(fixture.shell.read_with(cx, |shell, _| shell.open_file.is_none()));
    assert!(cx.debug_bounds("file-viewer-save").is_none());
    assert!(fixture.source_commands.try_recv().is_err());
    assert!(fixture.other_commands.try_recv().is_err());
}

#[gpui_kit::gpui::test]
fn delayed_view_reply_stays_readonly_and_close_sends_no_write(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    request_file(&mut fixture, false, cx);
    select_tab(OTHER, cx);
    reply_from_source(&fixture, false, cx);
    assert!(fixture.shell.read_with(cx, |shell, _| shell.open_file.is_some()));
    assert!(cx.debug_bounds("file-viewer-save").is_none());
    let close = cx.update(|window, _| window.find("file-viewer-close").bounds());
    cx.simulate_click(close.center(), Default::default());
    settle(cx);
    assert!(fixture.shell.read_with(cx, |shell, _| shell.open_file.is_none()));
    assert!(fixture.source_commands.try_recv().is_err());
    assert!(fixture.other_commands.try_recv().is_err());
}
