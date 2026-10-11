//! The system-information window: the detailed probe's tables, one card per subject.
//!
//! ## Why a window
//!
//! The original detaches it, and the reason holds: this is the screen you open when you
//! want to know what a machine is, which is exactly when you also want the terminal
//! beside it. It opens from the resource panel's title row and follows the same session
//! the panel is describing.
//!
//! ## Where the numbers come from
//!
//! `crate::resource::TabStatus::sys` — the detailed one-shot probe a session runs
//! alongside its resource samples. The probe is slower than the samples (it runs `df`,
//! `lspci` and friends) and arrives later, which is why an empty card says "no data"
//! rather than showing zeroes: nothing has been reported yet, which is not the same
//! statement as a machine with nothing in it.
//!
//! ## What is the original's, and what is not
//!
//! The cards, their columns, the widths of those columns and the values in them are
//! the original's. The chrome is this shell's own: a padded, unclipped
//! card with an inset title — the shape the plugin manager's cards already use — rather
//! than the original's full-bleed shaded title bar, which needs a card that clips its
//! children and draws empty when that clip is applied before the card's height has
//! accounted for its rows.
//!
//! Values are left-aligned where the original centres them. Centring a value that does
//! not fit cuts off its beginning, and the beginning is the part that says *which*
//! interface or filesystem the row is about.

use std::time::Duration;

use gpui_kit::{
    component::{h_flex, v_flex, ActiveTheme as _},
    div,
    prelude::*,
    px, AnyElement, Context, FontWeight, IntoElement, Render, SharedString, Task,
    Window,
};

use crate::resource::{
    chunked_rows, cpu_usage_rows, overview_rows, single_row, tuple5_rows, InfoRow,
};
use crate::session::protocol::SystemDetails;

use super::SessionState;

/// How often the window re-reads the probe.
///
/// The probe arrives once per session and is slow, so this is about catching it when it
/// lands rather than about tracking it — the resource panel's own cadence, for the same
/// reason.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// One table row's height. Fixed rather than content-driven: a table is read by scanning
/// down a column, which needs the rows to line up.
const ROW_HEIGHT: f32 = 22.0;

/// The column widths, as fractions of the card. They are the original's, including the
/// zero-width last column of the GPU and Swap cards, which have only four.
const PAIR_WIDTHS: [f32; 5] = [0.12, 0.38, 0.12, 0.38, 0.0];
const CPU_WIDTHS: [f32; 5] = [0.48, 0.10, 0.15, 0.16, 0.11];
const GPU_WIDTHS: [f32; 5] = [0.32, 0.30, 0.20, 0.18, 0.0];
const CPU_USE_WIDTHS: [f32; 5] = [0.14, 0.14, 0.14, 0.14, 0.44];
const MEMORY_WIDTHS: [f32; 5] = [0.20, 0.20, 0.20, 0.20, 0.20];
const SWAP_WIDTHS: [f32; 5] = [0.25, 0.25, 0.25, 0.25, 0.0];
const NETWORK_WIDTHS: [f32; 5] = [0.18, 0.20, 0.20, 0.21, 0.21];
const FILESYSTEM_WIDTHS: [f32; 5] = [0.38, 0.16, 0.12, 0.16, 0.18];

/// The system-information window.
pub(crate) struct SystemInfoWindowView {
    /// The window's session state: the tables are a projection of its per-tab status.
    state: SessionState,
    /// Which tab's host is described. The shell sets it, and re-sets it when the main
    /// window's active tab changes.
    tab: Option<String>,
    /// The probe as of the last poll.
    details: SystemDetails,
    /// Whether a probe has been seen at all, which is what tells an empty table from one
    /// that has not been filled yet.
    reported: bool,
    /// The window's poll, kept alive for its life.
    _poll: Task<()>,
}

impl SystemInfoWindowView {
    pub(crate) fn new(
        state: SessionState,
        tab: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let Ok((fresh, reported)) = this.update(cx, |view, _| view.sample()) else {
                break;
            };
            let alive = this.update(cx, |view, cx| {
                if view.details != fresh || view.reported != reported {
                    view.details = fresh;
                    view.reported = reported;
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        });

        let mut view = Self {
            state,
            tab,
            details: SystemDetails::default(),
            reported: false,
            _poll: poll,
        };
        let (fresh, reported) = view.sample();
        view.details = fresh;
        view.reported = reported;
        view
    }

    /// The detailed probe for the tab this window is following.
    ///
    /// Read from the shared store rather than kept: the probe belongs to the session, and
    /// a copy here would be a second version of it that ages differently.
    fn sample(&self) -> (SystemDetails, bool) {
        let Some(tab) = self.tab.as_deref() else {
            return (SystemDetails::default(), false);
        };
        let Ok(statuses) = self.state.statuses.lock() else {
            return (SystemDetails::default(), false);
        };
        let Some(status) = statuses.get(tab) else {
            return (SystemDetails::default(), false);
        };
        // A machine can report an empty set of details; what says "nothing has arrived"
        // is whether any of the blocks carries anything.
        (status.sys.clone(), !status.sys.overview.is_empty())
    }

    /// Follow a different tab, which the shell asks for when the main window's active tab
    /// changes.
    pub(crate) fn set_tab(&mut self, tab: Option<String>, cx: &mut Context<Self>) {
        self.tab = tab;
        let (fresh, reported) = self.sample();
        self.details = fresh;
        self.reported = reported;
        cx.notify();
    }

    /// The host shown in the title bar.
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

    /// The whole window: a scrolling column of cards.
    fn cards(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let sys = &self.details;
        let empty = crate::i18n::t("暂无数据", "No data");

        v_flex()
            .id("system-info")
            .size_full()
            .overflow_y_scroll()
            .gap_3()
            .p_3()
            .child(pair_card(
                crate::i18n::t("概览", "Overview"),
                &overview_rows(&sys.overview),
                empty,
                theme,
            ))
            .child(table_card(
                "CPU",
                [
                    crate::i18n::t("名称", "Name"),
                    crate::i18n::t("核心", "Cores"),
                    crate::i18n::t("频率", "Frequency"),
                    crate::i18n::t("缓存", "Cache"),
                    crate::i18n::t("厂商", "Vendor"),
                ],
                CPU_WIDTHS,
                &single_row(&sys.cpu_info),
                empty,
                theme,
            ))
            .child(table_card(
                "GPU",
                [
                    crate::i18n::t("名称", "Name"),
                    crate::i18n::t("厂商", "Vendor"),
                    crate::i18n::t("驱动", "Driver"),
                    crate::i18n::t("显存", "Memory"),
                    "",
                ],
                GPU_WIDTHS,
                &chunked_rows(&sys.gpu_info, 4),
                empty,
                theme,
            ))
            .child(table_card(
                crate::i18n::t("CPU 占用", "CPU usage"),
                [
                    crate::i18n::t("用户", "User"),
                    crate::i18n::t("系统", "System"),
                    "Nice",
                    crate::i18n::t("空闲", "Idle"),
                    "IO / IRQ / SoftIRQ / Steal",
                ],
                CPU_USE_WIDTHS,
                &cpu_usage_rows(&sys.cpu_usage),
                empty,
                theme,
            ))
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_start()
                    .child(div().flex_1().min_w_0().child(table_card(
                        crate::i18n::t("内存", "Memory"),
                        [
                            crate::i18n::t("总量", "Total"),
                            crate::i18n::t("已用", "Used"),
                            crate::i18n::t("可用", "Free"),
                            crate::i18n::t("使用率", "Usage"),
                            crate::i18n::t("缓存", "Cache"),
                        ],
                        MEMORY_WIDTHS,
                        &single_row(&sys.memory),
                        empty,
                        theme,
                    )))
                    .child(div().flex_1().min_w_0().child(table_card(
                        crate::i18n::t("交换", "Swap"),
                        [
                            crate::i18n::t("总量", "Total"),
                            crate::i18n::t("已用", "Used"),
                            crate::i18n::t("可用", "Free"),
                            crate::i18n::t("使用率", "Usage"),
                            "",
                        ],
                        SWAP_WIDTHS,
                        &single_row(&sys.swap),
                        empty,
                        theme,
                    ))),
            )
            .child(table_card(
                crate::i18n::t("网络接口", "Network interfaces"),
                [
                    crate::i18n::t("名称", "Name"),
                    crate::i18n::t("已发送", "Sent"),
                    crate::i18n::t("已接收", "Received"),
                    crate::i18n::t("发送速度", "Send speed"),
                    crate::i18n::t("接收速度", "Receive speed"),
                ],
                NETWORK_WIDTHS,
                &tuple5_rows(&sys.networks),
                empty,
                theme,
            ))
            .child(table_card(
                crate::i18n::t("文件系统", "Filesystems"),
                [
                    crate::i18n::t("名称", "Name"),
                    crate::i18n::t("大小", "Size"),
                    crate::i18n::t("已用", "Used"),
                    crate::i18n::t("可用", "Available"),
                    crate::i18n::t("挂载点", "Mount point"),
                ],
                FILESYSTEM_WIDTHS,
                &tuple5_rows(&sys.filesystems),
                empty,
                theme,
            ))
            .into_any_element()
    }
}

impl Render for SystemInfoWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Root owns overlay presentation; this view renders only its resource cards.
        let theme = cx.theme();
        let background = theme.background;
        let cards = self.cards(cx);

        v_flex().size_full().bg(background).child(cards)
    }
}

/// A card: its title, and the rows under it.
///
/// Padded and unclipped, which is this shell's own card shape — the plugin manager's
/// cards are built the same way. The original draws its title in a full-bleed shaded bar,
/// which needs the card to clip its children against a rounded corner; a clipped card
/// whose height does not yet account for what it is clipping draws an empty card. An
/// inset title and an inset header strip say the same thing without the clip.
fn card_shell(
    title: SharedString,
    body: AnyElement,
    theme: &gpui_kit::component::Theme,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_2()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .px_1()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(body)
        .into_any_element()
}

/// The overview card: two label/value pairs per row, without a header row of its own.
fn pair_card(
    title: impl Into<SharedString>,
    rows: &[InfoRow],
    empty: &'static str,
    theme: &gpui_kit::component::Theme,
) -> AnyElement {
    let body = v_flex()
        .w_full()
        .children(if rows.is_empty() {
            vec![empty_row(empty, theme)]
        } else {
            rows.iter()
                .enumerate()
                .map(|(index, row)| {
                    let cells: Vec<AnyElement> = row
                        .cells()
                        .iter()
                        .enumerate()
                        .map(|(column, cell)| {
                            // The even columns are the labels: an overview that read as
                            // eight equal cells would be a list, not a keyed pair.
                            let muted = column % 2 == 0;
                            text_cell(cell, width_of(&PAIR_WIDTHS, column), muted, theme)
                        })
                        .collect();
                    data_row(cells, index + 1 == rows.len(), theme)
                })
                .collect()
        })
        .into_any_element();

    card_shell(title.into(), body, theme)
}

/// The width for a column of a table, falling back to the last one.
///
/// A row carries as many cells as its source produces, and the widths are a fixed table beside
/// it, so the two can disagree — a machine with an extra field, or a table whose shape was
/// edited on one side only. Indexing the widths with a column that does not exist panicked and
/// took the window down; a row that is one cell wider than its table is not a reason to lose
/// the window, so an extra cell takes the last width and is drawn.
fn width_of(widths: &[f32], column: usize) -> f32 {
    widths
        .get(column)
        .copied()
        .unwrap_or_else(|| widths.last().copied().unwrap_or(0.0))
}
/// A five-column table card, with the header row its column meanings need.
fn table_card(
    title: impl Into<SharedString>,
    headers: [&'static str; 5],
    widths: [f32; 5],
    rows: &[InfoRow],
    empty: &'static str,
    theme: &gpui_kit::component::Theme,
) -> AnyElement {
    let header = h_flex()
        .w_full()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(theme.muted)
        .children(
            headers
                .iter()
                .enumerate()
                .filter(|(index, _)| width_of(&widths, *index) > 0.0)
                .map(|(index, header)| header_cell(header, width_of(&widths, index), theme)),
        )
        .into_any_element();

    let body = v_flex()
        .w_full()
        .children(if rows.is_empty() {
            vec![empty_row(empty, theme)]
        } else {
            rows.iter()
                .enumerate()
                .map(|(index, row)| {
                    let cells: Vec<AnyElement> = row
                        .cells()
                        .iter()
                        .enumerate()
                        .filter(|(column, _)| width_of(&widths, *column) > 0.0)
                        .map(|(column, cell)| {
                            text_cell(cell, width_of(&widths, column), false, theme)
                        })
                        .collect();
                    data_row(cells, index + 1 == rows.len(), theme)
                })
                .collect()
        })
        .into_any_element();

    card_shell(
        title.into(),
        v_flex()
            .w_full()
            .gap_1()
            .child(header)
            .child(body)
            .into_any_element(),
        theme,
    )
}

/// One data row: the cells, and the hairline that separates it from the next.
///
/// No line under the last row: the card's own border is already there, and a doubled
/// edge reads as a mistake at this size.
fn data_row(cells: Vec<AnyElement>, last: bool, theme: &gpui_kit::component::Theme) -> AnyElement {
    h_flex()
        .w_full()
        .gap_2()
        .px_1()
        .py_1()
        .items_center()
        .when(!last, |this| this.border_b_1().border_color(theme.border))
        .children(cells)
        .into_any_element()
}

/// One cell of a data row.
///
/// Sized by flex weight rather than by percentage: the widths in the original are
/// proportions of the card, and a *percentage* width plus the row's gaps adds up to more
/// than the row — the last column then overflows the card. A flex weight distributes what
/// is left after the gaps, so the columns stay proportional and inside the card.
fn text_cell(
    text: &str,
    weight: f32,
    muted: bool,
    theme: &gpui_kit::component::Theme,
) -> AnyElement {
    div()
        .flex_grow(weight)
        .flex_basis(px(0.))
        .min_w_0()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .text_xs()
        .truncate()
        .font_family(theme.mono_font_family.clone())
        .text_color(if muted {
            theme.muted_foreground
        } else {
            theme.foreground
        })
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

/// One cell of a header row: the same geometry, in the muted colour.
fn header_cell(text: &str, weight: f32, theme: &gpui_kit::component::Theme) -> AnyElement {
    div()
        .flex_grow(weight)
        .flex_basis(px(0.))
        .min_w_0()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .text_xs()
        .truncate()
        .text_color(theme.muted_foreground)
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

/// What a card with nothing to show says.
fn empty_row(text: &'static str, theme: &gpui_kit::component::Theme) -> AnyElement {
    div()
        .w_full()
        .py_2()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(SharedString::from(text))
        .into_any_element()
}

/// The window's title: what it is, and which host it describes.
pub(crate) fn window_title(host: &str) -> SharedString {
    let what = crate::i18n::t("系统信息", "System information");
    if host.is_empty() {
        SharedString::from(format!("XenTerm — {what}"))
    } else {
        SharedString::from(format!("XenTerm — {what} · {host}"))
    }
}

/// The shell drives every detached window the same way; this is what lets the
/// system-information window be one of them.
impl super::TabFollower for SystemInfoWindowView {
    fn set_tab(&mut self, tab: Option<String>, cx: &mut Context<Self>) {
        SystemInfoWindowView::set_tab(self, tab, cx);
    }

    fn host(&self) -> SharedString {
        SystemInfoWindowView::host(self)
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

    #[test]
    fn the_column_fractions_match_the_cards_they_belong_to() {
        // The GPU and Swap cards have four columns; a zero-width fifth is how the header
        // row knows not to draw an empty cell at the end.
        assert_eq!(GPU_WIDTHS[4], 0.0);
        assert_eq!(SWAP_WIDTHS[4], 0.0);
        for widths in [
            PAIR_WIDTHS,
            CPU_WIDTHS,
            GPU_WIDTHS,
            CPU_USE_WIDTHS,
            MEMORY_WIDTHS,
            SWAP_WIDTHS,
            NETWORK_WIDTHS,
            FILESYSTEM_WIDTHS,
        ] {
            let total: f32 = widths.iter().sum();
            assert!(
                (total - 1.0).abs() < 0.001,
                "a card's columns should fill it: {widths:?} sums to {total}"
            );
        }
    }
}
