//! The approval audit journal, in a window of its own.
//!
//! A window rather than a settings section because the journal is a *record*
//! to browse — potentially hundreds of rows across many days — and a record
//! browser inside a form page would have the page scroll fighting the table.
//!
//! Read-only, deliberately. The journal's whole value is that a decision
//! cannot be edited after the fact; the only writes to it are the decisions
//! themselves and the retention sweep, and this window offers neither. The
//! folder behind it stays reachable from the toolbar for anyone who wants the
//! raw files.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex, v_flex, ActiveTheme as _, Icon, Root, Sizable as _,
    },
    div,
    prelude::*,
    px, size, AnyElement, Context, Entity, FontWeight, IntoElement, Render, SharedString, Task,
    WeakEntity, Window,
};

use crate::automation::approval::{queue_dir, AuditRecord};

/// How often the journal re-reads itself. Decisions are rare — seconds of
/// staleness cost nothing, and the toolbar's 刷新 answers anyone who cannot
/// wait.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// The approval audit journal, in its own window.
pub(crate) struct AuditLogView {
    /// The records as of the last read, newest decision first.
    records: Vec<AuditRecord>,
    _poll: Task<()>,
}

impl AuditLogView {
    pub(crate) fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let records = read_journal();
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let alive = this.update(cx, |view, cx| {
                let records = read_journal();
                if records != view.records {
                    view.records = records;
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        });
        Self {
            records,
            _poll: poll,
        }
    }

    /// Re-read the journal now, for the toolbar's 刷新.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.records = read_journal();
        cx.notify();
    }

    /// Open the journal folder in the OS file manager — the raw files are the
    /// export format, and the retention sweep works on whole day files there.
    fn open_folder(&self, _cx: &mut Context<Self>) {
        let dir = queue_dir().join("audit");
        let _ = std::fs::create_dir_all(&dir);
        let opener = if cfg!(target_os = "windows") {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener).arg(&dir).spawn();
    }

    fn row(&self, record: &AuditRecord, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let approved = record.outcome == "approved";
        let by: SharedString = match record.by.as_str() {
            "manual" => crate::i18n::t("人工", "Manual").into(),
            "auto-timeout" => crate::i18n::t("超时自动", "Timeout").into(),
            "auto-expired" => crate::i18n::t("过期自动", "Expired").into(),
            "auto-limit" => crate::i18n::t("队列满自动", "Queue full").into(),
            _ => SharedString::from(record.by.clone()),
        };
        let time = chrono::DateTime::<chrono::Local>::from(
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(record.decided_at_ms),
        )
        .format("%m-%d %H:%M:%S")
        .to_string();
        h_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .px_2()
            .py_1()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .w(px(110.))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(time)),
            )
            .child(
                div()
                    .w(px(64.))
                    .flex_shrink_0()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if approved {
                        theme.success
                    } else {
                        theme.danger
                    })
                    .child(if approved {
                        crate::i18n::t("批准", "Approved")
                    } else {
                        crate::i18n::t("拒绝", "Denied")
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_family(theme.mono_font_family.clone())
                    .child(record.command.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(record.session.clone()),
            )
            .child(
                div()
                    .w(px(80.))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(by),
            )
            .into_any_element()
    }
}

/// Read every day file in the journal, newest decision first. A line that
/// fails to parse is skipped — a truncated tail (the writer crashed
/// mid-line) must not empty the whole view.
fn read_journal() -> Vec<AuditRecord> {
    let dir = queue_dir().join("audit");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut records: Vec<AuditRecord> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .filter_map(|path| std::fs::read_to_string(&path).ok())
        .flat_map(|body| {
            body.lines()
                .filter_map(|line| serde_json::from_str::<AuditRecord>(line).ok())
                .collect::<Vec<_>>()
        })
        .collect();
    records.sort_by_key(|record| std::cmp::Reverse(record.decided_at_ms));
    records
}

impl Render for AuditLogView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let header = h_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .px_2()
            .py_1p5()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .w(px(110.))
                    .flex_shrink_0()
                    .child(crate::i18n::t("时间", "Time")),
            )
            .child(
                div()
                    .w(px(64.))
                    .flex_shrink_0()
                    .child(crate::i18n::t("结果", "Outcome")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(crate::i18n::t("命令", "Command")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(crate::i18n::t("会话", "Session")),
            )
            .child(
                div()
                    .w(px(80.))
                    .flex_shrink_0()
                    .child(crate::i18n::t("方式", "By")),
            );

        let mut body = v_flex().w_full().min_w_0().child(header);
        if self.records.is_empty() {
            body = body.child(
                v_flex()
                    .w_full()
                    .py_6()
                    .items_center()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(crate::i18n::t("还没有审批记录。", "No approval records yet.")),
            );
        } else {
            let rows: Vec<AnyElement> = self
                .records
                .iter()
                .map(|record| self.row(record, cx))
                .collect();
            body = body.children(rows);
        }

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::i18n::t("审批记录", "Approval audit")),
                    )
                    .child(
                        Button::new("audit-refresh")
                            .icon(Icon::new(gpui_kit::assets::IconName::RefreshCw))
                            .label(crate::i18n::t("刷新", "Refresh"))
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        Button::new("audit-open-folder")
                            .icon(Icon::new(gpui_kit::assets::IconName::FolderOpen))
                            .label(crate::i18n::t("打开文件夹", "Open folder"))
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.open_folder(cx))),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from("audit-rows"))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .child(body),
            )
    }
}

/// The window handle pair the shell keeps for this viewer — mirror of the
/// detached-window pattern without the tab-following those windows need.
pub(crate) struct AuditWindowHandle {
    pub(crate) window: Option<gpui_kit::WindowHandle<Root>>,
    pub(crate) view: Option<WeakEntity<AuditLogView>>,
}

impl AuditWindowHandle {
    pub(crate) fn new() -> Self {
        Self {
            window: None,
            view: None,
        }
    }

    /// Bring the window forward, or open it.
    pub(crate) fn show(&mut self, cx: &mut Context<super::shell::Shell>) {
        if let Some(handle) = self.window {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            self.window = None;
            self.view = None;
        }

        let options = gpui_kit::WindowOptions {
            window_bounds: Some(gpui_kit::WindowBounds::centered(
                size(px(860.), px(560.)),
                cx,
            )),
            window_min_size: Some(size(px(560.), px(320.))),
            titlebar: Some(gpui_kit::TitlebarOptions {
                title: Some("XenTerm — 审批记录".into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let opened: Rc<std::cell::RefCell<Option<Entity<AuditLogView>>>> =
            Rc::new(std::cell::RefCell::new(None));
        let stash = opened.clone();
        match cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| AuditLogView::new(window, cx));
            *stash.borrow_mut() = Some(view.clone());
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            Ok(handle) => {
                self.window = Some(handle);
                self.view = opened.borrow().as_ref().map(|view| view.downgrade());
            }
            Err(error) => {
                tracing::warn!("could not open the audit window: {error}");
            }
        }
    }
}
