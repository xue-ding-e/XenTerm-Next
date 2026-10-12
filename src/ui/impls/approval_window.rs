//! A single approval window using the same GPUI dialog as the main app.
//! Opening it never restores tabs, connects SSH sessions, or starts plugins.

use std::time::Duration;

use anyhow::{Context as _, Result};
use gpui_kit::{
    component::{ActiveTheme as _, Root, Theme, ThemeMode},
    div,
    prelude::*,
    px, size, Context, Subscription, Task, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::automation::approval::{self, ApprovalRequest};

pub(crate) fn run(id: String) -> Result<()> {
    let request = approval::request_state(&id).context("approval request is unavailable")?;
    anyhow::ensure!(
        request.status == "pending" && !approval::past_deadline(&request),
        "approval request has already finished"
    );
    let store = crate::config::ConfigStore::load()?;
    let theme = store.theme_pref().to_string();
    crate::i18n::set_language(&store.language());
    let (failed, failures) = std::sync::mpsc::channel();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            gpui_kit::component::set_locale(if crate::i18n::is_en() { "en" } else { "zh-CN" });
            super::dialogs::init(cx);
            match theme.as_str() {
                "dark" => Theme::change(ThemeMode::Dark, None, cx),
                "light" => Theme::change(ThemeMode::Light, None, cx),
                _ => Theme::sync_system_appearance(None, cx),
            }
            let fonts = vec![
                std::borrow::Cow::Borrowed(
                    include_bytes!("../../../assets/fonts/MiSans-Regular.ttf").as_slice(),
                ),
                std::borrow::Cow::Borrowed(
                    include_bytes!("../../../assets/fonts/MeatshellMono-Regular.ttf").as_slice(),
                ),
            ];
            let _ = cx.text_system().add_fonts(fonts);
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(660.), px(580.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some(
                        crate::i18n::t("XenTerm · 操作确认", "XenTerm · Confirmation").into(),
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let outcome = cx.open_window(options, move |window, cx| {
                let view = cx.new(|cx| ApprovalWindow::new(request, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            });
            if let Err(error) = outcome {
                let _ = failed.send(error.to_string());
                cx.quit();
            }
        });
    if let Ok(error) = failures.try_recv() {
        anyhow::bail!("could not open the approval window: {error}");
    }
    Ok(())
}

struct ApprovalWindow {
    request: ApprovalRequest,
    dialog: Option<super::dialogs::ApprovalDialogHandle>,
    root_watch: Option<Subscription>,
    _poll: Task<()>,
}

impl ApprovalWindow {
    fn new(request: ApprovalRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.defer_in(window, |view, window, cx| view.open_dialog(window, cx));
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let finished = this.update_in(cx, |view, _window, cx| {
                let current = approval::request_state(&view.request.id);
                let expired = current.as_ref().is_some_and(approval::past_deadline);
                if expired {
                    approval::resolve_timeout_in(&approval::queue_dir(), &view.request.id);
                }
                let finished = expired
                    || current.is_none()
                    || current.is_some_and(|request| request.status != "pending");
                if finished {
                    if let Some(dialog) = &view.dialog {
                        dialog.finished.set(true);
                    }
                    cx.quit();
                } else {
                    cx.notify();
                }
                finished
            });
            if !matches!(finished, Ok(false)) {
                break;
            }
        });
        Self {
            request,
            dialog: None,
            root_watch: None,
            _poll: poll,
        }
    }

    fn open_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let approve_id = self.request.id.clone();
        let deny_id = self.request.id.clone();
        self.dialog = Some(super::dialogs::approval_dialog(
            window,
            cx,
            self.request.clone(),
            self.request.reasons.clone(),
            move |_, _| approval::resolve_manual(&approve_id, true),
            move |_, _| approval::resolve_manual(&deny_id, false),
        ));
    }
}

impl Drop for ApprovalWindow {
    fn drop(&mut self) {
        // A title-bar close is a refusal. It cannot reverse a written result.
        approval::resolve_manual(&self.request.id, false);
    }
}

impl Render for ApprovalWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.root_watch.is_none() {
            self.root_watch = super::dialogs::follow_root(window, cx);
        }
        div()
            .size_full()
            .bg(cx.theme().background)
            .children(Root::render_dialog_layer(window, cx))
    }
}
