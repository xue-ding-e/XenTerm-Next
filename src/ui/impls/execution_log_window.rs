//! A read-only viewer for opt-in personal execution history.
//!
//! A separate window avoids competing with settings-page scrolling. Reading
//! history never enables logging, creates its directory, or changes records.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        tooltip::Tooltip,
        v_flex, ActiveTheme as _, Icon, Root, Sizable as _,
    },
    div,
    prelude::*,
    px, size, uniform_list, AnyElement, Context, Entity, FontWeight, IntoElement,
    ListSizingBehavior, Render, SharedString, Task, UniformListScrollHandle, WeakEntity, Window,
};

use crate::automation::execution_log::{log_dir, read_records, ExecutionRecord};
use gpui_kit::component::Disableable as _;

/// Re-read at a modest interval; the refresh button also updates immediately.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Coalesce refresh requests while the single background read is running.
#[derive(Default)]
struct RefreshState {
    running: bool,
    pending: bool,
}

impl RefreshState {
    fn request(&mut self) -> bool {
        if self.running {
            self.pending = true;
            false
        } else {
            self.running = true;
            true
        }
    }

    fn finish(&mut self) -> bool {
        if std::mem::take(&mut self.pending) {
            true
        } else {
            self.running = false;
            false
        }
    }
}

/// Optional personal execution history, in its own window.
pub(crate) struct ExecutionLogView {
    /// The records as of the last read, newest result first.
    records: Vec<ExecutionRecord>,
    /// The scroll position of the virtualized journal list.
    list_scroll: UniformListScrollHandle,
    refresh_state: RefreshState,
    _refresh: Option<Task<()>>,
    _poll: Task<()>,
}

impl ExecutionLogView {
    pub(crate) fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            if this.update(cx, |view, cx| view.refresh(cx)).is_err() {
                break;
            }
        });
        let mut view = Self {
            records: Vec::new(),
            list_scroll: UniformListScrollHandle::new(),
            refresh_state: RefreshState::default(),
            _refresh: None,
            _poll: poll,
        };
        view.refresh(cx);
        view
    }

    /// Re-read the journal now, for the toolbar's 刷新. Only the bounded disk
    /// read and JSON parsing run off-thread; entity updates stay on the UI thread.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.refresh_state.request() {
            return;
        }
        self._refresh = Some(cx.spawn(async move |this, cx| loop {
            let records = cx.background_executor().spawn(async { read_records() }).await;
            let again = this.update(cx, |view, cx| {
                if records != view.records {
                    view.records = records;
                    cx.notify();
                }
                view.refresh_state.finish()
            });
            // A closed view is never retained by this task. Serial reads also
            // prevent an older result from overwriting a later manual refresh.
            if !matches!(again, Ok(true)) {
                break;
            }
        }));
    }

    /// Open the journal folder in the OS file manager — the raw files are the
    /// export format, and the retention sweep works on whole day files there.
    fn open_folder(&self, _cx: &mut Context<Self>) {
        let dir = log_dir();
        if !dir.is_dir() {
            return;
        }
        let opener = if cfg!(target_os = "windows") {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener).arg(&dir).spawn();
    }
}

/// One journal row, without the view's `Context`: the virtualized list builds the
/// visible rows from inside a `&mut App` closure. Command cells copy their
/// full stored value without changing the record.
fn execution_row(record: &ExecutionRecord, theme: &gpui_kit::component::Theme) -> AnyElement {
    let (outcome, succeeded) = match record.outcome.as_str() {
        "succeeded" => (crate::i18n::t("完成", "Completed"), true),
        "timed_out" => (crate::i18n::t("超时", "Timed out"), false),
        "interrupted" => (crate::i18n::t("中断", "Interrupted"), false),
        _ => (crate::i18n::t("失败", "Failed"), false),
    };
    let by: SharedString = format!(
        "{} ms",
        record.finished_at_ms.saturating_sub(record.started_at_ms)
    )
    .into();
    let time = i64::try_from(record.started_at_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "—".to_string());
    let action = match record.operation.as_str() {
        "run_command" => crate::i18n::t("命令", "Command"),
        "list_remote_files" => crate::i18n::t("列目录", "List"),
        "read_remote_text_file" => crate::i18n::t("读文件", "Read"),
        "upload_file" => crate::i18n::t("上传", "Upload"),
        "download_file" => crate::i18n::t("下载", "Download"),
        _ => crate::i18n::t("操作", "Operation"),
    };
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
                .text_color(if succeeded {
                    theme.success
                } else {
                    theme.danger
                })
                .child(outcome),
        )
        .child(
            div()
                .id(SharedString::from(format!(
                    "execution-command-{}",
                    record.id
                )))
                .debug_selector({
                    let id = record.id.clone();
                    move || format!("execution-command-{id}").into()
                })
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_family(theme.mono_font_family.clone())
                .cursor_pointer()
                .tooltip(|window, cx| {
                    Tooltip::new(crate::i18n::t(
                        "点击复制命令或目标",
                        "Click to copy the command or target",
                    ))
                    .build(window, cx)
                })
                .on_click({
                    let command = record.command.clone();
                    move |_, _, cx| {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(command.clone()))
                    }
                })
                .child(SharedString::from(format!(
                    "{} · {action} · {}{}",
                    record.source.to_uppercase(),
                    record.command.replace(['\r', '\n'], " "),
                    if record.command_truncated { "…" } else { "" }
                ))),
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

impl Render for ExecutionLogView {
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
                    .child(crate::i18n::t("操作 / 目标", "Operation / target")),
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
                    .child(crate::i18n::t("耗时", "Duration")),
            );

        let mut body = v_flex().size_full().min_w_0().min_h_0().child(header);
        if self.records.is_empty() {
            body = body.child(
                v_flex()
                    .w_full()
                    .py_6()
                    .items_center()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(crate::i18n::t(
                        "暂无执行记录。默认关闭，可在设置 → MCP 与权限中开启。",
                        "No execution records. Enable history in Settings → MCP & permissions.",
                    )),
            );
        } else {
            // Virtual: the journal is "potentially hundreds of rows across many
            // days" by its own doc, and every one of them is the same height.
            let view = cx.entity();
            let scroll = self.list_scroll.clone();
            let rows: AnyElement = uniform_list(
                "execution-rows-list",
                self.records.len(),
                move |range, _window, cx| {
                    let execution_view = view.read(cx);
                    let theme = cx.theme();
                    range
                        .clone()
                        .filter_map(|index| {
                            let record = execution_view.records.get(index)?;
                            Some(execution_row(record, theme))
                        })
                        .collect()
                },
            )
            .flex_1()
            .min_h_0()
            .with_sizing_behavior(ListSizingBehavior::Auto)
            .track_scroll(&scroll)
            .into_any_element();
            body = body.child(rows);
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
                            .child(crate::i18n::t("执行记录", "Execution history")),
                    )
                    .child(
                        Button::new("execution-refresh")
                            .icon(Icon::new(gpui_kit::assets::IconName::RefreshCw))
                            .label(crate::i18n::t("刷新", "Refresh"))
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        Button::new("execution-open-folder")
                            .icon(Icon::new(gpui_kit::assets::IconName::FolderOpen))
                            .label(crate::i18n::t("打开文件夹", "Open folder"))
                            .disabled(!log_dir().is_dir())
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.open_folder(cx))),
                    ),
            )
            .child(
                div().px_3().pb_2().text_xs().text_color(theme.muted_foreground)
                    .child(crate::i18n::t(
                        "仅加载近期记录（最多 32 个文件、16 MiB）；完整历史可在文件夹中查看。",
                        "Recent history only (up to 32 files / 16 MiB); open the folder for complete history.",
                    )),
            )
            .child(
                div()
                    .id(SharedString::from("execution-rows"))
                    .debug_selector(|| "execution-rows-container".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .px_2()
                    .child(body),
            )
    }
}

/// The window handle pair the shell keeps for this viewer — mirror of the
/// detached-window pattern without the tab-following those windows need.
pub(crate) struct ExecutionLogWindowHandle {
    pub(crate) window: Option<gpui_kit::WindowHandle<Root>>,
    pub(crate) view: Option<WeakEntity<ExecutionLogView>>,
}

impl ExecutionLogWindowHandle {
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
                title: Some(
                    crate::i18n::t("XenTerm — 执行记录", "XenTerm — Execution history").into(),
                ),
                ..Default::default()
            }),
            ..Default::default()
        };

        let opened: Rc<std::cell::RefCell<Option<Entity<ExecutionLogView>>>> =
            Rc::new(std::cell::RefCell::new(None));
        let stash = opened.clone();
        match cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| ExecutionLogView::new(window, cx));
            *stash.borrow_mut() = Some(view.clone());
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            Ok(handle) => {
                self.window = Some(handle);
                self.view = opened.borrow().as_ref().map(|view| view.downgrade());
            }
            Err(error) => {
                tracing::warn!("could not open the execution history window: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::TestAppContext;

    #[test]
    fn refresh_requests_are_serial_and_coalesce_without_losing_manual_refresh() {
        let mut state = RefreshState::default();
        assert!(state.request());
        assert!(!state.request());
        assert!(!state.request());
        assert!(state.finish());
        assert!(state.running);
        assert!(!state.pending);
        assert!(!state.request());
        assert!(state.finish());
        assert!(!state.finish());
        assert!(!state.running);
        assert!(state.request());
    }

    #[gpui_kit::gpui::test]
    fn clicking_an_execution_copies_its_complete_command(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let command = "printf '第一行\\n第二行'";
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| ExecutionLogView {
                records: vec![ExecutionRecord {
                    id: "fixture".into(),
                    started_at_ms: 1,
                    finished_at_ms: 2,
                    operation: "run_command".into(),
                    source: "mcp".into(),
                    session_id: "fixture".into(),
                    session: "Loopback fixture".into(),
                    command: command.into(),
                    command_truncated: false,
                    outcome: "succeeded".into(),
                    exit_code: Some(0),
                }],
                list_scroll: UniformListScrollHandle::new(),
                refresh_state: RefreshState::default(),
                _refresh: None,
                _poll: cx.spawn(async move |_, _| {}),
            });
            Root::new(view, window, cx)
        });
        for _ in 0..3 {
            cx.update(|window, cx| {
                window.resize(size(px(860.), px(560.)));
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            cx.run_until_parked();
        }
        let body = cx.debug_bounds("execution-rows-container");
        let command_bounds = cx.debug_bounds("execution-command-fixture").expect(&format!("body: {body:?}"));
        cx.simulate_click(command_bounds.center(), gpui_kit::Modifiers::default());
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some(command.to_string())
        );
    }
}
