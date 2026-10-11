//! Real Shell shortcuts followed immediately by terminal input, without a body click.
use super::*;
use gpui_kit::test::TestWindowExt as _;

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

fn assert_typed_only_to(
    selected: usize,
    inboxes: &mut [tokio::sync::mpsc::UnboundedReceiver<SessionCommand>],
    cx: &mut VisualTestContext,
) {
    cx.simulate_input("echo cycle-focus");
    cx.simulate_keystrokes("enter");
    draw(cx);
    for (index, inbox) in inboxes.iter_mut().enumerate() {
        assert_eq!(
            raw_input(inbox),
            if index == selected {
                b"echo cycle-focus\r\n".to_vec()
            } else {
                vec![]
            },
            "terminal {index} received the wrong bytes after cycling to {selected}"
        );
    }
}

fn exercise_shortcut(reverse: bool, cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let ids = ["cycle-a", "cycle-b", "cycle-c"];
    let mut inboxes = Vec::new();
    for id in ids {
        open_entry_tab(&shell, id, cx);
        inboxes.push(entry_inbox(&shell, id, &runtime, cx));
    }
    click_chip("cycle-b", cx);
    let key = if reverse {
        "ctrl-shift-tab"
    } else {
        "ctrl-tab"
    };
    let sequence = if reverse { [0, 2, 1] } else { [2, 0, 1] };
    for selected in sequence {
        cx.simulate_keystrokes(key);
        draw(cx);
        shell.read_with(cx, |shell, cx| {
            assert_eq!(
                shell.pages.terminal.read(cx).active_tab_id().as_deref(),
                Some(ids[selected])
            )
        });
        assert_typed_only_to(selected, &mut inboxes, cx);
    }
}

#[gpui_kit::gpui::test]
fn tab_cycle_ctrl_tab_wraps_and_routes_immediate_enter_to_new_terminal(cx: &mut TestAppContext) {
    exercise_shortcut(false, cx);
}

#[gpui_kit::gpui::test]
fn tab_cycle_ctrl_shift_tab_wraps_and_routes_immediate_enter_to_new_terminal(
    cx: &mut TestAppContext,
) {
    exercise_shortcut(true, cx);
}

#[gpui_kit::gpui::test]
fn tab_cycle_palette_next_and_previous_restore_terminal_input(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut inboxes = Vec::new();
    for id in ["cycle-a", "cycle-b"] {
        open_entry_tab(&shell, id, cx);
        inboxes.push(entry_inbox(&shell, id, &runtime, cx));
    }
    click_chip("cycle-b", cx);
    for (label, selected) in [
        (crate::i18n::t("下一个标签", "Next tab"), 0),
        (crate::i18n::t("上一个标签", "Previous tab"), 1),
    ] {
        cx.simulate_keystrokes("ctrl-shift-p");
        draw(cx);
        cx.simulate_input(label);
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert_typed_only_to(selected, &mut inboxes, cx);
    }
}

#[gpui_kit::gpui::test]
fn tab_cycle_single_or_empty_tab_preserves_existing_focus(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    for key in ["ctrl-tab", "ctrl-shift-tab"] {
        cx.simulate_keystrokes(key);
        draw(cx);
        assert!(cx.update(|window, _| before.is_focused(window)));
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    open_entry_tab(&shell, "cycle-only", cx);
    let mut inboxes = [entry_inbox(&shell, "cycle-only", &runtime, cx)];
    click_chip("cycle-only", cx);
    for key in ["ctrl-tab", "ctrl-shift-tab"] {
        cx.simulate_keystrokes(key);
        draw(cx);
        assert_typed_only_to(0, &mut inboxes, cx);
    }
}

#[gpui_kit::gpui::test]
fn tab_cycle_does_not_steal_settings_or_modal_focus(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    for id in ["cycle-a", "cycle-b"] {
        open_entry_tab(&shell, id, cx);
    }
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_page(PageId::Settings, window, cx)
        })
    });
    draw(cx);
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    for key in ["ctrl-tab", "ctrl-shift-tab"] {
        cx.simulate_keystrokes(key);
        draw(cx);
        assert!(cx.update(|window, _| before.is_focused(window)));
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.pages.active, PageId::Settings)
        });
    }
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_page(PageId::Terminal, window, cx)
        })
    });
    cx.simulate_keystrokes("ctrl-k");
    draw(cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    let before = cx.update(|window, cx| window.focused(cx).unwrap());
    for key in ["ctrl-tab", "ctrl-shift-tab"] {
        cx.simulate_keystrokes(key);
        draw(cx);
        assert!(cx.update(|window, _| before.is_focused(window)));
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    }
}
