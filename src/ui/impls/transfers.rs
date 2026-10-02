//! The transfer manager: every download and upload this window has seen.
//!
//! ## Why it exists
//!
//! The SFTP panel can start a download and an upload, and until this view both were
//! invisible: the progress events arrived, were written into the shared store, and
//! nothing drew it. A transfer with no progress is indistinguishable from a transfer
//! that never started.
//!
//! ## Where the rows come from
//!
//! `crate::core::TransferStore` — the shared transfer store, with the same
//! `Transfer::detail()` and `Transfer::percent()` projections. The store owns row
//! identity, ordering and the in-progress count; this view only reads it,
//! and reports what the user asked for (cancel one, clear the list, open the folder)
//! back to the shell, which holds the session handles and the config.
//!
//! ## Where it is drawn
//!
//! As a popover anchored to the tab strip's transfer button — the same bubble the
//! command bar's quick commands and history use, and the same shape the original gives
//! it. The list scrolls inside a fixed width instead of following the window, because a
//! bubble the width of the window would be a panel with a tail.

use std::sync::{Arc, Mutex};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        progress::Progress,
        v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    },
    div,
    prelude::*,
    px, Animation, AnimationExt as _, AnyElement, Context, FontWeight, IntoElement, Render,
    SharedString, Task, Window,
};

// The full Lucide catalog rather than the component library's curated subset: the two
// icons this panel needs for its own actions are not in that subset.
use gpui_kit::assets::IconName;

use crate::core::{Transfer, TransferPhase, TransferStore};

/// What the panel wants the shell to do.
///
/// The shell owns the session handles and the download directory, so the panel reports
/// intent. Same shape as the SFTP panel's actions, and for the same reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TransferAction {
    /// Stop one transfer: broadcast to the sessions, because only the one that started
    /// it has it registered.
    Cancel(String),
    /// Drop every row, running or finished.
    Clear,
    /// Open the download directory in the OS file manager.
    OpenFolder,
}

/// The transfer list.
pub(crate) struct TransferListView {
    /// The store the sessions write their progress into.
    records: Arc<Mutex<TransferStore>>,
    /// The rows as of the last poll, newest first, which is the order a manager reads in:
    /// the thing you just started is the thing you are waiting for.
    rows: Vec<Transfer>,
    /// Whether anything is still running, as of the same poll. Drives the indicator on
    /// the button that opens this panel.
    active: bool,
    /// The last action an interaction produced, drained by the shell.
    pending: Option<TransferAction>,
    /// Every transfer id this view has already drawn. A row entrance plays
    /// only for ids outside this set, so re-renders and reopenings don't
    /// replay the fade for rows the user has already seen.
    seen: std::collections::HashSet<String>,
    /// Re-reads the store. A task rather than a subscription because a transfer's
    /// progress arrives on a session's own channel, which has no route to this view.
    _poll: Task<()>,
}

impl TransferListView {
    pub(crate) fn new(records: Arc<Mutex<TransferStore>>, cx: &mut Context<Self>) -> Self {
        let (rows, active) = read(&records);

        // Twice a second: fast enough that a running transfer's bar moves, slow enough
        // that a large transfer's per-chunk events are not a repaint each.
        let polled = records.clone();
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            let (fresh, active) = read(&polled);
            let alive = this.update(cx, |view, cx| {
                if view.rows != fresh || view.active != active {
                    view.rows = fresh;
                    view.active = active;
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        });

        Self {
            records,
            rows,
            active,
            pending: None,
            seen: std::collections::HashSet::new(),
            _poll: poll,
        }
    }

    /// Take the next action the user asked for, if any.
    pub(crate) fn take_action(&mut self) -> Option<TransferAction> {
        self.pending.take()
    }

    /// Re-read the store now, for the shell to call after it has changed something.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let (rows, active) = read(&self.records);
        if self.rows != rows || self.active != active {
            self.rows = rows;
            self.active = active;
            cx.notify();
        }
    }

    /// Whether anything is still running, for the button's indicator.
    pub(crate) fn has_active(&self) -> bool {
        self.active
    }

    /// One row: the name, what it is doing, and how far along it is. `entrance`
    /// is true only for a row the view has not shown before, and is what gates
    /// the fade-in.
    fn row(&self, transfer: &Transfer, entrance: bool, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let phase = transfer.phase;
        let percent = transfer.percent();
        // The bar's colour carries the outcome, so a list of twenty finished transfers
        // is read by scanning for the one that is red.
        let colour = match phase {
            TransferPhase::Failed => theme.danger,
            TransferPhase::Done => theme.success,
            TransferPhase::Cancelled => theme.muted_foreground,
            TransferPhase::Active | TransferPhase::Preparing => theme.chart_2,
        };
        let cancel_id = transfer.id.clone();
        let running = phase.is_in_progress();
        // No byte total yet — the remote is still preparing, or the worker never
        // learned the size — so a determinate bar would sit frozen at zero and
        // read as stalled. The library's indeterminate slide says "working, no
        // fraction yet", which is the truth of this phase.
        let unknown_total = running && transfer.total == 0;

        let row = h_flex()
            .w_full()
            .gap_2()
            .px_2()
            .py_1()
            .items_center()
            .child(
                // Direction as an icon rather than a word: the arrow says which way the
                // bytes are going at a glance, in the same place on every row.
                Icon::new(if transfer.is_upload {
                    IconName::ArrowUp
                } else {
                    IconName::ArrowDown
                })
                .size_3()
                .text_color(if transfer.is_upload {
                    theme.success
                } else {
                    theme.chart_2
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .child(SharedString::from(transfer.name.clone())),
            )
            .child(
                div().w(px(160.)).flex_shrink_0().child(
                    Progress::new(SharedString::from(format!("transfer-{}", transfer.id)))
                        .value(percent * 100.0)
                        .loading(unknown_total)
                        .color(colour)
                        .h(px(6.))
                        .w_full(),
                ),
            )
            .child(
                div()
                    .w(px(150.))
                    .flex_shrink_0()
                    .truncate()
                    .text_xs()
                    .text_color(if phase == TransferPhase::Failed {
                        theme.danger
                    } else {
                        theme.muted_foreground
                    })
                    .child(SharedString::from(transfer.detail())),
            )
            .child(
                // The cancel button exists only while there is something to cancel, so a
                // finished row's slot reads as done rather than as unavailable.
                div()
                    .w(px(28.))
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .when(running, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("cancel-{}", transfer.id)))
                                .debug_selector({
                                    let selector = format!("cancel-{}", transfer.id);
                                    move || selector.clone()
                                })
                                .icon(Icon::new(IconName::X))
                                .ghost()
                                .small()
                                .tooltip(crate::i18n::t("取消传输", "Cancel transfer"))
                                .on_click(cx.listener(move |this, _, _, _| {
                                    this.pending = Some(TransferAction::Cancel(cancel_id.clone()));
                                })),
                        )
                    }),
            );
        // Entrance, keyed by the transfer's own id, and only on a row's first
        // appearance — a branch rather than a conditional wrapper, because the
        // animated and plain rows are different element types.
        if entrance {
            row.with_animation(
                SharedString::from(format!("transfer-row-{}", transfer.id)),
                Animation::new(std::time::Duration::from_millis(200)),
                |row, delta| row.opacity(delta),
            )
            .into_any_element()
        } else {
            row.into_any_element()
        }
    }
}

impl Render for TransferListView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;

        let body = if self.rows.is_empty() {
            v_flex()
                .w_full()
                .px_2()
                .py_3()
                .text_xs()
                .text_color(muted)
                .child(crate::i18n::t("暂无传输", "No transfers"))
                .into_any_element()
        } else {
            // Rows arriving for the first time get the entrance fade; rows the
            // view has already shown — including everything present when the
            // popover reopens — draw at once. Without the seen-set, every
            // reopen replayed the whole list's entrance, which read as a
            // flash rather than as anything happening.
            let fresh: Vec<String> = self
                .rows
                .iter()
                .filter(|transfer| !self.seen.contains(&transfer.id))
                .map(|transfer| transfer.id.clone())
                .collect();
            self.seen.extend(fresh.iter().cloned());
            v_flex()
                .id("transfer-rows")
                .w_full()
                .max_h(px(200.))
                .overflow_y_scroll()
                .children(
                    // Newest first: the store appends, so the display order is its
                    // reverse.
                    self.rows
                        .iter()
                        .rev()
                        .map(|transfer| self.row(transfer, fresh.contains(&transfer.id), cx))
                        .collect::<Vec<_>>(),
                )
                .into_any_element()
        };

        v_flex()
            .w(px(620.))
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .pb_1()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::i18n::t("传输记录", "Transfer records")),
                    )
                    .child(
                        Button::new("transfer-open-folder")
                            .debug_selector(|| "transfer-open-folder".to_string())
                            .icon(Icon::new(IconName::FolderOpen))
                            .label(crate::i18n::t("打开目录", "Open folder"))
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, _| {
                                this.pending = Some(TransferAction::OpenFolder);
                            })),
                    )
                    .child(
                        Button::new("transfer-clear")
                            .debug_selector(|| "transfer-clear".to_string())
                            .icon(Icon::new(IconName::Trash))
                            .label(crate::i18n::t("清空", "Clear"))
                            .ghost()
                            .small()
                            // Nothing to clear is a button that would do nothing, so it
                            // is not offered.
                            .disabled(self.rows.is_empty())
                            .on_click(cx.listener(|this, _, _, _| {
                                this.pending = Some(TransferAction::Clear);
                            })),
                    ),
            )
            .child(body)
    }
}

/// The store's rows, newest first, and whether anything is running.
///
/// A poisoned lock reads as "nothing": a transfer manager that took the window down with
/// it would be worse than one that shows an empty list for a frame.
fn read(records: &Arc<Mutex<TransferStore>>) -> (Vec<Transfer>, bool) {
    let Ok(store) = records.lock() else {
        return (Vec::new(), false);
    };
    (store.rows().to_vec(), store.has_in_progress())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::TransferPhase;
    use gpui_kit::gpui::{Modifiers, TestAppContext};

    fn transfer(id: &str, phase: TransferPhase) -> Transfer {
        Transfer {
            id: id.to_string(),
            name: format!("{id}.bin"),
            is_upload: false,
            transferred: 10,
            total: 100,
            phase,
            message: String::new(),
        }
    }

    /// The panel's own job, with no window in the way: it reads the store the sessions write
    /// into, and its "something is still running" flag — which is what puts the indicator on
    /// the button that opens it — follows the store rather than the rows it happened to hold.
    ///
    /// Worth testing because it is a polling view: the rows arrive from a session's own
    /// channel, which has no route here, so the panel re-reads a shared store instead. That
    /// is the seam a test can use without a session, a server or a window.
    #[test]
    fn the_panel_follows_the_store_it_polls() {
        let records = Arc::new(Mutex::new(TransferStore::new()));
        {
            let mut store = records.lock().expect("the store is not poisoned");
            store.upsert(transfer("a", TransferPhase::Active));
            store.upsert(transfer("b", TransferPhase::Done));
        }

        let (rows, active) = read(&records);
        assert_eq!(rows.len(), 2, "both transfers are rows");
        assert!(active, "one of them is still running");
        assert_eq!(
            rows.iter()
                .filter(|t| t.phase == TransferPhase::Done)
                .count(),
            1
        );

        {
            let mut store = records.lock().expect("the store is not poisoned");
            store.upsert(transfer("a", TransferPhase::Done));
        }
        let (rows, active) = read(&records);
        assert!(!active, "with everything finished nothing is running");
        assert_eq!(rows.len(), 2, "and both rows are still there to read");

        // A failed transfer carries its reason, which is the one place the panel's detail
        // line has something to say.
        {
            let mut store = records.lock().expect("the store is not poisoned");
            let mut failed = transfer("c", TransferPhase::Failed);
            failed.message = "no space left on device".into();
            store.upsert(failed);
        }
        let (rows, _) = read(&records);
        let failed = rows
            .iter()
            .find(|t| t.id == "c")
            .expect("the failed row is there");
        assert_eq!(failed.message, "no space left on device");
    }

    /// The view itself draws what the store holds, which is the step after `read`.
    ///
    /// No selectors and no clicks: this checks the wiring from the injected store into the
    /// rows the render walks, which is the part that a change to the polling would break
    /// quietly.
    #[gpui_kit::gpui::test]
    fn the_view_takes_its_rows_from_the_store(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let records = Arc::new(Mutex::new(TransferStore::new()));
        {
            let mut store = records.lock().expect("the store is not poisoned");
            store.upsert(transfer("a", TransferPhase::Active));
        }

        let (view, cx) = cx.add_window_view({
            let records = records.clone();
            move |_, cx| TransferListView::new(records, cx)
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx.update(|_, cx| view.update(cx, |panel, cx| panel.refresh(cx)));

        assert!(
            view.read_with(cx, |panel, _| panel.rows.len()) == 1,
            "the panel took the row the store had"
        );
        assert!(
            view.read_with(cx, |panel, _| panel.has_active()),
            "and it knows something is running"
        );
    }

    /// The three buttons, pressed for the first time.
    ///
    /// The panel's actions are the whole of what it does — the shell is what cancels a
    /// transfer, opens a folder or drops the rows — so this is the panel's entire contract,
    /// and none of it had ever run. Two of them are conditional: cancel is offered only for a
    /// row that is still going, and clear only when there is something to clear, because a
    /// button that would do nothing is not offered. Both conditions are asserted here.
    #[gpui_kit::gpui::test]
    fn the_buttons_report_the_actions_they_stand_for(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let records = Arc::new(Mutex::new(TransferStore::new()));
        {
            let mut store = records.lock().expect("the store is not poisoned");
            store.upsert(transfer("running", TransferPhase::Active));
            store.upsert(transfer("finished", TransferPhase::Done));
        }

        let (view, cx) = cx.add_window_view({
            let records = records.clone();
            move |_, cx| TransferListView::new(records, cx)
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx.update(|_, cx| view.update(cx, |panel, cx| panel.refresh(cx)));

        // Cancel belongs to the running row, and only to it.
        assert!(
            cx.debug_bounds(leaked("cancel-finished".to_string()))
                .is_none(),
            "a finished transfer is not offered a cancel"
        );
        let bounds = cx
            .debug_bounds(leaked("cancel-running".to_string()))
            .unwrap_or_else(|| panic!("the running row was not offered a cancel"));
        cx.simulate_click(bounds.center(), Modifiers::default());
        assert_eq!(
            view.update(cx, |panel, _| panel.take_action()),
            Some(TransferAction::Cancel("running".into()))
        );

        // Clear is offered while there are rows, and reports that it wants them gone.
        let bounds = cx
            .debug_bounds("transfer-clear")
            .unwrap_or_else(|| panic!("the clear button was not drawn"));
        cx.simulate_click(bounds.center(), Modifiers::default());
        assert_eq!(
            view.update(cx, |panel, _| panel.take_action()),
            Some(TransferAction::Clear)
        );

        // And the folder button, which is unconditional: it is about the download directory
        // rather than about the rows.
        let bounds = cx
            .debug_bounds("transfer-open-folder")
            .unwrap_or_else(|| panic!("the folder button was not drawn"));
        cx.simulate_click(bounds.center(), Modifiers::default());
        assert_eq!(
            view.update(cx, |panel, _| panel.take_action()),
            Some(TransferAction::OpenFolder)
        );
    }

    /// `debug_bounds` wants a `'static` selector and these are built from transfer ids.
    fn leaked(selector: String) -> &'static str {
        Box::leak(selector.into_boxed_str())
    }
}
