//! Probe tests for the animation patterns the UI uses: each one mounts the
//! same shape a real view mounts and asserts the animation requests frames —
//! which is the whole mechanism (`with_animation` → frame request → `notify`
//! → re-render → delta advances). A pattern that stops requesting frames is an
//! animation that exists but never plays, which is exactly the failure mode
//! worth catching here rather than by eye.

use std::time::Duration;

use gpui_kit::component::{
    progress::Progress, spinner::Spinner, ActiveTheme as _, Icon, Sizable as _,
};
use gpui_kit::gpui::TestAppContext;
use gpui_kit::{
    div, prelude::*, Animation, AnimationExt as _, Context, IntoElement, Render, SharedString,
    Transformation, Window, percentage,
};

/// Every pattern the UI animates with, mounted side by side.
struct ProbeView;

impl Render for ProbeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        div()
            .size_full()
            .child(
                // The tab-dot / sidebar-dot pulse: repeating opacity sweep on a div.
                div()
                    .size_2()
                    .rounded_full()
                    .bg(muted)
                    .with_animation(
                        "probe-pulse",
                        Animation::new(Duration::from_millis(1200)).repeat(),
                        |dot, delta| {
                            let pulse = 0.5 - 0.5 * (std::f32::consts::TAU * delta).cos();
                            dot.opacity(0.35 + 0.65 * pulse)
                        },
                    ),
            )
            .child(
                // The transfer-row / copied-label entrance: one-shot fade-in.
                div()
                    .with_animation(
                        "probe-fade",
                        Animation::new(Duration::from_millis(200)),
                        |row, delta| row.opacity(delta),
                    )
                    .child("fading in"),
            )
            .child(
                // The folding chevron: an icon rotating through an animation.
                Icon::new(gpui_kit::assets::IconName::ChevronDown)
                    .size_3()
                    .with_animation(
                        "probe-chevron",
                        Animation::new(Duration::from_millis(160)),
                        move |icon, delta| {
                            icon.transform(Transformation::rotate(percentage(delta)))
                        },
                    ),
            )
            .child(Spinner::new().with_size(gpui_kit::component::Size::Small))
            .child(
                Progress::new("probe-indeterminate")
                    .value(0.0)
                    .loading(true)
                    .w_full(),
            )
    }
}

#[gpui_kit::gpui::test]
fn every_animated_pattern_keeps_requesting_frames(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (_view, cx) = cx.add_window_view(|_, _| ProbeView);
    cx.run_until_parked();

    // First simulated frame after mount: every unfinished animation schedules
    // its continuation. Five patterns mounted; each contributes one callback.
    let requested = cx.update(|window, cx| window.simulate_next_frame(cx));
    assert!(
        requested >= 5,
        "expected the pulse, fade, chevron, spinner and indeterminate progress \
         to each request a frame, got {requested}"
    );

    // And they keep going: the repeating ones never settle.
    for _ in 0..3 {
        cx.run_until_parked();
        let requested = cx.update(|window, cx| window.simulate_next_frame(cx));
        assert!(
            requested >= 3,
            "repeating animations must keep requesting frames, got {requested}"
        );
    }
}

#[gpui_kit::gpui::test]
fn one_shot_animations_settle_and_stop_requesting(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    struct OneShot;

    impl Render for OneShot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().with_animation(
                "probe-oneshot",
                Animation::new(Duration::from_millis(50)),
                |row, delta| row.opacity(delta),
            )
        }
    }

    let (_view, cx) = cx.add_window_view(|_, _| OneShot);
    cx.run_until_parked();

    // The animation's clock is the real one — the test executor's virtual
    // clock does not move `Instant::now` — so the wait has to be real too.
    // Sleep past the 50ms lifetime, then draw: the first simulated frame is
    // the one that discovers the animation finished, and the one after that
    // must find nothing left running. A settled animation must not keep the
    // window redrawing forever.
    std::thread::sleep(Duration::from_millis(80));
    cx.run_until_parked();
    let _settle = cx.update(|window, cx| window.simulate_next_frame(cx));
    let requested = cx.update(|window, cx| window.simulate_next_frame(cx));
    assert_eq!(
        requested, 0,
        "a finished one-shot animation must stop requesting frames"
    );
}
