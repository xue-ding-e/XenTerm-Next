//! Real Shell pane-cycle shortcut followed by input, with no body click after cycling.
use super::*;

fn split_fixture(
    cx: &mut TestAppContext,
) -> (
    Entity<Shell>,
    &mut VisualTestContext,
    tokio::runtime::Runtime,
    Vec<tokio::sync::mpsc::UnboundedReceiver<SessionCommand>>,
) {
    let (shell, cx) = fixture(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut inboxes = Vec::new();
    for id in ["pane-left", "pane-right"] {
        open_entry_tab(&shell, id, cx);
        inboxes.push(entry_inbox(&shell, id, &runtime, cx));
    }
    shell.update(cx, |shell, cx| {
        shell.pages.terminal.update(cx, |page, cx| {
            page.set_active_tab(Some("pane-left".into()), cx)
        })
    });
    draw(cx);
    let area = cx.debug_bounds("terminal-pane-area").unwrap();
    cx.simulate_click(area.center(), Default::default());
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
    cx.simulate_input("right-before-cycle");
    assert_eq!(raw_input(&mut inboxes[1]), b"right-before-cycle");
    assert!(raw_input(&mut inboxes[0]).is_empty());
    (shell, cx, runtime, inboxes)
}

#[gpui_kit::gpui::test]
fn pane_cycle_shortcut_routes_input_to_each_selected_pane_without_mouse(cx: &mut TestAppContext) {
    let (shell, cx, _runtime, mut inboxes) = split_fixture(cx);
    for selected in [0, 1, 0, 1] {
        cx.simulate_keystrokes("ctrl-f6");
        for _ in 0..3 {
            draw(cx);
        }
        shell.read_with(cx, |shell, cx| {
            assert_eq!(
                shell.pages.terminal.read(cx).active_tab_id().as_deref(),
                Some(if selected == 0 {
                    "pane-left"
                } else {
                    "pane-right"
                }),
                "the actual Shell key action must change pane selection"
            )
        });
        // No direct focus assignment or pointer click after the shortcut.
        cx.simulate_input("echo pane-cycle");
        cx.simulate_keystrokes("enter");
        draw(cx);
        for (index, inbox) in inboxes.iter_mut().enumerate() {
            assert_eq!(
                raw_input(inbox),
                if index == selected {
                    b"echo pane-cycle\r\n".to_vec()
                } else {
                    vec![]
                },
                "input must follow the selected pane, receiver {index}"
            );
        }
    }
}

#[gpui_kit::gpui::test]
fn pane_cycle_empty_and_single_pane_keep_the_existing_input_target(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    cx.simulate_keystrokes("ctrl-f6");
    draw(cx);
    assert!(cx.update(|window, _| before.is_focused(window)));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    open_entry_tab(&shell, "pane-only", cx);
    let mut inbox = entry_inbox(&shell, "pane-only", &runtime, cx);
    let area = cx.debug_bounds("terminal-pane-area").unwrap();
    cx.simulate_click(area.center(), Default::default());
    draw(cx);
    cx.simulate_keystrokes("ctrl-f6");
    draw(cx);
    assert_entry_input(&mut inbox, cx);
}

#[gpui_kit::gpui::test]
fn pane_cycle_does_not_take_focus_from_settings_or_an_open_dialog(cx: &mut TestAppContext) {
    let (shell, cx, _runtime, _inboxes) = split_fixture(cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_page(PageId::Settings, window, cx)
        })
    });
    draw(cx);
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    cx.simulate_keystrokes("ctrl-f6");
    draw(cx);
    assert!(cx.update(|window, _| before.is_focused(window)));
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.pages.active, PageId::Settings)
    });
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_page(PageId::Terminal, window, cx)
        })
    });
    draw(cx);
    cx.simulate_keystrokes("ctrl-k");
    draw(cx);
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    let pane_before = shell.read_with(cx, |shell, cx| {
        shell.pages.terminal.read(cx).navigation_test_state()
    });
    cx.simulate_keystrokes("ctrl-f6");
    draw(cx);
    assert_eq!(
        shell.read_with(cx, |shell, cx| shell
            .pages
            .terminal
            .read(cx)
            .navigation_test_state()),
        pane_before,
        "a modal must preserve both the pane and tab target"
    );
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(cx.update(|window, _| before.is_focused(window)));
}
