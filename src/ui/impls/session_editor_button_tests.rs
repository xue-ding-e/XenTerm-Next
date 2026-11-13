//! Real editor dialog focus and activation helpers. All state and clipboard
//! content belong to GPUI's test platform, never the user's desktop.

use super::*;
use gpui_kit::component::{Root, WindowExt as _};
use gpui_kit::gpui::{
    KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, TestAppContext, VisualTestContext,
};
use gpui_kit::test::TestWindowExt as _;
use std::time::{Duration, Instant};

struct EditorHarness;

impl Render for EditorHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

pub(super) fn editor_window(cx: &mut TestAppContext) -> &mut VisualTestContext {
    cx.update(gpui_kit::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| EditorHarness);
        Root::new(view, window, cx)
    });
    cx.simulate_resize(gpui_kit::size(px(1100.), px(900.)));
    cx
}

pub(super) fn open_editor(
    cx: &mut VisualTestContext,
    fixture: &super::tests::StoreFixture,
) -> Entity<SessionEditor> {
    cx.update(|window, cx| {
        let editor = cx.new(|_| SessionEditor::new_session(fixture.store.clone(), String::new()));
        editor.update(cx, |editor, _| {
            let mut draft = editor.draft.borrow_mut();
            draft.host = "keyboard-fixture.invalid".into();
            draft.user = "synthetic-user".into();
            draft.name = "synthetic-unsaved-name".into();
            draft.auth = "key".into();
        });
        super::super::shell::Shell::editor_dialog(editor.clone(), window, cx);
        window.draw(cx).clear(cx);
        editor
    })
}

pub(super) fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

pub(super) fn focus_action(cx: &mut VisualTestContext, id: &'static str) {
    draw(cx);
    cx.update(|window, cx| window.blur(cx));
    for _ in 0..100 {
        let focused = cx.update(|window, cx| {
            window.focus_next(cx);
            window.draw(cx).clear(cx);
            window.try_find(id).and_then(|button| button.focused()) == Some(true)
        });
        if focused {
            return;
        }
    }
    panic!("actual editor button was not keyboard-focusable: {id}");
}

pub(super) fn show_import_controls(cx: &mut VisualTestContext) {
    // Settings virtualizes whole groups. Filter through its real search input
    // so the import controls are rendered, rather than depending on overscan.
    draw(cx);
    cx.update(|window, cx| window.blur(cx));
    for _ in 0..100 {
        let in_search = cx.update(|window, cx| {
            window.focus_next(cx);
            window.draw(cx).clear(cx);
            window.focused_input(cx).is_some_and(|input| {
                input.as_input().is_some_and(|input| {
                    window
                        .try_find(("input", input.entity_id()))
                        .is_some_and(|field| field.path().contains(&"settings-sidebar".into()))
                })
            })
        });
        if in_search {
            cx.simulate_input(crate::i18n::t("私钥内容", "Private-key contents"));
            draw(cx);
            assert!(cx.debug_bounds("private-key-paste").is_some());
            return;
        }
    }
    panic!("the real Settings search input was not keyboard-focusable");
}

pub(super) fn close_editor(cx: &mut VisualTestContext, editor: &Entity<SessionEditor>) {
    cx.update(|window, cx| {
        editor.update(cx, |editor, cx| editor.clear_sensitive_inputs(window, cx));
        window.close_dialog(cx);
        window.draw(cx).clear(cx);
        assert!(!window.has_active_dialog(cx));
    });
}

pub(super) fn key_down(cx: &mut VisualTestContext, key: &str) {
    cx.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse(key).unwrap(),
        is_held: false,
        prefer_character_input: false,
    });
    draw(cx);
}

pub(super) fn key_up(cx: &mut VisualTestContext, key: &str) {
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(key).unwrap(),
    });
    draw(cx);
}

pub(super) fn click_action(cx: &mut VisualTestContext, id: &'static str) {
    // The dialog's entrance animation uses real time. Observe stable geometry,
    // as the ordinary host-key dialog tests do, rather than guess a coordinate.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut previous = None;
    let mut stable = 0;
    loop {
        draw(cx);
        let bounds = cx.debug_bounds(id);
        if bounds.is_some() && bounds == previous {
            stable += 1;
            if stable == 3 {
                cx.simulate_click(bounds.unwrap().center(), Modifiers::default());
                draw(cx);
                return;
            }
        } else {
            stable = 0;
        }
        assert!(
            Instant::now() < deadline,
            "editor button did not settle: {id}"
        );
        previous = bounds;
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn assert_open_unsaved(
    cx: &mut VisualTestContext,
    editor: &Entity<SessionEditor>,
    fixture: &super::tests::StoreFixture,
) {
    cx.update(|window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(editor.read(cx).outcome.is_none());
        assert_eq!(
            editor.read(cx).draft.borrow().name,
            "synthetic-unsaved-name"
        );
    });
    assert_unsaved(fixture);
}

fn assert_unsaved(fixture: &super::tests::StoreFixture) {
    let store = fixture.store.borrow();
    assert!(!store.path.exists());
    let saved = store.saved_state.lock().unwrap();
    assert_eq!(saved.submitted, 0);
    assert_eq!(saved.attempted, 0);
    assert!(saved.sessions.is_empty());
}

#[gpui_kit::gpui::test]
fn enter_outside_editor_action_controls_keeps_the_dialog_default(cx: &mut TestAppContext) {
    let fixture = super::tests::StoreFixture::new();
    let cx = editor_window(cx);
    let editor = open_editor(cx, &fixture);
    // Opening the real dialog focuses its container, not one of our buttons.
    key_down(cx, "enter");
    key_up(cx, "enter");
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(editor.read_with(cx, |editor, _| editor.outcome.is_none()));
    assert_unsaved(&fixture);
}
