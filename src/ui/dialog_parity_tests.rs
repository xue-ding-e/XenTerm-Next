//! Event-level checks for the GPUI dialog and single-line note input used by the editor.
//! All text is synthetic; the harness never loads configuration or a keyring.
use std::{cell::Cell, rc::Rc};

use gpui_kit::{
    component::{
        dialog::DialogButtonProps,
        input::{Input, InputState},
        select::{Select, SelectState},
        v_flex, Root, WindowExt as _,
    },
    div,
    gpui::{Focusable as _, Modifiers, MouseButton, TestAppContext, VisualTestContext},
    point,
    prelude::*,
    px, Context, Entity, IntoElement, SharedString, Window,
};

struct DialogHarness;

impl Render for DialogHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
    }
}

struct Fixture {
    input: Entity<InputState>,
    select: Entity<SelectState<Vec<SharedString>>>,
    cancelled: Rc<Cell<usize>>,
    confirmed: Rc<Cell<usize>>,
    closed: Rc<Cell<usize>>,
}

fn open_dialog(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| DialogHarness);
        Root::new(view, window, cx)
    });
    let fixture = cx.update(|window, cx| {
        let input = cx.new(|cx| InputState::new(window, cx).default_value("fixture note"));
        let select =
            cx.new(|cx| SelectState::new(vec!["SSH".into(), "Local".into()], None, window, cx));
        let fixture = Fixture {
            input: input.clone(),
            select: select.clone(),
            cancelled: Rc::new(Cell::new(0)),
            confirmed: Rc::new(Cell::new(0)),
            closed: Rc::new(Cell::new(0)),
        };
        let cancelled = fixture.cancelled.clone();
        let confirmed = fixture.confirmed.clone();
        let closed = fixture.closed.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let cancelled = cancelled.clone();
            let confirmed = confirmed.clone();
            let closed = closed.clone();
            dialog
                .title("Fixture editor")
                .width(px(420.))
                .overlay_closable(false)
                .button_props(
                    DialogButtonProps::default()
                        .on_cancel(move |_, _, _| {
                            cancelled.set(cancelled.get() + 1);
                            true
                        })
                        .on_ok(move |_, _, _| {
                            confirmed.set(confirmed.get() + 1);
                            false
                        }),
                )
                .on_close(move |_, _, _| closed.set(closed.get() + 1))
                .child(
                    v_flex()
                        .gap_4()
                        .child(
                            div()
                                .debug_selector(|| "parity-note".into())
                                .w(px(256.))
                                .child(Input::new(&input).w_full()),
                        )
                        .child(
                            div()
                                .debug_selector(|| "parity-select".into())
                                .w(px(256.))
                                .child(Select::new(&select).w_full()),
                        ),
                )
        });
        fixture.input.read(cx).focus_handle(cx).focus(window, cx);
        fixture
    });
    // Dialog entrance animation uses real Instant, not the test executor clock.
    // Pointer probes must start only after its bounds stop moving.
    wait_for_stable_dialog_bounds(cx);
    (fixture, cx)
}

fn wait_for_stable_dialog_bounds(cx: &mut VisualTestContext) {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut previous = None;
    let mut stable_samples = 0;
    loop {
        draw(cx);
        let bounds = cx
            .debug_bounds("parity-note")
            .zip(cx.debug_bounds("parity-select"));
        if bounds.is_some() && bounds == previous {
            stable_samples += 1;
            // Require several separately rendered samples, rather than one
            // unchanged frame or a fixed guess at the animation duration.
            if stable_samples == 3 {
                return;
            }
        } else {
            stable_samples = 0;
        }
        assert!(
            Instant::now() < deadline,
            "dialog bounds did not stabilize: previous={previous:?}, current={bounds:?}"
        );
        previous = bounds;
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn assert_cancelled(fixture: &Fixture, cx: &mut VisualTestContext) {
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(fixture.cancelled.get(), 1);
    assert_eq!(fixture.closed.get(), 1);
    assert_eq!(
        fixture.confirmed.get(),
        0,
        "cancel must never submit the editor"
    );
}

#[gpui_kit::gpui::test]
fn escape_cancels_root_dialog_with_focused_input(cx: &mut TestAppContext) {
    let (fixture, cx) = open_dialog(cx);
    cx.simulate_input(" unsaved");
    cx.simulate_keystrokes("escape");
    assert_cancelled(&fixture, cx);
}

#[gpui_kit::gpui::test]
fn command_period_cancels_root_dialog_with_focused_input(cx: &mut TestAppContext) {
    let (fixture, cx) = open_dialog(cx);
    cx.update(|_, cx| super::dialogs::bind_macos_cancel(cx));
    cx.simulate_input(" unsaved");
    cx.simulate_keystrokes("cmd-.");
    assert_cancelled(&fixture, cx);
}

#[gpui_kit::gpui::test]
fn escape_dismisses_nested_select_before_dialog(cx: &mut TestAppContext) {
    let (fixture, cx) = open_dialog(cx);
    let bounds = cx
        .debug_bounds("parity-select")
        .expect("select trigger is laid out");
    cx.simulate_click(bounds.center(), Modifiers::default());
    draw(cx);
    cx.simulate_keystrokes("escape");
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(fixture.closed.get(), 0);
    assert_eq!(fixture.confirmed.get(), 0);
    cx.simulate_keystrokes("escape");
    assert_cancelled(&fixture, cx);
}

#[gpui_kit::gpui::test]
fn long_chinese_note_drag_selects_and_scrolls_in_both_directions(cx: &mut TestAppContext) {
    let (fixture, cx) = open_dialog(cx);
    let text = "甲乙丙丁戊己庚辛壬癸".repeat(8);
    assert_eq!(text.chars().count(), 80);
    cx.update(|window, cx| {
        fixture.input.update(cx, |state, cx| {
            state.set_value(text.clone(), window, cx);
            state.set_selected_range(0..0, cx);
        })
    });
    draw(cx);
    let bounds = cx
        .debug_bounds("parity-note")
        .expect("note input is laid out");
    let start = point(bounds.left() + px(20.), bounds.center().y);
    let right = point(bounds.right() + px(90.), bounds.center().y);
    cx.simulate_mouse_move(start, None, Modifiers::default());
    draw(cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    for _ in 0..12 {
        cx.simulate_mouse_move(right, MouseButton::Left, Modifiers::default());
        draw(cx);
    }
    cx.simulate_mouse_up(right, MouseButton::Left, Modifiers::default());
    let forward = fixture.input.read_with(cx, |state, _| {
        (state.selected_range(), state.scroll_offset())
    });
    assert!(
        forward.0.end > forward.0.start,
        "mouse drag must select actual text"
    );
    assert!(
        forward.1.x < px(0.),
        "overflowing note must scroll horizontally"
    );
    assert!(text.is_char_boundary(forward.0.start) && text.is_char_boundary(forward.0.end));
    let start = point(bounds.right() - px(20.), bounds.center().y);
    let left = point(bounds.left() - px(90.), bounds.center().y);
    cx.simulate_mouse_move(start, None, Modifiers::default());
    draw(cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    for _ in 0..12 {
        cx.simulate_mouse_move(left, MouseButton::Left, Modifiers::default());
        draw(cx);
    }
    cx.simulate_mouse_up(left, MouseButton::Left, Modifiers::default());
    let backward = fixture.input.read_with(cx, |state, _| {
        (state.selected_range(), state.scroll_offset(), state.value())
    });
    assert!(
        backward.0.end > backward.0.start,
        "reverse mouse drag must select text"
    );
    assert!(
        backward.1.x > forward.1.x,
        "reverse drag must scroll back toward the beginning"
    );
    assert!(text.is_char_boundary(backward.0.start) && text.is_char_boundary(backward.0.end));
    assert_eq!(backward.2.as_ref(), text);
    assert_eq!(
        cx.debug_bounds("parity-note").unwrap(),
        bounds,
        "selection and scrolling must not expand the field geometry"
    );
    assert_eq!(fixture.confirmed.get(), 0);
}

#[gpui_kit::gpui::test]
fn command_period_dismisses_nested_select_before_dialog(cx: &mut TestAppContext) {
    let (fixture, cx) = open_dialog(cx);
    cx.update(|_, cx| super::dialogs::bind_macos_cancel(cx));
    let bounds = cx
        .debug_bounds("parity-select")
        .expect("select trigger is laid out");
    cx.simulate_click(bounds.center(), Modifiers::default());
    draw(cx);
    cx.simulate_keystrokes("cmd-.");
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(fixture.closed.get(), 0);
    assert_eq!(fixture.confirmed.get(), 0);
    assert!(fixture
        .select
        .read_with(cx, |state, _| state.selected_value().is_none()));
    cx.simulate_keystrokes("cmd-.");
    assert_cancelled(&fixture, cx);
}
