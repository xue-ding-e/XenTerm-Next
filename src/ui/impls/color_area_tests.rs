//! Exercise the rendered controls through GPUI's real pointer/key dispatch.
use super::*;
use gpui_kit::{point, size, AppContext as _, Entity, TestAppContext, VisualTestContext};
use std::cell::RefCell;

struct Harness {
    area: Option<Entity<HsvAreaState>>,
    after: FocusHandle,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(24.))
            .flex()
            .flex_col()
            .when_some(self.area.clone(), |el, area| el.child(area))
            .child(
                div()
                    .id("after-hsv-area")
                    .track_focus(&self.after)
                    .tab_index(0)
                    .w(px(120.))
                    .h(px(24.))
                    .child("After colour"),
            )
    }
}

fn fixture(
    cx: &mut TestAppContext,
) -> (
    Entity<Harness>,
    Entity<HsvAreaState>,
    &mut VisualTestContext,
) {
    cx.update(gpui_kit::init);
    let (harness, cx) = cx.add_window_view(|window, cx| Harness {
        area: Some(cx.new(|cx| HsvAreaState::new(window, cx))),
        after: cx.focus_handle().tab_stop(true),
    });
    draw(cx);
    let area = harness.read_with(cx, |harness, _| harness.area.clone().unwrap());
    (harness, area, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn position(
    area: &Entity<HsvAreaState>,
    channel: Channel,
    x: f32,
    y: f32,
    cx: &mut VisualTestContext,
) -> Point<Pixels> {
    let bounds = area.read_with(cx, |area, _| area.bounds[channel as usize].get());
    assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
    point(
        bounds.origin.x + bounds.size.width * x,
        bounds.origin.y + bounds.size.height * y,
    )
}

fn drag(area: &Entity<HsvAreaState>, channel: Channel, x: f32, y: f32, cx: &mut VisualTestContext) {
    let start = position(area, channel, 0.5, 0.5, cx);
    let end = position(area, channel, x, y, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    draw(cx);
    cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    draw(cx);
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    draw(cx);
}

fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.0001, "{actual} != {expected}");
}

fn set_color(area: &Entity<HsvAreaState>, color: Hsla, cx: &mut VisualTestContext) {
    cx.update(|window, cx| area.update(cx, |area, cx| area.set_color(color, window, cx)));
    draw(cx);
}

#[gpui_kit::gpui::test]
fn rendered_sv_corners_and_midpoint_match_hsv(cx: &mut TestAppContext) {
    let (_harness, area, cx) = fixture(cx);
    for (x, y, expected) in [
        (0., 0., [255, 255, 255, 255]),
        (1., 0., [255, 0, 0, 255]),
        (0., 1., [0, 0, 0, 255]),
        (1., 1., [0, 0, 0, 255]),
        (0.5, 0.5, [128, 64, 64, 255]),
    ] {
        drag(&area, Channel::Sv, x, y, cx);
        area.read_with(cx, |area, _| {
            close(area.saturation, x);
            close(area.brightness, 1. - y);
            assert_eq!(area.color().to_rgba8(), expected);
        });
    }
}

#[gpui_kit::gpui::test]
fn capture_drag_clamps_outside_and_stops_after_release(cx: &mut TestAppContext) {
    let (_harness, area, cx) = fixture(cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorded = events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&area, move |_, event: &HsvAreaEvent, _| {
            recorded.borrow_mut().push(*event);
        })
    });
    let start = position(&area, Channel::Sv, 0.25, 0.25, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    draw(cx);
    let outside = position(&area, Channel::Sv, 1.4, 1.4, cx);
    cx.simulate_mouse_move(outside, MouseButton::Left, Modifiers::none());
    draw(cx);
    area.read_with(cx, |area, _| {
        close(area.saturation, 1.);
        close(area.brightness, 0.);
    });
    let release = position(&area, Channel::Sv, -0.2, -0.2, cx);
    cx.simulate_mouse_up(release, MouseButton::Left, Modifiers::none());
    draw(cx);
    area.read_with(cx, |area, _| {
        close(area.saturation, 0.);
        close(area.brightness, 1.);
        assert!(area.dragging.is_none());
    });
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|event| matches!(event, HsvAreaEvent::End(_)))
            .count(),
        1
    );
    assert!(events
        .borrow()
        .iter()
        .any(|event| matches!(event, HsvAreaEvent::Change { .. })));
    let before = area.read_with(cx, |area, _| area.channels());
    let event_count = events.borrow().len();
    // A move with no button and a later unrelated left drag cannot resume it.
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.simulate_mouse_move(start, MouseButton::Left, Modifiers::none());
    draw(cx);
    assert_eq!(area.read_with(cx, |area, _| area.channels()), before);
    assert_eq!(events.borrow().len(), event_count);
}

#[gpui_kit::gpui::test]
fn hue_alpha_are_continuous_and_transparency_keeps_rgb(cx: &mut TestAppContext) {
    let (_harness, area, cx) = fixture(cx);
    drag(&area, Channel::Sv, 1., 0., cx);
    drag(&area, Channel::Hue, 0.12345, 0.5, cx);
    close(area.read_with(cx, |area, _| area.hsv()[0]), 44.442);
    drag(&area, Channel::Alpha, 0.45678, 0.5, cx);
    close(area.read_with(cx, |area, _| area.alpha()), 0.45678);
    let rgb_before = area.read_with(cx, |area, _| area.color().to_rgba8());
    drag(&area, Channel::Alpha, -0.2, 0.5, cx);
    let transparent = area.read_with(cx, |area, _| area.color().to_rgba8());
    assert_eq!(&transparent[..3], &rgb_before[..3]);
    assert_eq!(transparent[3], 0);
    drag(&area, Channel::Alpha, 1.2, 0.5, cx);
    assert_eq!(
        area.read_with(cx, |area, _| area.color().to_rgba8()[3]),
        255
    );
    for (x, hue) in [(-0.2, 0.), (1.2, 360.)] {
        drag(&area, Channel::Hue, x, 0.5, cx);
        close(area.read_with(cx, |area, _| area.hsv()[0]), hue);
        assert_eq!(
            area.read_with(cx, |area, _| area.color().to_rgba8()),
            [255, 0, 0, 255]
        );
    }
}

#[gpui_kit::gpui::test]
fn quiet_sync_preserves_latent_hue_and_canonical_echo_precision(cx: &mut TestAppContext) {
    let (_harness, area, cx) = fixture(cx);
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorded = events.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&area, move |_, event: &HsvAreaEvent, _| {
            recorded.borrow_mut().push(*event);
        })
    });
    // Keep a real native drag held across exact event echoes. With H=0 and
    // V=.5, the red channel is exactly the 127.5 nearest-byte boundary.
    let start = position(&area, Channel::Sv, 0.3, 0.5, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    draw(cx);
    for saturation in [0.3, 0.31, 0.32, 0.49, 0.7] {
        let at = position(&area, Channel::Sv, saturation, 0.5, cx);
        cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::none());
        draw(cx);
        let (echoed, emitted_rgba) = events
            .borrow()
            .iter()
            .rev()
            .find_map(|event| match event {
                HsvAreaEvent::Change { color, rgba } => Some((*color, *rgba)),
                HsvAreaEvent::End(_) => None,
            })
            .expect("native pointer movement emits a colour");
        let before = area.read_with(cx, |area, _| area.channels());
        let bytes = area.read_with(cx, |area, _| area.rgba8());
        assert_eq!(
            emitted_rgba, bytes,
            "the event owns its authoritative HSV bytes"
        );
        assert_eq!(bytes[0], 128);
        if saturation == 0.31 {
            assert_eq!(bytes, [128, 88, 88, 255]);
        }
        let event_count = events.borrow().len();
        set_color(&area, echoed, cx);
        area.read_with(cx, |area, _| {
            assert_eq!(
                area.channels(),
                before,
                "an exact event echo preserves float HSV"
            );
            assert_eq!(area.rgba8(), bytes);
            assert!(
                area.is_dragging(),
                "quiet echoes must not release the held pointer"
            );
        });
        assert_eq!(
            events.borrow().len(),
            event_count,
            "quiet echoes emit no events"
        );
    }
    let event_bytes: Vec<_> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            HsvAreaEvent::Change { rgba, .. } => Some(*rgba),
            HsvAreaEvent::End(_) => None,
        })
        .collect();
    assert_eq!(
        event_bytes,
        [
            [128, 89, 89, 255],
            [128, 88, 88, 255],
            [128, 87, 87, 255],
            [128, 65, 65, 255],
            [128, 38, 38, 255],
        ],
        "earlier events retain their own bytes after subsequent native moves"
    );
    let release = position(&area, Channel::Sv, 0.7, 0.5, cx);
    cx.simulate_mouse_up(release, MouseButton::Left, Modifiers::none());
    draw(cx);
    drag(&area, Channel::Hue, 0.712345, 0.5, cx);
    drag(&area, Channel::Sv, 0.62345, 0.34567, cx);
    let before = area.read_with(cx, |area, _| area.channels());
    let bytes = area.read_with(cx, |area, _| area.color().to_rgba8());
    let [h, s, l, a] =
        csscolorparser::Color::from_rgba8(bytes[0], bytes[1], bytes[2], bytes[3]).to_hsla();
    set_color(&area, hsla(h / 360., s, l, a), cx);
    assert_eq!(area.read_with(cx, |area, _| area.channels()), before);
    set_color(&area, hsla(0., 0., 0., 1.), cx);
    area.read_with(cx, |area, _| {
        close(area.hue, before[0]);
        close(area.saturation, before[1]);
        close(area.brightness, 0.);
    });
    set_color(&area, hsla(0., 0., 0.5, 1.), cx);
    area.read_with(cx, |area, _| {
        close(area.hue, before[0]);
        close(area.saturation, 0.);
        close(area.brightness, 0.5);
    });
    cx.update(|window, cx| area.update(cx, |area, cx| area.reset_default(window, cx)));
    assert_eq!(
        area.read_with(cx, |area, _| area.channels()),
        [0., 0., 1., 1.]
    );
}

#[gpui_kit::gpui::test]
fn keyboard_focus_steps_modifiers_and_endpoints(cx: &mut TestAppContext) {
    let (harness, area, cx) = fixture(cx);
    cx.update(|window, cx| {
        let focus = area.read(cx).focus[0].clone();
        focus.focus(window, cx);
    });
    draw(cx);
    cx.simulate_keystrokes("home right shift-up");
    area.read_with(cx, |area, _| {
        close(area.saturation, 0.01);
        close(area.brightness, 0.001);
    });
    let before = area.read_with(cx, |area, _| area.channels());
    cx.simulate_keystrokes("ctrl-right alt-up cmd-left");
    assert_eq!(area.read_with(cx, |area, _| area.channels()), before);
    cx.simulate_keystrokes("end tab");
    draw(cx);
    assert!(cx.update(|window, cx| area.read(cx).focus[1].is_focused(window)));
    cx.simulate_keystrokes("home right shift-right");
    close(area.read_with(cx, |area, _| area.hsv()[0]), 1.1);
    cx.simulate_keystrokes("end tab");
    draw(cx);
    assert!(cx.update(|window, cx| area.read(cx).focus[2].is_focused(window)));
    cx.simulate_keystrokes("home right shift-right");
    close(area.read_with(cx, |area, _| area.alpha()), 0.011);
    cx.simulate_keystrokes("end tab");
    draw(cx);
    assert!(cx.update(|window, cx| harness.read(cx).after.is_focused(window)));
    assert_eq!(
        area.read_with(cx, |area, _| area.channels()),
        [1., 1., 1., 1.]
    );
}

#[gpui_kit::gpui::test]
fn right_button_modified_clicks_and_degenerate_bounds_do_not_edit(cx: &mut TestAppContext) {
    let (_harness, area, cx) = fixture(cx);
    let center = position(&area, Channel::Sv, 0.5, 0.5, cx);
    let before = area.read_with(cx, |area, _| area.channels());
    cx.simulate_mouse_down(center, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(center, MouseButton::Right, Modifiers::none());
    for modifiers in [
        Modifiers {
            control: true,
            ..Modifiers::none()
        },
        Modifiers {
            alt: true,
            ..Modifiers::none()
        },
        Modifiers {
            platform: true,
            ..Modifiers::none()
        },
    ] {
        cx.simulate_click(center, modifiers);
    }
    draw(cx);
    assert_eq!(area.read_with(cx, |area, _| area.channels()), before);
    assert!(area.read_with(cx, |area, _| area.dragging.is_none()));
    area.update(cx, |area, _| {
        let bounds = area.bounds[0].get();
        area.bounds[0].set(Bounds {
            origin: bounds.origin,
            size: size(px(0.), px(0.)),
        });
        assert!(area.coordinates(Channel::Sv, center).is_none());
        area.bounds[0].set(bounds);
        assert!(area
            .coordinates(Channel::Sv, point(px(f32::NAN), px(1.)))
            .is_none());
    });
    set_color(
        &area,
        Hsla {
            h: f32::NAN,
            s: 1.,
            l: 0.5,
            a: 1.,
        },
        cx,
    );
    assert_eq!(area.read_with(cx, |area, _| area.channels()), before);
}

#[gpui_kit::gpui::test]
fn removing_dragged_control_releases_weak_callbacks(cx: &mut TestAppContext) {
    let (harness, area, cx) = fixture(cx);
    let start = position(&area, Channel::Sv, 0.3, 0.3, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    draw(cx);
    assert!(area.read_with(cx, |area, _| area.dragging.is_some()));
    let weak = area.downgrade();
    harness.update(cx, |harness, cx| {
        harness.area = None;
        cx.notify();
    });
    drop(area);
    draw(cx);
    cx.simulate_mouse_move(
        point(px(500.), px(500.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        point(px(500.), px(500.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    draw(cx);
    assert!(
        weak.upgrade().is_none(),
        "paint callbacks must not retain an unmounted editor"
    );
}
