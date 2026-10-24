//! Real host-key dialog events, using documentation-only addresses and synthetic keys.
//! No SSH connection, profile, keyring, known_hosts file or external clipboard is used.

use super::*;
use gpui_kit::{
    component::Root,
    gpui::{KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, TestAppContext, VisualTestContext},
    px, size, Context, IntoElement,
};
use std::time::{Duration, Instant};
use tokio::sync::oneshot::{self, error::TryRecvError};

const FINGERPRINT: &str = "SHA256:AbCdEf0123456789+/AbCdEf0123456789+/AbCdEf0123";
const KEY_TYPE: &str = "ssh-ed25519";
const PORT: u16 = 2222;

struct DialogHarness;

impl Render for DialogHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn fixture(cx: &mut TestAppContext) -> &mut VisualTestContext {
    crate::session::abort_window(WINDOW_ID);
    cx.update(gpui_kit::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| DialogHarness);
        Root::new(view, window, cx)
    });
    cx.simulate_resize(size(px(860.), px(620.)));
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("untouched".into()));
    });
    cx
}

fn open_prompt(
    cx: &mut VisualTestContext,
    host: &str,
    fingerprint: &str,
    changed: bool,
) -> oneshot::Receiver<bool> {
    let (tx, rx) = oneshot::channel();
    cx.update(|window, cx| {
        show_host_key(
            host.into(),
            PORT,
            KEY_TYPE.into(),
            fingerprint.into(),
            changed,
            HostKeyResponder::new(tx),
            window,
            cx,
        );
    });
    rx
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn settle_dialog(cx: &mut VisualTestContext) {
    // Dialog entrance animation uses real Instant, not the test executor clock.
    // Wait for observed geometry rather than clicking a fixed guessed position.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut previous = None;
    let mut stable_samples = 0;
    loop {
        draw(cx);
        let bounds = cx
            .debug_bounds("host-key-copy-fingerprint")
            .zip(cx.debug_bounds("host-key-copy-details"))
            .zip(cx.debug_bounds("dialog-cancel"))
            .zip(cx.debug_bounds("dialog-confirm"));
        if bounds.is_some() && bounds == previous {
            stable_samples += 1;
            if stable_samples == 3 {
                return;
            }
        } else {
            stable_samples = 0;
        }
        assert!(
            Instant::now() < deadline,
            "host-key controls did not stabilize"
        );
        previous = bounds;
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    settle_dialog(cx);
    let bounds = cx.debug_bounds(selector).expect("visible dialog control");
    cx.simulate_click(bounds.center(), Modifiers::default());
    draw(cx);
}

fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

fn press_key(cx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).unwrap();
    cx.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke });
    draw(cx);
}

fn assert_pending(cx: &mut VisualTestContext, rx: &mut oneshot::Receiver<bool>) {
    assert_eq!(
        rx.try_recv(),
        Err(TryRecvError::Empty),
        "copy must not answer trust"
    );
    assert!(
        host_key_prompt(WINDOW_ID).is_some(),
        "the queue must remain pending"
    );
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
}

fn copy_and_cancel(cx: &mut TestAppContext, host: &str, changed: bool) {
    let cx = fixture(cx);
    let mut rx = open_prompt(cx, host, FINGERPRINT, changed);
    let prompt = host_key_prompt(WINDOW_ID).unwrap();
    assert_eq!(prompt.changed, changed);
    assert_eq!(clipboard(cx).as_deref(), Some("untouched"));
    let details = format!("{host}:{PORT}  ({KEY_TYPE})\n{FINGERPRINT}");

    // Repeated actions and switching targets must always copy exactly the chosen
    // value, never labels, a truncated rendering, or a trust decision.
    for (selector, expected) in [
        ("host-key-copy-fingerprint", FINGERPRINT),
        ("host-key-copy-fingerprint", FINGERPRINT),
        ("host-key-copy-details", details.as_str()),
        ("host-key-copy-fingerprint", FINGERPRINT),
    ] {
        click(cx, selector);
        assert_eq!(clipboard(cx).as_deref(), Some(expected));
        let feedback = if selector == "host-key-copy-fingerprint" {
            "host-key-fingerprint-copied"
        } else {
            "host-key-details-copied"
        };
        assert!(
            cx.debug_bounds(feedback).is_some(),
            "copy feedback must repaint"
        );
        assert_pending(cx, &mut rx);
        assert_eq!(host_key_prompt(WINDOW_ID).unwrap(), prompt);
    }

    click(cx, "dialog-cancel");
    assert_eq!(rx.try_recv(), Ok(false));
    assert!(host_key_prompt(WINDOW_ID).is_none());
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));

    // Neither copying nor cancelling may leave an acceptance cached for reconnect.
    let mut again = open_prompt(cx, host, FINGERPRINT, changed);
    assert_pending(cx, &mut again);
    settle_dialog(cx);
    assert!(cx.debug_bounds("host-key-copy-ready").is_some());
    click(cx, "dialog-cancel");
    assert_eq!(again.try_recv(), Ok(false));
}

#[gpui_kit::gpui::test]
fn unknown_host_copy_is_exact_repeatable_and_cancel_still_rejects(cx: &mut TestAppContext) {
    copy_and_cancel(cx, "192.0.2.10", false);
}

#[gpui_kit::gpui::test]
fn changed_host_copy_is_exact_repeatable_and_cancel_still_rejects(cx: &mut TestAppContext) {
    copy_and_cancel(cx, "192.0.2.20", true);
}

#[gpui_kit::gpui::test]
fn copying_keeps_escape_available_and_does_not_answer_merged_responders(cx: &mut TestAppContext) {
    let cx = fixture(cx);
    let mut first = open_prompt(cx, "192.0.2.30", FINGERPRINT, false);
    let mut merged = open_prompt(cx, "192.0.2.30", FINGERPRINT, false);
    click(cx, "host-key-copy-fingerprint");
    assert_eq!(clipboard(cx).as_deref(), Some(FINGERPRINT));
    assert_pending(cx, &mut first);
    assert_pending(cx, &mut merged);
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert_eq!(first.try_recv(), Ok(false));
    assert_eq!(merged.try_recv(), Ok(false));
    assert!(host_key_prompt(WINDOW_ID).is_none());
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
}

#[gpui_kit::gpui::test]
fn copying_only_accepts_after_the_explicit_trust_button(cx: &mut TestAppContext) {
    let cx = fixture(cx);
    // A changed-key acceptance is deliberately not cached, keeping test retries
    // independent without resetting or weakening the application's trust memory.
    let mut rx = open_prompt(cx, "192.0.2.40", FINGERPRINT, true);
    click(cx, "host-key-copy-details");
    assert_pending(cx, &mut rx);
    click(cx, "dialog-confirm");
    assert_eq!(rx.try_recv(), Ok(true));
    assert!(host_key_prompt(WINDOW_ID).is_none());
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
}

#[gpui_kit::gpui::test]
fn enter_on_a_focused_copy_button_copies_without_trusting(cx: &mut TestAppContext) {
    let cx = fixture(cx);
    let mut rx = open_prompt(cx, "192.0.2.60", FINGERPRINT, false);
    settle_dialog(cx);
    // The two copy controls are the first tab stops in the real dialog body.
    // Enter must activate the focused button, not bubble into dialog Confirm.
    cx.update(|window, cx| window.focus_next(cx));
    draw(cx);
    press_key(cx, "enter");
    assert_eq!(clipboard(cx).as_deref(), Some(FINGERPRINT));
    assert_pending(cx, &mut rx);
    cx.update(|window, cx| window.focus_next(cx));
    draw(cx);
    press_key(cx, "enter");
    assert_eq!(
        clipboard(cx),
        Some(format!("192.0.2.60:{PORT}  ({KEY_TYPE})\n{FINGERPRINT}"))
    );
    assert_pending(cx, &mut rx);
    // Space uses the toolkit's native key-up click path rather than Confirm.
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("space sentinel".into()));
    });
    press_key(cx, "space");
    assert_eq!(
        clipboard(cx),
        Some(format!("192.0.2.60:{PORT}  ({KEY_TYPE})\n{FINGERPRINT}"))
    );
    assert_pending(cx, &mut rx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert_eq!(rx.try_recv(), Ok(false));
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
}

#[gpui_kit::gpui::test]
fn queued_host_copies_its_own_key_after_cancelling_the_previous_prompt(cx: &mut TestAppContext) {
    let cx = fixture(cx);
    let mut first = open_prompt(cx, "192.0.2.50", FINGERPRINT, false);
    let second_fingerprint = "SHA256:0123456789AbCdEf+/0123456789AbCdEf+/0123456789";
    let mut second = open_prompt(cx, "198.51.100.50", second_fingerprint, true);
    click(cx, "host-key-copy-fingerprint");
    assert_eq!(clipboard(cx).as_deref(), Some(FINGERPRINT));
    assert_pending(cx, &mut first);
    assert_eq!(second.try_recv(), Err(TryRecvError::Empty));
    click(cx, "dialog-cancel");
    assert_eq!(first.try_recv(), Ok(false));

    click(cx, "host-key-copy-fingerprint");
    assert_eq!(clipboard(cx).as_deref(), Some(second_fingerprint));
    assert_pending(cx, &mut second);
    click(cx, "host-key-copy-details");
    assert_eq!(
        clipboard(cx),
        Some(format!(
            "198.51.100.50:{PORT}  ({KEY_TYPE})\n{second_fingerprint}"
        ))
    );
    assert_pending(cx, &mut second);
    click(cx, "dialog-cancel");
    assert_eq!(second.try_recv(), Ok(false));
    assert!(host_key_prompt(WINDOW_ID).is_none());
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
}
