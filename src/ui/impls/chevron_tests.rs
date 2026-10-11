//! Exercise the shared chevron through layout, including the real quick-group click.
use super::*;
use crate::ui::quick_commands::{DockEdge, QuickAction, QuickCommandsView};
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{component::ActiveTheme as _, div, point, px, Context, IntoElement, Render, Window};
use std::cell::RefCell;

struct ChevronHarness {
    folded: bool,
    turn: Option<u64>,
}

impl Render for ChevronHarness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(folding_chevron(
            "synthetic-chevron",
            self.folded,
            self.turn,
            cx.theme().foreground,
        ))
    }
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
    // GPUI animation uses scheduler::Instant. Advance its test clock and draw
    // intermediate frames as well as the resting frame, rather than skipping
    // the animation by invoking the click callback directly.
    for _ in 0..12 {
        cx.executor().advance_clock(Duration::from_millis(20));
        draw(cx);
    }
}

#[gpui_kit::gpui::test]
fn static_folded_and_open_chevrons_render(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, cx) = cx.add_window_view(|_, _| ChevronHarness {
        folded: true,
        turn: None,
    });
    draw(cx);
    view.update(cx, |view, cx| {
        view.folded = false;
        cx.notify();
    });
    draw(cx);
}

#[gpui_kit::gpui::test]
fn folding_and_unfolding_animations_render_through_their_resting_frames(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, cx) = cx.add_window_view(|_, _| ChevronHarness {
        folded: false,
        turn: None,
    });
    draw(cx);
    for epoch in 1..=4 {
        view.update(cx, |view, cx| {
            view.folded = epoch % 2 == 1;
            view.turn = Some(epoch);
            cx.notify();
        });
        draw(cx);
        settle(cx);
    }
}

#[gpui_kit::gpui::test]
fn reduced_motion_renders_the_folded_resting_pose(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_reduce_motion(true);
    });
    let (view, cx) = cx.add_window_view(|_, _| ChevronHarness {
        folded: true,
        turn: Some(1),
    });
    draw(cx);
    view.update(cx, |view, cx| {
        view.folded = false;
        view.turn = Some(2);
        cx.notify();
    });
    draw(cx);
}

#[gpui_kit::gpui::test]
fn quick_group_pointer_toggles_animate_and_commands_still_run(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let cache = crate::config::ConfigFile {
        quick_commands: (0..3)
            .map(|index| crate::config::QuickCommand {
                name: format!("Command {index}"),
                command: format!("echo synthetic-{index}"),
                group: "qa-quick-group".into(),
                send_enter: true,
            })
            .collect(),
        ..Default::default()
    };
    // Pure in-memory commands: no persistence, terminal process, or transport.
    let store = Rc::new(RefCell::new(crate::config::ConfigStore {
        path: Default::default(),
        backup_dir: None,
        key: [0; 32],
        keyring_enabled: false,
        saved_state: std::sync::Mutex::new(crate::config::SavedState::of_cache(&cache)).into(),
        cache,
    }));
    let (view, cx) =
        cx.add_window_view(|_, _| QuickCommandsView::new(store, DockEdge::Right, false));
    draw(cx);
    for _ in 0..4 {
        let bounds = cx
            .debug_bounds("quick-group-qa-quick-group")
            .expect("real group heading");
        cx.simulate_click(bounds.center(), Default::default());
        draw(cx);
        assert_eq!(
            view.update(cx, |view, _| view.take_action()),
            Some(QuickAction::ToggleGroup("qa-quick-group".into()))
        );
        settle(cx);
    }
    // Four toggles restore the open group. Click the first command's actual
    // row just below the measured header and assert its original payload.
    let heading = cx.debug_bounds("quick-group-qa-quick-group").unwrap();
    cx.simulate_click(
        point(heading.left() + px(40.), heading.bottom() + px(12.)),
        Default::default(),
    );
    draw(cx);
    assert_eq!(
        view.update(cx, |view, _| view.take_action()),
        Some(QuickAction::Run {
            command: "echo synthetic-0".into(),
            send_enter: true,
        })
    );
}
