use super::*;
use gpui_kit::gpui::InputEvent as _;
use std::path::PathBuf;

fn drag_enter(paths: Vec<PathBuf>, position: gpui_kit::Point<gpui_kit::Pixels>, cx: &mut VisualTestContext) {
    cx.simulate_event(gpui_kit::FileDropEvent::Entered {
        position, paths: gpui_kit::ExternalPaths(paths.into_iter().collect()),
    });
    settle(cx);
}

#[gpui_kit::gpui::test]
fn drop_routes_only_to_the_visible_sftp_directory_without_legacy_overwrite(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    let position = cx.debug_bounds("sftp-drop-zone").unwrap().center();
    let local = std::env::temp_dir().join("合成 上传.txt");
    drag_enter(vec![local.clone(), local.clone()], position, cx);
    assert!(fixture.source_commands.try_recv().is_err(), "hover does not upload");
    cx.simulate_event(gpui_kit::FileDropEvent::Submit { position });
    settle(cx);
    assert!(matches!(fixture.source_commands.try_recv(), Ok(crate::sftp::SftpCommand::UploadDropped { locals, remote_dir })
        if locals == vec![local] && remote_dir == "/synthetic/共享路径"));
    assert!(fixture.source_commands.try_recv().is_err());
    assert!(fixture.other_commands.try_recv().is_err());
}

#[gpui_kit::gpui::test]
fn drop_rechecks_the_active_session_between_native_submit_and_shell_drain(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    let position = cx.debug_bounds("sftp-drop-zone").unwrap().center();
    drag_enter(vec![std::env::temp_dir().join("stale-drop.txt")], position, cx);
    cx.update(|window, cx| {
        window.dispatch_event(gpui_kit::FileDropEvent::Submit { position }.to_platform_input(), cx);
        fixture.shell.update(cx, |shell, cx| {
            shell.pages.terminal.update(cx, |page, cx| page.set_active_tab(Some(OTHER.into()), cx));
        });
    });
    settle(cx);
    assert_eq!(active_tab(&fixture, cx).as_deref(), Some(OTHER));
    assert!(fixture.source_commands.try_recv().is_err());
    assert!(fixture.other_commands.try_recv().is_err());
}

#[gpui_kit::gpui::test]
fn drop_to_a_disconnected_transport_does_not_fallback_or_change_directory(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    let position = cx.debug_bounds("sftp-drop-zone").unwrap().center();
    fixture.source_commands.close();
    drag_enter(vec![std::env::temp_dir().join("disconnected.txt")], position, cx);
    cx.simulate_event(gpui_kit::FileDropEvent::Submit { position });
    settle(cx);
    assert!(fixture.source_commands.try_recv().is_err());
    assert!(fixture.other_commands.try_recv().is_err());
    fixture.shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.pages.terminal.read(cx).dock_panel().read(cx).path(), "/synthetic/共享路径");
    });
}

#[gpui_kit::gpui::test]
fn drop_on_the_terminal_never_becomes_pty_input_or_an_upload(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (commands, mut terminal_input) = tokio::sync::mpsc::unbounded_channel();
    fixture.shell.update(cx, |shell, _| {
        shell.state.handles.borrow_mut().insert(SOURCE.into(), crate::session::protocol::SessionHandle {
            tab_id: SOURCE.into(), commands, join: runtime.spawn(async {}),
        });
    });
    settle(cx);
    while terminal_input.try_recv().is_ok() {}
    let position = cx.debug_bounds("terminal-grid-container").unwrap().center();
    let drop_zone = cx.debug_bounds("sftp-drop-zone").unwrap();
    assert!(!drop_zone.contains(&position), "the target is the terminal, not SFTP");
    drag_enter(vec![std::env::temp_dir().join("not-a-command;echo.txt")], position, cx);
    cx.simulate_event(gpui_kit::FileDropEvent::Submit { position });
    settle(cx);
    while let Ok(command) = terminal_input.try_recv() {
        assert!(!matches!(command, crate::session::protocol::SessionCommand::RawInput(_)), "file paths must never be executed or pasted into PTY");
    }
    assert!(fixture.source_commands.try_recv().is_err());
    assert!(fixture.other_commands.try_recv().is_err());
}
