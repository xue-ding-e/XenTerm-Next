use super::*;

fn edit_path(text: &str, cx: &mut VisualTestContext) {
    let button = cx.debug_bounds("sftp-path-edit").expect("editable remote path");
    cx.simulate_click(button.center(), Default::default());
    settle(cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" });
    cx.simulate_input(text);
    settle(cx);
}

fn shown_path(fixture: &Fixture, cx: &VisualTestContext) -> String {
    fixture.shell.read_with(cx, |shell, cx| {
        shell.pages.terminal.read(cx).dock_panel().read(cx).path().into()
    })
}

#[gpui_kit::gpui::test]
fn entered_path_waits_for_success_and_permission_failure_keeps_the_old_directory(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    let old = shown_path(&fixture, cx);
    edit_path("../拒绝访问", cx);
    cx.simulate_keystrokes("enter");
    settle(cx);
    let requested = format!("{old}/../拒绝访问");
    assert!(matches!(fixture.source_commands.try_recv(), Ok(crate::sftp::SftpCommand::ListDir(path)) if path == requested));
    assert!(fixture.source_commands.try_recv().is_err());
    assert!(fixture.other_commands.try_recv().is_err());
    assert_eq!(shown_path(&fixture, cx), old, "a request is not a successful cd");
    fixture.source_sink.deliver_for_generation(SOURCE, fixture.source_generation,
        vec![crate::session::protocol::SessionEvent::SftpError("Permission denied (synthetic)".into())]);
    settle(cx);
    assert_eq!(shown_path(&fixture, cx), old);
    edit_path("/共享/成功 路径", cx);
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(matches!(fixture.source_commands.try_recv(), Ok(crate::sftp::SftpCommand::ListDir(path)) if path == "/共享/成功 路径"));
    fixture.source_sink.deliver_for_generation(SOURCE, fixture.source_generation,
        vec![crate::session::protocol::SessionEvent::SftpEntries { path: "/共享/成功 路径".into(), entries: vec![] }]);
    settle(cx);
    assert_eq!(shown_path(&fixture, cx), "/共享/成功 路径");
}

#[gpui_kit::gpui::test]
fn disconnected_entered_navigation_does_not_change_position(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    let old = shown_path(&fixture, cx);
    fixture.source_commands.close();
    edit_path("/unreachable", cx);
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(fixture.source_commands.try_recv().is_err());
    assert!(fixture.other_commands.try_recv().is_err());
    assert_eq!(shown_path(&fixture, cx), old);
}

#[gpui_kit::gpui::test]
fn replaced_transport_rejects_a_draft_even_if_the_listing_is_identical(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    let old = shown_path(&fixture, cx);
    edit_path("/must-not-send", cx);
    fixture.shell.update(cx, |shell, _| {
        shell.state.statuses.lock().unwrap().get_mut(SOURCE).unwrap().monitor_generation += 1;
    });
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(fixture.source_commands.try_recv().is_err());
    assert!(fixture.other_commands.try_recv().is_err());
    assert_eq!(shown_path(&fixture, cx), old);
}

#[gpui_kit::gpui::test]
fn switching_tabs_never_sends_the_old_path_to_the_new_session(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    edit_path("/old-session-only", cx);
    select_tab(OTHER, cx);
    // Deliberately return focus to the still-visible stale draft. It must be
    // rejected by its origin, not silently rebound to whichever tab is active.
    let input = cx.debug_bounds("sftp-path-input").unwrap();
    cx.simulate_click(input.center(), Default::default());
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert_eq!(active_tab(&fixture, cx).as_deref(), Some(OTHER));
    assert!(fixture.source_commands.try_recv().is_err());
    assert!(fixture.other_commands.try_recv().is_err());
}
