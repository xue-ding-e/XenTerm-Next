//! The piece every modal in this shell needs, and why it is not optional.
//!
//! `Root::open_dialog` builds a `Dialog`, and a `Dialog` renders `button_props`'s
//! *callbacks* but not its *buttons*: the button text, variant and `show_cancel` fields
//! exist for `AlertDialog`, which draws them itself. A plain `Dialog` shows whatever
//! `.footer(...)` it was given, and nothing at all if it was given none.
//!
//! That is a quiet failure. Enter and Escape still dispatch Confirm and Cancel, so the
//! dialog answers to the keyboard and looks merely sparse to anyone who tries that
//! first — while a user who reaches for the mouse has no way out of it. The auth prompts
//! are the worst case: a host-key confirmation parks the connection until it is
//! answered, so "no buttons" reads as a connection that hangs.
//!
//! So every dialog built here ends with [`answer_footer`], and the view that mounts the
//! layer calls [`follow_root`].

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants, ButtonVariant},
        dialog::{DialogAction, DialogButtonProps, DialogClose, DialogFooter},
        h_flex,
        input::{Input, InputState},
        v_flex, Root, Sizable as _, ActiveTheme as _,
    },
    div,
    prelude::*,
    px, App, Context, Entity, IntoElement, SharedString, Subscription, Window,
};

/// Extend the toolkit's dialog keys with the native macOS cancel shortcut.
/// Dispatch its existing Cancel action so focused inputs and nested dropdowns
/// keep the same dismissal order as Escape; never intercept keys globally.
pub(crate) fn init(cx: &mut App) {
    if cfg!(target_os = "macos") {
        bind_macos_cancel(cx);
    }
}

// Shared with event-level tests so the macOS key path is exercised on CI too.
pub(super) fn bind_macos_cancel(cx: &mut App) {
    cx.bind_keys([gpui_kit::KeyBinding::new(
        "cmd-.",
        gpui_kit::component::dialog::Cancel,
        Some("Dialog"),
    )]);
}

/// Keep `view` repainting whenever the window's [`Root`] changes.
///
/// `Root` owns the queue of open dialogs and sheets, and it draws none of them: the view
/// that calls `Root::render_dialog_layer` is the one that puts them on screen. `Root`
/// notifies *itself* when that queue changes, and a notification does not reach its
/// children — so a view that mounts the layer without this renders whatever queue it saw
/// the last time it happened to repaint. That fails quietly in both directions: a dialog
/// opened from another view appears a frame late or not at all, and one closed by its own
/// Cancel button stays on screen.
pub(crate) fn follow_root<T: Render + 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
) -> Option<Subscription> {
    let root = window.root::<Root>().flatten()?;
    Some(cx.observe(&root, |_, _, cx| cx.notify()))
}

/// A dialog's action row: a cancel that closes it and a confirm that answers it.
///
/// The confirm dispatches the dialog's own `Confirm` action rather than carrying a
/// handler, so it runs exactly the `on_ok` the dialog was built with — including a
/// handler that refuses and keeps the dialog open. Two paths into one callback is the
/// point: a button that did its own thing could disagree with the Enter key.
pub(crate) fn answer_footer(
    confirm: SharedString,
    confirm_is_danger: bool,
    cancel: SharedString,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_end()
        .child(
            DialogFooter::new()
                .child(
                    DialogClose::new().trigger(move |button| button.label(cancel.clone()).small()),
                )
                .child(
                    DialogAction::new().child(
                        Button::new("dialog-confirm")
                            .label(confirm)
                            .small()
                            .when(confirm_is_danger, |this| this.danger()),
                    ),
                ),
        )
        .into_any_element()
}

/// Ask for one line of text, and hand it to `on_ok` when it is confirmed.
///
/// One field and two buttons, which is what every prompt in this shell needs: a new
/// folder's name, a new file's name, a mode to change a file to. The dialog's own
/// Confirm runs `on_ok` and `on_ok` decides whether to close it — returning `true` keeps
/// it open, which is what a prompt wants when the text is refused.
///
/// `on_ok` is handed the text rather than the field, so a caller cannot keep an entity
/// alive past the dialog that owns it.
pub(crate) fn prompt<T: Render + 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    title: SharedString,
    placeholder: SharedString,
    confirm: SharedString,
    on_ok: impl Fn(String, &mut Window, &mut App) -> bool + 'static,
) {
    let input: Entity<InputState> =
        cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.clone()));
    let cancel = crate::i18n::t("取消", "Cancel");
    // The handler is shared rather than moved: the dialog's builder is `Fn` and runs once
    // per frame, so a captured `impl Fn` cannot be moved out of it — only cloned.
    let on_ok = std::rc::Rc::new(on_ok);
    Root::update(window, cx, move |root, window, cx| {
        root.open_dialog(
            move |dialog, _window, cx| {
                dialog
                    .title(title.clone())
                    .button_props(
                        DialogButtonProps::default()
                            .on_ok({
                                let field = input.clone();
                                let on_ok = on_ok.clone();
                                move |_, window, cx| {
                                    let text = field.read(cx).value().to_string();
                                    on_ok(text, window, cx)
                                }
                            })
                            // Cancel answers the dialog and lets it close, like the button
                            // in the footer: two paths, one answer.
                            .on_cancel(|_, _, _| true),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .child(div().w_full().child(Input::new(&input))),
                    )
                    .footer(answer_footer(confirm.clone(), false, cancel.into()))
            },
            window,
            cx,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::{Focusable as _, TestAppContext};

    /// A view whose only job is one text field, so a test can ask the one question this
    /// project could not answer from outside: does typing reach a focused input at all?
    ///
    /// The question matters because the shell has three flows whose verification depends
    /// on it — the session editor's save, the settings paste box, and the tunnel panel's
    /// start — and every attempt to drive them through synthetic Windows messages failed
    /// while clicks worked. A test owns the window, so there is nothing between the
    /// keystroke and the focus handle.
    struct FieldHarness {
        field: Entity<InputState>,
    }

    impl Render for FieldHarness {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().child(Input::new(&self.field))
        }
    }

    #[gpui_kit::gpui::test]
    fn typing_reaches_a_focused_input(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let field = cx.new(|cx| InputState::new(window, cx));
            // Focused through the API rather than by a click: this test is about the
            // keystrokes, not about hit testing.
            window.focus(&field.read(cx).focus_handle(cx), cx);
            FieldHarness { field }
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        // `simulate_input` is the text path: `simulate_event(KeyDownEvent { .. })` delivers
        // a *keystroke*, which an input reads for Enter and the arrow keys but not for
        // text — the first version of this test typed five keys and got an empty field.
        cx.simulate_input("hello");

        let value = view.read_with(cx, |harness, cx| harness.field.read(cx).value().to_string());
        assert_eq!(
            value, "hello",
            "a focused input takes what is typed into it"
        );
    }
}

/// The risky-command approval dialog: centered, built to be read.
///
/// The body is structured — the session and the command each under their own
/// label, the command in the mono family on its own block, the risk reasons
/// as a bulleted list — because the whole point of asking is that the human
/// can see *what* they are approving and *why* it was flagged. The dialog
/// centers itself in the window: `margin_top` is computed from the viewport
/// instead of the toolkit's fixed top-tenth, and an explicit width is what
/// makes the toolkit's own horizontal centering arithmetic land.
///
/// `deny_label` is re-evaluated every frame — it carries the countdown. Both
/// answers close the dialog; the caller resolves the request file either way.
pub(crate) fn approval_dialog<T: Render + 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    request: crate::automation::approval::ApprovalRequest,
    reasons: Vec<String>,
    deny_label: impl Fn() -> SharedString + 'static,
    on_approve: impl Fn(&mut Window, &mut App) -> bool + 'static,
    on_deny: impl Fn(&mut Window, &mut App) -> bool + 'static,
) {
    let on_approve = std::rc::Rc::new(on_approve);
    let on_deny = std::rc::Rc::new(on_deny);
    let title = crate::i18n::t("风险命令审批", "Risky command approval");
    Root::update(window, cx, move |root, window, cx| {
        root.open_dialog(
            move |dialog, window, cx| {
                // Centered: a fixed width for the horizontal arithmetic, and
                // a top margin that puts the card's middle at the window's.
                let viewport_h = f32::from(window.viewport_size().height);
                let margin_top = (viewport_h * 0.26).max(60.0);
                let reasons_list = reasons
                    .iter()
                    .map(|reason| {
                        div()
                            .child(SharedString::from(format!("• {reason}")))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>();
                let seconds_left = crate::automation::approval::seconds_left(&request);
                dialog
                    .title(title.clone())
                    .width(px(560.))
                    .margin_top(margin_top)
                    .button_props(
                        DialogButtonProps::default()
                            .ok_text(crate::i18n::t("批准", "Approve"))
                            .ok_variant(ButtonVariant::Danger)
                            // The two callbacks' bools mean opposite things —
                            // `on_ok`: true keeps the dialog open; `on_cancel`:
                            // true closes it. Both answers here close it, so
                            // ok returns false and cancel returns true.
                            .on_ok({
                                let on_approve = on_approve.clone();
                                move |_, window, cx| {
                                    on_approve(window, cx);
                                    false
                                }
                            })
                            .on_cancel({
                                let on_deny = on_deny.clone();
                                move |_, window, cx| {
                                    on_deny(window, cx);
                                    true
                                }
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_2p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .w(px(56.))
                                            .flex_shrink_0()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(crate::i18n::t("会话", "Session")),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .child(request.session.clone()),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(crate::i18n::t("命令", "Command")),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .rounded_sm()
                                            .bg(cx.theme().muted)
                                            .px_2()
                                            .py_1p5()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_sm()
                                            .child(request.command.clone()),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(crate::i18n::t(
                                                "风险原因",
                                                "Why this was flagged",
                                            )),
                                    )
                                    .children(reasons_list),
                            ),
                    )
                    .footer(answer_footer(
                        crate::i18n::t("批准", "Approve").into(),
                        true,
                        SharedString::from(if seconds_left > 0 {
                            format!(
                                "{} ({}s)",
                                crate::i18n::t("拒绝", "Deny"),
                                seconds_left
                            )
                        } else {
                            crate::i18n::t("拒绝", "Deny").to_string()
                        }),
                    ))
                    .on_close({
                        // The dialog's own close paths — the footer's deny
                        // button, the ×, Escape — all mean the same thing:
                        // no. `on_cancel` above carries the callback; this
                        // hook only guarantees a close that bypasses it still
                        // ends in a denial.
                        let on_deny = on_deny.clone();
                        move |_, window, cx| {
                            on_deny(window, cx);
                        }
                    })
            },
            window,
            cx,
        );
    });
}
