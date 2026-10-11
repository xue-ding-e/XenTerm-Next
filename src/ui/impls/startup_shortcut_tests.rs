//! Exercise initial focus and page navigation through actual Shell events.
//! No test-only focus assignment precedes cold-window shortcuts.
use super::*;
use gpui_kit::test::TestWindowExt as _;

fn shortcut_reaches_app(key: &str, cx: &mut VisualTestContext) {
    let observed = Rc::new(Cell::new(0));
    let count = observed.clone();
    let _subscription =
        cx.update(|_, cx| cx.intercept_keystrokes(move |_, _, _| count.set(count.get() + 1)));
    cx.simulate_keystrokes(key);
    draw(cx);
    assert!(observed.get() > 0, "test keystroke did not enter GPUI");
}

fn assert_quick_connect(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    assert!(
        cx.update(|window, cx| window.has_active_dialog(cx)),
        "Ctrl+K reached GPUI but did not open quick-connect"
    );
    shell.read_with(cx, |shell, _| {
        assert!(matches!(shell.overlay, Overlay::QuickConnect(_)))
    });
    assert!(cx.update(|window, cx| window.focused_input(cx).is_some()));
}

fn dismiss(cx: &mut VisualTestContext) {
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.background_executor
        .advance_clock(std::time::Duration::from_millis(250));
    draw(cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
}

fn click_chip(id: &str, cx: &mut VisualTestContext) {
    let bounds = cx.update(|window, _| {
        window
            .find(SharedString::from(format!("close-{id}")))
            .bounds()
    });
    cx.simulate_click(
        gpui_kit::point(bounds.left() - px(20.), bounds.center().y),
        Default::default(),
    );
    draw(cx);
}

#[gpui_kit::gpui::test]
fn startup_shortcut_ctrl_k_opens_from_untouched_empty_window(cx: &mut TestAppContext) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    eprintln!(
        "cold window initially has focus: {}",
        cx.update(|window, cx| window.focused(cx).is_some())
    );
    shortcut_reaches_app("ctrl-k", cx);
    assert_quick_connect(&shell, cx);
    dismiss(cx);
    shortcut_reaches_app("ctrl-k", cx);
    assert_quick_connect(&shell, cx);
}

#[gpui_kit::gpui::test]
fn startup_shortcut_command_palette_also_resolves_without_pointer_focus(cx: &mut TestAppContext) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    shortcut_reaches_app("ctrl-shift-p", cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    shell.read_with(cx, |shell, _| {
        assert!(matches!(shell.overlay, Overlay::Commands(_)))
    });
}

#[gpui_kit::gpui::test]
fn startup_shortcut_returns_after_button_opened_dialog_is_cancelled(cx: &mut TestAppContext) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    click_entry_element("empty-quick-connect", cx);
    assert_quick_connect(&shell, cx);
    dismiss(cx);
    shortcut_reaches_app("ctrl-k", cx);
    assert_quick_connect(&shell, cx);
}

#[gpui_kit::gpui::test]
fn startup_shortcut_returns_when_the_last_terminal_is_closed(cx: &mut TestAppContext) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    open_entry_tab(&shell, "last-startup-tab", cx);
    click_chip("last-startup-tab", cx);
    cx.simulate_keystrokes("ctrl-shift-w");
    draw(cx);
    shell.read_with(cx, |shell, cx| {
        assert!(shell.pages.terminal.read(cx).tabs.is_empty())
    });
    shortcut_reaches_app("ctrl-k", cx);
    assert_quick_connect(&shell, cx);
}

#[gpui_kit::gpui::test]
fn startup_shortcut_existing_terminal_keeps_input_after_cancel(cx: &mut TestAppContext) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    open_entry_tab(&shell, "live-startup-tab", cx);
    let mut inbox = entry_inbox(&shell, "live-startup-tab", &runtime, cx);
    click_chip("live-startup-tab", cx);
    shortcut_reaches_app("ctrl-k", cx);
    assert_quick_connect(&shell, cx);
    dismiss(cx);
    assert_entry_input(&mut inbox, cx);
}

#[gpui_kit::gpui::test]
fn startup_shortcut_respects_existing_modal_and_settings_input(cx: &mut TestAppContext) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    click_entry_element("empty-quick-connect", cx);
    assert_quick_connect(&shell, cx);
    let initial = shell.read_with(cx, |shell, _| match &shell.overlay {
        Overlay::QuickConnect(list) => list.entity_id(),
        _ => unreachable!(),
    });
    shortcut_reaches_app("ctrl-k", cx);
    let current = shell.read_with(cx, |shell, _| match &shell.overlay {
        Overlay::QuickConnect(list) => list.entity_id(),
        _ => unreachable!(),
    });
    assert_eq!(initial, current);
    dismiss(cx);
    click_entry_element("nav-Settings", cx);
    let mut found = false;
    for _ in 0..60 {
        cx.update(|window, cx| window.focus_next(cx));
        draw(cx);
        if cx.update(|window, cx| window.focused_input(cx).is_some()) {
            found = true;
            break;
        }
    }
    assert!(found, "settings fixture needs a real focused input");
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    shortcut_reaches_app("ctrl-k", cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(cx.update(|window, _| before.is_focused(window)));
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.pages.active, PageId::Settings)
    });
}

#[gpui_kit::gpui::test]
fn startup_shortcut_persistent_shell_focus_survives_empty_workspace_page_changes(
    cx: &mut TestAppContext,
) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    for id in ["nav-Settings", "nav-Terminal", "nav-Sessions"] {
        click_entry_element(id, cx);
        shortcut_reaches_app("ctrl-k", cx);
        assert_quick_connect(&shell, cx);
        dismiss(cx);
    }
}

fn click_pasted_connections_field(cx: &mut VisualTestContext) {
    // Match the rendered native field, not the taller layout wrapper around it.
    let bounds = cx.update(|window, _| {
        let fields: Vec<_> = gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|field| {
                field.visible()
                    && field.label()
                        == Some(crate::i18n::t(
                            "每行一个：主机|端口|用户名|密码|名称",
                            "One per line: host|port|user|password|name",
                        ))
            })
            .collect();
        assert_eq!(
            fields.len(),
            1,
            "exactly one visible pasted-connections textarea"
        );
        fields[0].bounds()
    });
    cx.simulate_click(bounds.center(), Default::default());
    draw(cx);
    assert!(cx.update(|window, cx| window.focused_input(cx).is_some()));
}

fn focus_and_type_connection_draft(cx: &mut VisualTestContext) {
    cx.update(|window, _| {
        window.resize(gpui_kit::size(px(1180.), px(812.)));
        // Native focus/blur callbacks run only for an active window.
        window.activate_window();
    });
    draw(cx);
    click_entry_element("nav-Settings", cx);
    let page = cx
        .debug_bounds("settings-nav-Connections")
        .expect("visible Connections settings navigation");
    cx.simulate_click(page.center(), Default::default());
    draw(cx);
    click_pasted_connections_field(cx);
    cx.simulate_input("qa-focus");
    draw(cx);
    shortcut_reaches_app("ctrl-k", cx);
    assert!(
        !cx.update(|window, cx| window.has_active_dialog(cx)),
        "Ctrl+K inside the draft must remain protected"
    );
    cx.simulate_input("-ok");
    draw(cx);
    assert_eq!(
        cx.update(|window, cx| window.focused_input(cx).unwrap().value(cx).to_string()),
        "qa-focus-ok"
    );
}

#[gpui_kit::gpui::test]
fn page_navigation_from_typed_settings_draft_restores_empty_terminal_shortcuts(
    cx: &mut TestAppContext,
) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    focus_and_type_connection_draft(cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a backspace"
    } else {
        "ctrl-a backspace"
    });
    draw(cx);
    assert_eq!(
        cx.update(|window, cx| window.focused_input(cx).unwrap().value(cx).to_string()),
        ""
    );
    let draft_focus = cx.update(|window, cx| window.focused(cx).unwrap());
    click_entry_element("nav-Terminal", cx);
    eprintln!("after page switch, hidden draft still focused: {}, toolkit still reports focused input: {}",
        cx.update(|window, _| draft_focus.is_focused(window)),
        cx.update(|window, cx| window.focused_input(cx).is_some()));
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.pages.active, PageId::Terminal);
        assert!(shell.pages.terminal.read(cx).tabs.is_empty());
    });
    shortcut_reaches_app("ctrl-k", cx);
    assert_quick_connect(&shell, cx);
    dismiss(cx);
    shortcut_reaches_app("ctrl-shift-p", cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
}

#[gpui_kit::gpui::test]
fn page_navigation_from_typed_settings_draft_focuses_existing_terminal(cx: &mut TestAppContext) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    open_entry_tab(&shell, "page-return-terminal", cx);
    let mut inbox = entry_inbox(&shell, "page-return-terminal", &runtime, cx);
    click_chip("page-return-terminal", cx);
    focus_and_type_connection_draft(cx);
    click_entry_element("nav-Terminal", cx);
    // No terminal-body click or focus assignment after this navigation action.
    cx.simulate_input("echo page-focus");
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(raw_input(&mut inbox), b"echo page-focus\r\n");
    click_entry_element("nav-Settings", cx);
    click_pasted_connections_field(cx);
    assert_eq!(
        cx.update(|window, cx| window.focused_input(cx).unwrap().value(cx).to_string()),
        "qa-focus-ok",
        "terminal typing must not alter the cached settings draft"
    );
}

#[gpui_kit::gpui::test]
fn page_navigation_reselecting_settings_preserves_its_existing_draft_focus(
    cx: &mut TestAppContext,
) {
    let (_shell, cx) = fixture_without_forced_focus(cx);
    focus_and_type_connection_draft(cx);
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    click_entry_element("nav-Settings", cx);
    shortcut_reaches_app("ctrl-k", cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(cx.update(|window, _| before.is_focused(window)));
    cx.simulate_input("-kept");
    assert_eq!(
        cx.update(|window, cx| window.focused_input(cx).unwrap().value(cx).to_string()),
        "qa-focus-ok-kept"
    );
}

#[gpui_kit::gpui::test]
fn page_navigation_from_typed_settings_to_sessions_restores_shortcuts(cx: &mut TestAppContext) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    focus_and_type_connection_draft(cx);
    click_entry_element("nav-Sessions", cx);
    shortcut_reaches_app("ctrl-k", cx);
    assert_quick_connect(&shell, cx);
}

#[gpui_kit::gpui::test]
fn page_navigation_while_a_dialog_is_open_preserves_modal_input(cx: &mut TestAppContext) {
    let (shell, cx) = fixture_without_forced_focus(cx);
    click_entry_element("empty-quick-connect", cx);
    assert_quick_connect(&shell, cx);
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    // A queued page change must not steal input from an already open dialog.
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_page(PageId::Settings, window, cx)
        })
    });
    draw(cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(cx.update(|window, _| before.is_focused(window)));
    cx.simulate_input("modal-kept");
    assert_eq!(
        cx.update(|window, cx| window.focused_input(cx).unwrap().value(cx).to_string()),
        "modal-kept"
    );
}
