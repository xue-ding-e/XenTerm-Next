//! Independent queue-order regression: no clipboard controls or new prompt fields.
//! This probe can run against the baseline with only test wiring/debug selectors.

use super::*;
use gpui_kit::{
    component::Root,
    gpui::{Modifiers, TestAppContext, VisualTestContext},
    px, size, Context, IntoElement,
};
use std::time::{Duration, Instant};
use tokio::sync::oneshot::{self, error::TryRecvError};

struct QueueHarness;

impl Render for QueueHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn settle(cx: &mut VisualTestContext) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut previous = None;
    let mut samples = 0;
    loop {
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        let bounds = cx.debug_bounds("dialog-cancel");
        if bounds.is_some() && bounds == previous {
            samples += 1;
            if samples == 3 {
                return;
            }
        } else {
            samples = 0;
        }
        assert!(
            Instant::now() < deadline,
            "queue probe cancel button not stable"
        );
        previous = bounds;
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[gpui_kit::gpui::test]
fn cancelling_unknown_host_shows_the_next_changed_host_dialog(cx: &mut TestAppContext) {
    crate::session::abort_window(WINDOW_ID);
    cx.update(gpui_kit::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| QueueHarness);
        Root::new(view, window, cx)
    });
    let (first_tx, mut first_rx) = oneshot::channel();
    let (next_tx, mut next_rx) = oneshot::channel();
    cx.simulate_resize(size(px(860.), px(620.)));
    cx.update(|window, cx| {
        for (host, changed, tx) in [
            ("192.0.2.70", false, first_tx),
            ("198.51.100.70", true, next_tx),
        ] {
            show_host_key(
                host.into(),
                22,
                "ssh-ed25519".into(),
                "SHA256:synthetic-queue-probe".into(),
                changed,
                HostKeyResponder::new(tx),
                window,
                cx,
            );
        }
    });
    settle(cx);
    assert!(cx.debug_bounds("host-key-unknown").is_some());
    assert!(cx.debug_bounds("host-key-changed").is_none());
    let cancel = cx.debug_bounds("dialog-cancel").unwrap();
    cx.simulate_click(cancel.center(), Modifiers::default());
    settle(cx);
    assert_eq!(first_rx.try_recv(), Ok(false));
    assert_eq!(next_rx.try_recv(), Err(TryRecvError::Empty));
    let next = host_key_prompt(WINDOW_ID).expect("second prompt remains pending");
    assert_eq!(next.host, "198.51.100.70");
    assert!(next.changed);
    assert!(
        cx.debug_bounds("host-key-changed").is_some()
            && cx.debug_bounds("host-key-unknown").is_none(),
        "queued changed-host dialog must replace dismissed unknown-host dialog"
    );
    let cancel = cx.debug_bounds("dialog-cancel").unwrap();
    cx.simulate_click(cancel.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(next_rx.try_recv(), Ok(false));
    assert!(host_key_prompt(WINDOW_ID).is_none());
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
}
