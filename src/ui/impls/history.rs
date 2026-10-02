//! The command-history dropdown: what this session's box has run, and a way back to it.
//!
//! ## What it does
//!
//! The original's history dropdown, as a panel above the command bar: the commands the box
//! has run, oldest at the top, with a filter box and a per-entry delete. Clicking a row
//! *recalls* it into the input rather than running it — the original behaves that way too,
//! and it is the honest reading of a list you opened to find something you are about to
//! edit.
//!
//! ## Where the rows come from
//!
//! `crate::core::history`, which owns the filter rule: a case-insensitive substring, with
//! each row remembering its place in the store so a delete acts on the entry and not on
//! the position it happened to occupy on screen.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputEvent, InputState},
        v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    },
    div,
    prelude::*,
    px, AnyElement, Context, Entity, FontWeight, IntoElement, Render, SharedString, Subscription,
    Window,
};

// The full Lucide catalog rather than the component library's curated subset.
use gpui_kit::assets::IconName;

use crate::config::ConfigStore;
use crate::core::history;

/// What the panel wants the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HistoryAction {
    /// Put this command back in the input box, ready to run or edit.
    Recall(String),
    /// Forget one entry, by its place in the stored history.
    Delete(usize),
    /// Forget all of them.
    Clear,
}

/// The history dropdown.
pub(crate) struct HistoryView {
    store: Rc<RefCell<ConfigStore>>,
    filter: Entity<InputState>,
    pending: Option<HistoryAction>,
    /// Kept alive: a dropped subscription is a filter box that types and does not filter.
    _filter_subscription: Subscription,
}

impl HistoryView {
    pub(crate) fn new(
        store: Rc<RefCell<ConfigStore>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::t("搜索历史", "Search history"))
        });
        // The filter's value is read during render, so a keystroke has to redraw the panel
        // — there is nothing else to change.
        let filter_subscription = cx.subscribe_in(
            &filter,
            window,
            |_: &mut Self, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        );

        Self {
            store,
            filter,
            pending: None,
            _filter_subscription: filter_subscription,
        }
    }

    /// Take the next action, if any.
    pub(crate) fn take_action(&mut self) -> Option<HistoryAction> {
        self.pending.take()
    }

    /// Clear the filter, for the shell to call when the panel is opened.
    ///
    /// A dropdown that remembers the last search is a dropdown that looks empty the next
    /// time it is opened, which reads as "the history is gone".
    pub(crate) fn reset_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// Re-read the history, for the shell to call after it recorded something.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// One row: the command, and the way to forget it.
    fn row(&self, row: &history::HistoryRow, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let command = row.command.clone();
        let index = row.index;
        h_flex()
            .id(SharedString::from(format!("history-row-{index}")))
            .w_full()
            .gap_2()
            .px_2()
            .py_1()
            .items_center()
            .rounded_sm()
            .hover(|this| this.bg(theme.muted))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, _| {
                this.pending = Some(HistoryAction::Recall(command.clone()));
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .child(SharedString::from(row.command.clone())),
            )
            .child(
                Button::new(SharedString::from(format!("history-delete-{index}")))
                    .icon(Icon::new(IconName::X))
                    .ghost()
                    .small()
                    .tooltip(crate::i18n::t("删除这一条", "Forget this one"))
                    .accessibility_label(crate::i18n::t("删除这一条", "Forget this one"))
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.pending = Some(HistoryAction::Delete(index));
                    })),
            )
            .into_any_element()
    }
}

impl Render for HistoryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let query = self.filter.read(cx).value().to_string();
        let rows = {
            let store = self.store.borrow();
            history::filtered(store.command_history(), &query)
        };

        let body = if rows.is_empty() {
            v_flex()
                .w_full()
                .px_2()
                .py_3()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(
                    // Two different facts, and the difference matters: no history at all,
                    // or a filter that matched none of it.
                    if query.trim().is_empty() {
                        crate::i18n::t("还没有历史命令", "No commands yet")
                    } else {
                        crate::i18n::t("没有匹配的命令", "Nothing matches")
                    },
                )
                .into_any_element()
        } else {
            v_flex()
                .id("history-rows")
                .w_full()
                .max_h(px(220.))
                .overflow_y_scroll()
                .children(rows.iter().map(|row| self.row(row, cx)).collect::<Vec<_>>())
                .into_any_element()
        };

        v_flex()
            .w_full()
            .flex_shrink_0()
            .bg(theme.background)
            .border_t_1()
            .border_color(border)
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::i18n::t("命令历史", "Command history")),
                    )
                    .child(
                        Button::new("history-clear")
                            .icon(Icon::new(IconName::Trash))
                            .label(crate::i18n::t("清空", "Clear"))
                            .ghost()
                            .small()
                            .disabled(self.store.borrow().command_history().is_empty())
                            .on_click(cx.listener(|this, _, _, _| {
                                this.pending = Some(HistoryAction::Clear);
                            })),
                    ),
            )
            .child(div().w_full().px_2().py_1().child(Input::new(&self.filter)))
            .child(body)
    }
}
