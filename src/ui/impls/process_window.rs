//! The process monitor, in a window of its own.
//!
//! ## Why a window and not a panel
//!
//! The original detaches it, and the reason holds here: a process table is something you
//! keep open beside the terminal while you work in it. A panel inside the main window
//! would take width from the terminal every time you looked at what was eating the CPU.
//!
//! ## Where the rows come from
//!
//! `crate::resource::TabStatus` again — the same per-tab store the resource panel
//! projects. A session's process samples arrive on the tab's channel and are written
//! there by the terminal view that owns them, so this window only has to read. It polls
//! at 1 Hz for the same reason the resource panel does: nothing pushes to a sibling
//! window.
//!
//! ## What a click can do
//!
//! Two actions, matching the original: click a PID to copy it, right-click a row to
//! terminate it. Terminating root's or another user's process needs the administrator
//! password, and the confirmation asks for it — the rule about *when* it is needed is
//! [`crate::resource::process_needs_root`]'s, because a second answer to "may this
//! user signal this process" is a security bug and not a layout difference.
//!
//! The kill itself goes through the session handle the shell already holds, so nothing
//! here talks to SSH directly.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    component::{
        dialog::DialogButtonProps,
        h_flex,
        input::{Input, InputState},
        menu::{ContextMenuExt, PopupMenuItem},
        v_flex, ActiveTheme as _, Root,
    },
    div,
    prelude::*,
    px, relative, uniform_list, AnyElement, Context, Entity, IntoElement, ListSizingBehavior,
    Render, SharedString, Subscription, Task, UniformListScrollHandle, WeakEntity, Window,
};

use crate::config::Secret;
use crate::resource::{proc_rows, ProcRow};

use super::SessionState;

/// How often the table re-reads the shared sample.
///
/// Slower than the sampler that fills it, which is deliberate: a table that redrew on
/// every sample would be unreadable while the host is busy, and a click that lands on a
/// row that has moved is a click on the wrong process.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// What the window has to say about the last action, and how it should read.
#[derive(Clone, PartialEq)]
struct Status {
    text: SharedString,
    kind: StatusKind,
}

#[derive(Clone, Copy, PartialEq)]
enum StatusKind {
    Ok,
    Error,
}

/// The process table, in its own window.
pub(crate) struct ProcessWindowView {
    /// The window's session state: the table is a projection of its per-tab status and
    /// a termination goes through its session handle.
    state: SessionState,
    /// Which tab's processes are shown. The shell sets it when the window opens and
    /// again whenever the main window's active tab changes, because the process window
    /// follows the session the user is looking at.
    tab: Option<String>,
    /// The table as of the last poll.
    rows: Vec<ProcRow>,
    /// The last action's message.
    status: Option<Status>,
    /// Why the confirmation would not go through, shown inside the dialog.
    ///
    /// A cell rather than a plain field, because the dialog that shows it is built by
    /// `Root` *during* this view's render — and reading an entity that is already being
    /// updated panics. The message therefore has to be reachable without going through
    /// the view. Kept apart from `status` as well: the status line is behind the dialog
    /// and dimmed by it, so a refusal that lived only there would be invisible at the
    /// moment it matters.
    refusal: Rc<RefCell<Option<SharedString>>>,
    /// A termination the user asked for, waiting for a frame that has a window to open
    /// its confirmation in.
    ///
    /// Recorded rather than opened in place because the context menu's handler receives
    /// an `App`, which cannot open a dialog — the same reason `TerminalView` parks its
    /// auth prompts.
    pending: Rc<RefCell<Option<ProcRow>>>,
    /// Whether a termination is in flight. A second one is refused rather than queued:
    /// the answer to the first may already have changed the table it was aimed at.
    busy: bool,
    /// The table's own poll, kept alive for the window's life.
    _poll: Task<()>,
    /// Keeps this window repainting when the modal queue changes — a dialog closed by its
    /// own Cancel button is `Root`'s notification and nobody else's. See
    /// [`super::follow_root`].
    _root_subscription: Option<Subscription>,
    /// The scroll position of the virtualized process table.
    list_scroll: UniformListScrollHandle,
}

impl ProcessWindowView {
    pub(crate) fn new(
        state: SessionState,
        tab: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The poll. A task rather than a timer because it outlives each frame and
        // re-arms itself, and because a failing `update` is how it learns the window is
        // gone — the same shape the plugin manager's table uses.
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let Ok(fresh) = this.update(cx, |view, _| view.sample()) else {
                    // The window is gone, which is the normal end of this task.
                    break;
                };
                let alive = this.update(cx, |view, cx| {
                    if view.rows != fresh {
                        view.rows = fresh;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        let mut view = Self {
            state,
            tab,
            rows: Vec::new(),
            status: None,
            refusal: Rc::new(RefCell::new(None)),
            pending: Rc::new(RefCell::new(None)),
            busy: false,
            _poll: poll,
            _root_subscription: None,
            list_scroll: UniformListScrollHandle::new(),
        };
        view.rows = view.sample();
        view
    }

    /// The rows for the tab this window is following.
    ///
    /// Read from the shared store rather than kept: the sample belongs to the session,
    /// and a copy here would be a second version of it that ages differently.
    fn sample(&self) -> Vec<ProcRow> {
        let Some(tab) = self.tab.as_deref() else {
            return Vec::new();
        };
        let Ok(statuses) = self.state.statuses.lock() else {
            return Vec::new();
        };
        let Some(status) = statuses.get(tab) else {
            return Vec::new();
        };
        proc_rows(&status.procs, &status.user, tab)
    }

    /// Follow a different tab, which the shell asks for when the main window's active
    /// tab changes.
    pub(crate) fn set_tab(&mut self, tab: Option<String>, cx: &mut Context<Self>) {
        self.tab = tab;
        self.rows = self.sample();
        cx.notify();
    }

    /// The host shown in the title bar, so a window on a second monitor still says which
    /// machine it is about.
    pub(crate) fn host(&self) -> SharedString {
        let Some(tab) = self.tab.as_deref() else {
            return SharedString::default();
        };
        self.state
            .statuses
            .lock()
            .ok()
            .and_then(|statuses| statuses.get(tab).map(|status| status.host.clone()))
            .unwrap_or_default()
            .into()
    }

    /// Note what a row's PID click or a termination produced.
    fn note(&mut self, text: impl Into<SharedString>, kind: StatusKind) {
        self.status = Some(Status {
            text: text.into(),
            kind,
        });
    }

    /// Record why a confirmation will not go through, for the dialog to show.
    fn refuse(&mut self, why: &'static str) {
        *self.refusal.borrow_mut() = Some(why.into());
    }

    /// Copy a PID, and say so.
    fn copy_pid(&mut self, pid: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(pid.clone()));
        // The confirmation is the status line rather than a toast: it belongs in the
        // window the action came from, next to the PID that was copied.
        self.note(
            format!("{}: {pid}", crate::i18n::t("已复制 PID", "PID copied")),
            StatusKind::Ok,
        );
        cx.notify();
    }

    /// Ask for confirmation, then kill.
    ///
    /// The dialog is the library's, which is what the shell's other confirmations use: a
    /// hand-drawn modal would have to re-earn the focus handling, the escape key and the
    /// backdrop the dialog already has. What it does *not* do is disable its own button,
    /// so an empty password is answered by keeping the dialog open with the reason in it
    /// rather than by greying the button — the same guarantee, said in a sentence rather
    /// than in a shade of grey.
    fn confirm_terminate(&mut self, row: ProcRow, window: &mut Window, cx: &mut Context<Self>) {
        // A fresh confirmation starts with nothing refused; the message from a previous
        // one would otherwise be read as being about this process.
        *self.refusal.borrow_mut() = None;
        let needs_root = !row.own_process;
        let password: Option<Entity<InputState>> = needs_root.then(|| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder(crate::i18n::t(
                        "请输入 sudo 密码",
                        "Enter the sudo password",
                    ))
            })
        });

        let weak: WeakEntity<Self> = cx.entity().downgrade();
        let row_for_ok = row.clone();
        let hook = self.pending.clone();
        let refusal = self.refusal.clone();
        let password_for_ok = password.clone();
        let mono = cx.theme().mono_font_family.clone();

        let title: SharedString = crate::i18n::t("结束进程", "Terminate process").into();
        let explanation: SharedString = if row.own_process {
            crate::i18n::t(
                "确定要结束这个进程吗?未保存的数据可能会丢失。",
                "Terminate this process? It may lose unsaved data.",
            )
            .into()
        } else {
            crate::i18n::t(
                "该进程属于 root 或其他用户,需要输入管理员(sudo)密码才能结束。",
                "This process belongs to root or another user. Enter the administrator \
                 (sudo) password to continue.",
            )
            .into()
        };
        // The same identity the row shows, so the dialog cannot name a different process
        // from the one behind it.
        let identity: SharedString = format!("PID {}  ·  {}", row.pid, row.user).into();
        let command: SharedString = row.command.clone().into();

        Root::update(window, cx, move |root, window, cx| {
            root.open_dialog(
                move |dialog, _window, cx| {
                    let weak = weak.clone();
                    let hook_for_ok = hook.clone();
                    let hook_for_cancel = hook.clone();
                    let password_for_ok = password_for_ok.clone();
                    let password_field = password_for_ok.clone();
                    let row = row_for_ok.clone();

                    let mut body = v_flex()
                        .gap_2()
                        .child(div().child(explanation.clone()))
                        .child(
                            div()
                                .font_family(mono.clone())
                                .text_sm()
                                .child(identity.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(command.clone()),
                        );
                    if let Some(field) = password_field.clone() {
                        body = body.child(Input::new(&field));
                    }
                    // Why the last attempt did not go through, read from the cell the
                    // refusal was written to — not from this view, because the dialog is
                    // built during the view's own render and reading an entity that is
                    // already being updated panics. The dialog is rebuilt every frame, so
                    // this appears without reopening it.
                    if let Some(refusal) = refusal.borrow().clone() {
                        body = body
                            .child(div().text_xs().text_color(cx.theme().danger).child(refusal));
                    }

                    dialog
                        .title(title.clone())
                        // Only the callbacks: the visible buttons are the footer below,
                        // and the text/variant fields here are read by `AlertDialog`
                        // alone.
                        .button_props(
                            DialogButtonProps::default()
                                .on_ok(move |_, _window, cx| {
                                    let typed = password_for_ok
                                        .as_ref()
                                        .map(|input| input.read(cx).value().to_string())
                                        .unwrap_or_default();
                                    if needs_root && typed.is_empty() {
                                        // Left open, with the reason in it: a dialog that
                                        // closed on an empty password would look like a
                                        // kill that happened.
                                        if let Some(view) = weak.upgrade() {
                                            view.update(cx, |view, cx| {
                                                view.refuse(crate::i18n::t(
                                                    "请输入管理员(sudo)密码",
                                                    "Enter the administrator (sudo) password",
                                                ));
                                                cx.notify();
                                            });
                                        }
                                        return false;
                                    }
                                    // Taken before the kill, so a second confirmation
                                    // cannot be opened over the first one's answer.
                                    let _ = hook_for_ok.borrow_mut().take();
                                    if let Some(view) = weak.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.terminate(row.clone(), typed, cx)
                                        });
                                    }
                                    true
                                })
                                .on_cancel(move |_, _, _| {
                                    let _ = hook_for_cancel.borrow_mut().take();
                                    true
                                }),
                        )
                        .child(body)
                        // The buttons are the dialog's own `on_ok`/`on_cancel` in visible
                        // form: see `super::dialogs` for why a `Dialog` needs a footer to
                        // be answerable with the mouse at all.
                        .footer(super::answer_footer(
                            crate::i18n::t("结束进程", "Terminate").into(),
                            true,
                            crate::i18n::t("取消", "Cancel").into(),
                        ))
                },
                window,
                cx,
            );
        });
    }

    /// Signal the process, and report what came back.
    ///
    /// The password only reaches SSH when the rule says it is needed, and it is wrapped
    /// in a `Secret` so the buffer is zeroed when the request is done with it.
    fn terminate(&mut self, row: ProcRow, password: String, cx: &mut Context<Self>) {
        if self.busy {
            self.note(
                crate::i18n::t("上一个操作尚未完成", "The previous action has not finished"),
                StatusKind::Error,
            );
            cx.notify();
            return;
        }
        let Ok(pid) = row.pid.parse::<u32>() else {
            self.note(
                crate::i18n::t("无效的 PID", "Invalid PID"),
                StatusKind::Error,
            );
            cx.notify();
            return;
        };

        let secret = (!row.own_process).then(|| Secret::new(password));
        let response = self
            .state
            .handles
            .borrow()
            .get(&row.tab_id)
            .map(|handle| handle.kill_process(pid, secret));
        let Some(response) = response else {
            self.note(
                crate::i18n::t("SSH 会话不可用", "The SSH session is unavailable"),
                StatusKind::Error,
            );
            cx.notify();
            return;
        };

        self.busy = true;
        *self.refusal.borrow_mut() = None;
        self.note(
            crate::i18n::t("正在结束进程…", "Terminating process…"),
            StatusKind::Ok,
        );
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = response
                .await
                .unwrap_or_else(|_| crate::session::protocol::ProcessKillResult {
                    success: false,
                    message: crate::i18n::t("SSH 会话已关闭", "The SSH session has closed")
                        .to_string(),
                });
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                view.note(
                    SharedString::from(result.message),
                    if result.success {
                        StatusKind::Ok
                    } else {
                        StatusKind::Error
                    },
                );
                cx.notify();
            });
        })
        .detach();
    }

    /// The column header: the five columns the rows below fill.
    fn header(&self, cx: &Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .w_full()
            .flex_shrink_0()
            .gap(px(6.))
            .px_2()
            .text_xs()
            .text_color(muted)
            .child(cell(PID_WIDTH, Align::Start, "PID"))
            .child(cell(
                USER_WIDTH,
                Align::Start,
                crate::i18n::t("用户", "User"),
            ))
            .child(cell(CPU_WIDTH, Align::End, "CPU%"))
            .child(cell(MEM_WIDTH, Align::End, "MEM%"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(crate::i18n::t("命令", "Command")),
            )
            .into_any_element()
    }
}

impl Render for ProcessWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A termination the context menu recorded gets its window here, which is the
        // first point in the frame that has one. Taken into a local first: the borrow of
        // the cell has to end before the confirmation can borrow the view mutably.
        let asked = self.pending.borrow_mut().take();
        if let Some(row) = asked {
            self.confirm_terminate(row, window, cx);
        }

        // Everything that needs the theme or an immutable borrow of the context is taken
        // before the dialog layer, which needs it mutably. `Hsla` is `Copy`, so this
        // costs nothing and keeps the borrow checker out of the layout below.
        let theme = cx.theme();
        let background = theme.background;
        let danger = theme.danger;
        let success = theme.success;
        let muted = theme.muted_foreground;
        let header = self.header(cx);
        // The rows are virtual: only the visible window of the table is mounted, so a
        // host with four-digit PIDs worth of processes costs one screenful of elements.
        // Rows are one 20px line each — uniform by construction, plus a 1px foot that
        // stands in for the gap the old `.gap()` gave the scroll container.
        let view = cx.entity();
        let scroll = self.list_scroll.clone();
        let rows: AnyElement = uniform_list(
            "process-rows-list",
            self.rows.len(),
            move |range, _window, cx| {
                let proc_view = view.read(cx);
                let theme = cx.theme();
                let pending = proc_view.pending.clone();
                range
                    .clone()
                    .filter_map(|index| {
                        let row = proc_view.rows.get(index)?;
                        Some(
                            div()
                                .w_full()
                                .pb(px(1.))
                                .child(proc_row(row, pending.clone(), &view, theme)),
                        )
                    })
                    .collect()
            },
        )
        .with_sizing_behavior(ListSizingBehavior::Infer)
        .track_scroll(&scroll)
        .into_any_element();
        let status = self.status.clone();
        let empty = self.rows.is_empty();
        // The dialog this window's confirmation opens lives in `Root`'s queue, and
        // `Root` does not render that queue itself — the view inside it does.
        if self._root_subscription.is_none() {
            self._root_subscription = super::follow_root(window, cx);
        }
        let dialog_layer = Root::render_dialog_layer(window, cx);

        v_flex()
            .size_full()
            .bg(background)
            .child(
                v_flex()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .p_3()
                    .child(header)
                    .when_some(status, |this, status| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .truncate()
                                .text_color(if status.kind == StatusKind::Error {
                                    danger
                                } else {
                                    success
                                })
                                .child(status.text),
                        )
                    })
                    .child(
                        v_flex()
                            .id("process-rows")
                            .w_full()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .when(empty, |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(crate::i18n::t("暂无进程", "No processes")),
                                )
                            })
                            .when(!empty, |this| this.child(rows)),
                    ),
            )
            .children(dialog_layer)
            .into_any_element()
    }
}

/// How wide each column is, in the original's own numbers.
const PID_WIDTH: f32 = 56.0;
const USER_WIDTH: f32 = 84.0;
const CPU_WIDTH: f32 = 52.0;
const MEM_WIDTH: f32 = 52.0;

/// Which edge a column's text is aligned to. Numbers line up on the right and names on
/// the left, which is what makes a column of figures scannable.
#[derive(Clone, Copy, PartialEq)]
enum Align {
    Start,
    End,
}

/// A column header cell.
fn cell(width: f32, align: Align, text: impl Into<SharedString>) -> AnyElement {
    let text = text.into();
    div()
        .w(px(width))
        .flex_shrink_0()
        .truncate()
        .when(align == Align::End, |this| this.flex().justify_end())
        .child(text)
        .into_any_element()
}

/// A row's cell: fixed width, right-aligned where a number should line up.
fn cell_text(
    width: f32,
    align: Align,
    text: &str,
    theme: &gpui_kit::component::Theme,
) -> AnyElement {
    div()
        .w(px(width))
        .flex_shrink_0()
        .truncate()
        .when(align == Align::End, |this| this.flex().justify_end())
        .text_color(if align == Align::End {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

/// One process row, built without the view's `Context`.
///
/// The table is a `uniform_list`, which builds the visible rows from inside a
/// `&mut App` closure — there is no `Context` there to hand a listener to, so the
/// handlers carry an `Entity` handle instead, and the theme comes in as the
/// reference the closure already read. Same ids, same menu, same actions as the
/// listener-built row this replaced.
fn proc_row(
    row: &ProcRow,
    pending: Rc<RefCell<Option<ProcRow>>>,
    view: &Entity<ProcessWindowView>,
    theme: &gpui_kit::component::Theme,
) -> AnyElement {
    // The bar's colour says how hard the process is working, and the columns in
    // front of it are what it is working on. Drawn behind rather than beside so the
    // table keeps five columns at any width.
    let bar = if row.cpu_frac > 0.5 {
        theme.warning.opacity(0.28)
    } else {
        theme.chart_2.opacity(0.20)
    };
    let pid = row.pid.clone();
    let menu_row = row.clone();
    let hook = pending;
    // Kept so the menu's handler can mark this view dirty: the handler only gets an
    // `App`, and a request that nothing repaints for is a request that waits for an
    // unrelated frame — which may never come.
    let weak: WeakEntity<ProcessWindowView> = view.downgrade();
    let row_id = SharedString::from(format!("proc-{}-{}", row.tab_id, row.pid));
    let pid_id = SharedString::from(format!("pid-{}-{}", row.tab_id, row.pid));

    div()
        .w_full()
        .h(px(20.))
        .flex_shrink_0()
        .relative()
        .rounded(px(2.))
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .h_full()
                .w(relative(row.cpu_frac.clamp(0.0, 1.0)))
                .bg(bar),
        )
        .child(
            h_flex()
                .id(row_id)
                .w_full()
                .h_full()
                .gap(px(6.))
                .px_2()
                .items_center()
                .text_xs()
                .child(
                    div()
                        // An id, because an element with a click handler needs one to
                        // be hit-testable at all — without it the PID looks clickable
                        // and is not.
                        .id(pid_id)
                        .w(px(PID_WIDTH))
                        .flex_shrink_0()
                        .truncate()
                        // Foreground, not chart_2: the PID is drawn on top of
                        // the load bar, whose tint is chart_2 itself — blue on
                        // blue measured 2.9:1 in the light theme, 2.8:1 over a
                        // busy (yellow) bar in the dark one.
                        .text_color(theme.foreground)
                        .cursor_pointer()
                        .hover(|this| this.text_color(theme.primary))
                        .on_click({
                            let view = view.clone();
                            move |_, _, cx| {
                                let _ = view.update(cx, |view, cx| {
                                    view.copy_pid(pid.clone(), cx)
                                });
                            }
                        })
                        .child(SharedString::from(row.pid.clone())),
                )
                .child(cell_text(USER_WIDTH, Align::Start, &row.user, theme))
                .child(cell_text(CPU_WIDTH, Align::End, &row.cpu, theme))
                .child(cell_text(MEM_WIDTH, Align::End, &row.mem, theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(row.command.clone())),
                )
                // The context menu hangs off the row's own content, so it opens where
                // the pointer is rather than at the row's top-left corner.
                .context_menu(move |menu, _, _| {
                    let hook = hook.clone();
                    let weak = weak.clone();
                    let selected = menu_row.clone();
                    menu.item(
                        PopupMenuItem::new(crate::i18n::t("结束进程", "Terminate process"))
                            .on_click(move |_, _, cx| {
                                // Recorded, not acted on: this handler gets an `App`,
                                // and opening the confirmation needs a window. The
                                // notify is what makes the next frame the one that
                                // opens it.
                                if let Some(view) = weak.upgrade() {
                                    view.update(cx, |view, cx| {
                                        *hook.borrow_mut() = Some(selected.clone());
                                        *view.refusal.borrow_mut() = None;
                                        cx.notify();
                                    });
                                } else {
                                    *hook.borrow_mut() = Some(selected.clone());
                                }
                            }),
                    )
                }),
        )
        .into_any_element()
}

/// The window's title: what it is, and whose processes it is showing.
pub(crate) fn window_title(host: &str) -> SharedString {
    let what = crate::i18n::t("进程", "Processes");
    if host.is_empty() {
        SharedString::from(format!("XenTerm — {what}"))
    } else {
        SharedString::from(format!("XenTerm — {what} · {host}"))
    }
}

/// The shell drives every detached window the same way; this is what lets the process
/// monitor be one of them. Explicit delegation rather than a shared body, because the
/// methods are this view's own and only the shell's plumbing is generic.
impl super::TabFollower for ProcessWindowView {
    fn set_tab(&mut self, tab: Option<String>, cx: &mut Context<Self>) {
        ProcessWindowView::set_tab(self, tab, cx);
    }

    fn host(&self) -> SharedString {
        ProcessWindowView::host(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_says_which_host_when_it_knows() {
        assert!(window_title("").starts_with("XenTerm"));
        assert!(!window_title("").contains('·'));
        assert!(window_title("root@example.com:22").contains("root@example.com:22"));
    }
}
