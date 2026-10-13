//! The piece every modal in this shell needs, and why it is not optional.
//!
//! `WindowExt::open_dialog` builds a `Dialog`, and a `Dialog` renders `button_props`'s
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
//! So every dialog built here ends with [`answer_footer`]. GPUI Kit 0.7 Root
//! hosts the overlay layers itself; application views neither mount those layers
//! nor subscribe to Root just to repaint them.

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        dialog::{DialogAction, DialogButtonProps, DialogClose, DialogFooter},
        h_flex,
        input::{Input, InputState},
        v_flex, ActiveTheme as _, WindowExt as _, Sizable as _,
    },
    div,
    prelude::*,
    App, Context, Entity, IntoElement, SharedString, Window,
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

/// What a prompt's confirm decided: take the text and close, or keep the
/// dialog open and say why. The reason is part of the refusal — a prompt that
/// silently refused (as these all once did) looks like a button that did
/// nothing, and the user's next move is to click it again.
pub(crate) enum PromptAnswer {
    Accept,
    Refuse(SharedString),
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
///
/// The field takes focus when the dialog opens — a one-line prompt is answered
/// with the keyboard or it is answered twice — and a refusal shows its reason
/// under the field rather than doing nothing.
pub(crate) fn prompt<T: Render + 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    title: SharedString,
    placeholder: SharedString,
    confirm: SharedString,
    on_ok: impl Fn(String, &mut Window, &mut App) -> PromptAnswer + 'static,
) {
    let input: Entity<InputState> =
        cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.clone()));
    let cancel = crate::i18n::t("取消", "Cancel");
    // The refusal's explanation, shared between the confirm handler (writer)
    // and the dialog body (reader) because the builder re-runs each frame.
    let refusal: std::rc::Rc<std::cell::RefCell<Option<SharedString>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    // The handler is shared rather than moved: the dialog's builder is `Fn` and runs once
    // per frame, so a captured `impl Fn` cannot be moved out of it — only cloned.
    let on_ok = std::rc::Rc::new(on_ok);
    let input_for_focus = input.clone();
    window.open_dialog(cx, move |dialog, _window, cx| {
        let theme = cx.theme();
        let reason = refusal.borrow().clone();
        dialog
            .title(title.clone())
            .button_props(
                DialogButtonProps::default()
                    .on_ok({
                        let field = input.clone();
                        let on_ok = on_ok.clone();
                        let refusal = refusal.clone();
                        move |_, window, cx| {
                            let text = field.read(cx).value().to_string();
                            match on_ok(text, window, cx) {
                                PromptAnswer::Accept => {
                                    *refusal.borrow_mut() = None;
                                    true
                                }
                                PromptAnswer::Refuse(reason) => {
                                    *refusal.borrow_mut() = Some(reason);
                                    // The refusal line is read by the builder on
                                    // its next run; the field's own repaint is
                                    // what schedules that run, there being no
                                    // shell entity here to notify.
                                    field.update(cx, |_, cx| cx.notify());
                                    false
                                }
                            }
                        }
                    })
                    // Cancel answers the dialog and lets it close, like the button
                    // in the footer: two paths, one answer.
                    .on_cancel(|_, _, _| true),
            )
            .child(
                v_flex()
                    .w_full()
                    .child(div().w_full().child(Input::new(&input)))
                    .when_some(reason, |this, reason| {
                        this.child(
                            div()
                                .w_full()
                                .pt_1()
                                .text_xs()
                                .text_color(theme.danger)
                                .child(reason),
                        )
                    }),
            )
            .footer(answer_footer(confirm.clone(), false, cancel.into()))
    });
    // The same beat the quick-connect palette opens on: the layer mounts from
    // Root's queue, and focus set after `open_dialog` lands once it has.
    input_for_focus.update(cx, |input, cx| input.focus(window, cx));
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
