//! The three connection prompts, as GPUI dialogs.
//!
//! Presentation only. Whether to ask, whether a prompt merges into one already on
//! screen, and what an answer means are `crate::session::prompt_queue`'s — shared with
//! the rest of the shell so one security policy covers every prompt. This module writes
//! the queue's current prompt into a dialog and turns a button press into a resolve.
//!
//! That split is the reason these dialogs are short. The interesting decisions about a
//! host key are not about a dialog: they are about remembering an accept but never a
//! reject (#152), merging the shell's prompt with its SFTP channel's so one decision
//! answers both, and answering everything queued when a window closes.

use gpui_kit::{
    component::{
        dialog::DialogButtonProps,
        input::{Input, InputState},
        v_flex, ActiveTheme as _, Root,
    },
    div,
    prelude::*,
    App, SharedString, Window,
};

use crate::session::protocol::{
    CredentialReply, CredentialResponder, HostKeyResponder, MfaResponder,
};
use crate::session::{
    credential_prompt, enqueue_credential, enqueue_host_key, enqueue_mfa, host_key_prompt,
    mfa_prompt, resolve_credential, resolve_host_key, resolve_mfa,
};

use super::session_state::WINDOW_ID;

/// Show the host-key confirmation for a session that has met an unknown host.
///
/// The common case on a first connection to any host, which is why it is the prompt
/// that makes the GPUI shell usable against real servers.
pub(crate) fn show_host_key(
    host: String,
    port: u16,
    key_type: String,
    fingerprint: String,
    changed: bool,
    responder: HostKeyResponder,
    window: &mut Window,
    cx: &mut App,
) {
    if !enqueue_host_key(
        WINDOW_ID,
        host,
        port,
        key_type,
        fingerprint,
        changed,
        responder,
    )
    .should_show()
    {
        // Answered from memory, or merged into a dialog already up for this host.
        return;
    }
    present_host_key(window, cx);
}

/// Open the dialog for the window's current host-key prompt.
fn present_host_key(window: &mut Window, cx: &mut App) {
    let Some(prompt) = host_key_prompt(WINDOW_ID) else {
        return;
    };

    // A changed key is the one case where confirming is dangerous, so the button is
    // not the default colour and says what it does.
    let prompt_changed = prompt.changed;

    let detail = prompt.detail.clone();
    let title = prompt.title.clone();
    let message = prompt.message.clone();
    let confirm_label = prompt.confirm_label.clone();
    Root::update(window, cx, move |root, window, cx| {
        root.open_dialog(
            move |dialog, _window, cx| {
                dialog
                    .title(SharedString::from(title.clone()))
                    // Only the callbacks: a `Dialog` renders these, not its buttons, so
                    // the visible ones are in the footer below. See `super::dialogs`.
                    .button_props(
                        DialogButtonProps::default()
                            .on_ok(move |_, window, cx| {
                                advance_host_key(window, cx, true);
                                true
                            })
                            .on_cancel(move |_, window, cx| {
                                advance_host_key(window, cx, false);
                                true
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(div().child(SharedString::from(message.clone())))
                            .child(
                                div()
                                    .p_2()
                                    .rounded_md()
                                    .bg(cx.theme().muted)
                                    // A fingerprint is compared character by character
                                    // against `ssh-keyscan`, so it wants the monospace
                                    // family rather than the UI one.
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_sm()
                                    .child(SharedString::from(detail.clone())),
                            ),
                    )
                    .footer(super::answer_footer(
                        SharedString::from(confirm_label.clone()),
                        prompt_changed,
                        crate::i18n::t("取消", "Cancel").into(),
                    ))
            },
            window,
            cx,
        );
    });
}

/// Answer the current host-key prompt and show the next one if there is one.
fn advance_host_key(window: &mut Window, cx: &mut App, accept: bool) {
    if resolve_host_key(WINDOW_ID, accept) {
        present_host_key(window, cx);
    }
}

/// Show the credential prompt for a session whose login details are missing.
pub(crate) fn show_credential(
    session_id: String,
    host: String,
    user: String,
    need_user: bool,
    need_password: bool,
    responder: CredentialResponder,
    window: &mut Window,
    cx: &mut App,
) {
    if !enqueue_credential(
        WINDOW_ID,
        session_id,
        host,
        user,
        need_user,
        need_password,
        responder,
    )
    .should_show()
    {
        return;
    }
    present_credential(window, cx);
}

/// Open the dialog for the window's current credential prompt.
///
/// Unlike the host-key prompt this one takes input, so the fields are created here and
/// read back on confirm. They are created fresh per prompt rather than reused, so a
/// password typed for one session cannot be read for the next.
fn present_credential(window: &mut Window, cx: &mut App) {
    let Some(prompt) = credential_prompt(WINDOW_ID) else {
        return;
    };

    let user_input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(crate::i18n::t("用户名", "Username"))
            .default_value(prompt.user.clone())
    });
    let password_input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(crate::i18n::t("密码", "Password"))
            .masked(true)
    });

    let need_user = prompt.need_user;
    let need_password = prompt.need_password;
    let host = prompt.host.clone();
    let user_for_read = user_input.clone();
    let password_for_read = password_input.clone();

    Root::update(window, cx, move |root, window, cx| {
        let title = SharedString::from(crate::i18n::t("需要登录凭据", "Credentials required"));
        let body = SharedString::from(format!(
            "{} {host}",
            crate::i18n::t("请输入以下主机的登录信息:", "Sign in to")
        ));
        let user_field = user_input.clone();
        let password_field = password_input.clone();
        root.open_dialog(
            move |dialog, _window, _cx| {
                // Cloned inside rather than moved: the build closure is `Fn`, so it may
                // be called more than once and cannot take the entities with it.
                let user_for_ok = user_for_read.clone();
                let password_for_ok = password_for_read.clone();
                dialog
                    .title(title.clone())
                    .button_props(
                        DialogButtonProps::default()
                            .on_ok(move |_, window, cx| {
                                let user = user_for_ok.read(cx).value().to_string();
                                // Read from the dialog's own state, which is why the
                                // state lives in this closure rather than in a global: a
                                // secret should be reachable by exactly the code about
                                // to use it.
                                let password = password_for_ok.read(cx).value().to_string();
                                let reply: CredentialReply = (user, password, false);
                                advance_credential(window, cx, Some(reply));
                                true
                            })
                            .on_cancel(move |_, window, cx| {
                                advance_credential(window, cx, None);
                                true
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(div().child(body.clone()))
                            .when(need_user, |this| this.child(Input::new(&user_field)))
                            .when(need_password, |this| {
                                this.child(Input::new(&password_field))
                            }),
                    )
                    // A `Dialog` renders its `button_props` callbacks and not its
                    // buttons, so without this the prompt has no visible way to answer
                    // it. See `super::dialogs`.
                    .footer(super::answer_footer(
                        crate::i18n::t("连接", "Connect").into(),
                        false,
                        crate::i18n::t("取消", "Cancel").into(),
                    ))
            },
            window,
            cx,
        );
    });
}

/// Answer the current credential prompt and show the next one if there is one.
fn advance_credential(window: &mut Window, cx: &mut App, reply: Option<CredentialReply>) {
    if resolve_credential(WINDOW_ID, reply) {
        present_credential(window, cx);
    }
}

/// Show the MFA prompt for a session the server is challenging.
pub(crate) fn show_mfa(
    session_id: String,
    host: String,
    prompt: String,
    echo: bool,
    responder: MfaResponder,
    window: &mut Window,
    cx: &mut App,
) {
    if !enqueue_mfa(WINDOW_ID, session_id, host, prompt, echo, responder).should_show() {
        return;
    }
    present_mfa(window, cx);
}

/// Open the dialog for the window's current MFA prompt.
fn present_mfa(window: &mut Window, cx: &mut App) {
    let Some(prompt) = mfa_prompt(WINDOW_ID) else {
        return;
    };

    let answer_input = cx.new(|cx| {
        let state =
            InputState::new(window, cx).placeholder(crate::i18n::t("验证码", "Verification code"));
        // `echo` is the server saying whether the answer should be visible. An OTP is
        // echoed by convention even though it is a secret, because the user has to read
        // it back; a password-shaped challenge is not.
        if prompt.echo {
            state
        } else {
            state.masked(true)
        }
    });

    let host_text = prompt.host.clone();
    let prompt_text = prompt.prompt.clone();
    let answer_for_read = answer_input.clone();

    Root::update(window, cx, move |root, window, cx| {
        let title = SharedString::from(crate::i18n::t("需要验证码", "Verification required"));
        let question = SharedString::from(format!("{host_text} — {prompt_text}"));
        let field = answer_input.clone();
        root.open_dialog(
            move |dialog, _window, _cx| {
                // Cloned inside because the build closure is `Fn` and may run again.
                let answer_for_ok = answer_for_read.clone();
                dialog
                    .title(title.clone())
                    .button_props(
                        DialogButtonProps::default()
                            .on_ok(move |_, window, cx| {
                                let answer = answer_for_ok.read(cx).value().to_string();
                                advance_mfa(window, cx, Some(answer));
                                true
                            })
                            .on_cancel(move |_, window, cx| {
                                advance_mfa(window, cx, None);
                                true
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(div().child(question.clone()))
                            .child(Input::new(&field)),
                    )
                    // See `super::dialogs`: a `Dialog` shows its footer, not its
                    // `button_props` buttons.
                    .footer(super::answer_footer(
                        crate::i18n::t("提交", "Submit").into(),
                        false,
                        crate::i18n::t("取消", "Cancel").into(),
                    ))
            },
            window,
            cx,
        );
    });
}

/// Answer the current MFA prompt and show the next one if there is one.
fn advance_mfa(window: &mut Window, cx: &mut App, answer: Option<String>) {
    if resolve_mfa(WINDOW_ID, answer) {
        present_mfa(window, cx);
    }
}
