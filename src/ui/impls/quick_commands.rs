//! The quick-command dock: the commands a session is expected to need, one click away.
//!
//! ## Where the rows come from
//!
//! `crate::core::quick` — the grouping rule (the implicit "default" group first, then
//! named groups alphabetically, entries in their saved order) that this dock projects.
//! What stays here is the one thing that is a view's own: which groups the user has
//! folded.
//!
//! ## What is the original's, and what is not
//!
//! The panel's shape: a title strip with a close button, a scrolling list of foldable
//! groups whose entries show the name over the command it runs, and a manage button
//! along the bottom. The dock can be moved between the right edge and the bottom, which
//! is the shell's business — it owns the layout — so the panel only knows which edge it
//! is on, for its close button's icon.
//!
//! Clicking an entry either sends it or drops it into the command bar: that is the
//! `send_enter` flag, and it is the whole reason a quick command has one.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _,
    },
    div,
    prelude::*,
    px, Animation, AnimationExt as _, AnyElement, Context, FontWeight, IntoElement, Render,
    SharedString, Window,
};

// The full Lucide catalog rather than the component library's curated subset: the icons
// this panel needs for its own actions are not in that subset.
use gpui_kit::assets::IconName;

use crate::config::ConfigStore;
use crate::core::quick::{self, QuickRow};

/// How long a run's flash lasts: long enough to be seen, short enough that a
/// second command's flash is not blurred into the first's.
const FLASH_TIME: std::time::Duration = std::time::Duration::from_millis(350);

/// Where the dock sits, which decides the direction of its close button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DockEdge {
    Right,
    Bottom,
}

impl DockEdge {
    /// Read the setting.
    ///
    /// Anything that is not the bottom is the right edge: that is the default, and it is
    /// where a docked panel has always been.
    pub(crate) fn from_setting(value: &str) -> Self {
        if value == "bottom" {
            Self::Bottom
        } else {
            Self::Right
        }
    }
}

/// What the panel wants the shell to do.
///
/// The shell owns the sessions and the layout, so the panel reports intent — the same
/// shape as the other panels' actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QuickAction {
    /// Put this command where it belongs: sent with Return, or typed into the command bar
    /// for the user to finish.
    Run { command: String, send_enter: bool },
    /// Open the manager.
    Manage,
    /// Hide the dock.
    Close,
    /// Fold or unfold a group.
    ToggleGroup(String),
}

/// The quick-command dock.
pub(crate) struct QuickCommandsView {
    store: Rc<RefCell<ConfigStore>>,
    /// Folded groups, by display name. Not persisted and not shared, so folding a group
    /// is a decision about this dock rather than about the configuration.
    collapsed: HashSet<String>,
    /// Which edge the dock is on, for its close button's icon.
    edge: DockEdge,
    /// Whether to draw the dock's own title strip.
    ///
    /// False when the shell hosts it as a tab: the tab row above it *is* the header, and
    /// two headers stacked on each other is exactly the spread-out look the layout is
    /// trying not to be.
    chrome: bool,
    /// The last thing the user asked for, drained by the shell.
    pending: Option<QuickAction>,
    /// How many times each group has turned, which is what makes its chevron
    /// swing rather than swap.
    chevron_turn: super::chevron::TurnCounter,
    /// The row a command was just run from, as (position, epoch, when) — the
    /// confirmation a click gets while the bytes it sent are still only visible
    /// as the remote shell's echo. Epoch salts the flash animation's id, so a
    /// second click on the same row starts a new flash instead of finding the
    /// old one already spent.
    flash: Option<(usize, u64, std::time::Instant)>,
    flash_epoch: u64,
}

impl QuickCommandsView {
    pub(crate) fn new(store: Rc<RefCell<ConfigStore>>, edge: DockEdge, chrome: bool) -> Self {
        Self {
            store,
            // The original starts every group folded, which is what makes a long list of
            // commands a list of headings until the user asks for more.
            collapsed: HashSet::new(),
            edge,
            chrome,
            pending: None,
            chevron_turn: super::chevron::new_turn_counter(),
            flash: None,
            flash_epoch: 0,
        }
    }

    /// Take the next action the user asked for, if any.
    pub(crate) fn take_action(&mut self) -> Option<QuickAction> {
        self.pending.take()
    }

    /// Re-read the commands, for the shell to call after the manager saved something.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// The rows to draw, with folding already applied.
    fn rows(&self) -> Vec<QuickRow> {
        let store = self.store.borrow();
        quick::rows(store.quick_commands(), store.quick_groups())
    }

    /// The title strip: what this is, and the way to close it.
    fn header(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mut close = Button::new("quick-close")
            .icon(Icon::new(IconName::X))
            .ghost()
            .small()
            .tooltip(crate::i18n::t("关闭", "Close"))
            .accessibility_label(crate::i18n::t("关闭", "Close"));
        if self.edge == DockEdge::Bottom {
            close = close.tooltip(crate::i18n::t("收起面板", "Hide the panel"));
        }

        h_flex()
            .w_full()
            .flex_shrink_0()
            .h(px(34.))
            .gap_2()
            .px_2()
            .items_center()
            .bg(theme.muted)
            .border_b_1()
            .border_color(theme.border)
            .child(
                Icon::new(IconName::Zap)
                    .size_4()
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(crate::i18n::t("快速命令", "Quick commands")),
            )
            .child(close.on_click(cx.listener(|this, _, _, _| {
                this.pending = Some(QuickAction::Close);
            })))
            .into_any_element()
    }

    /// One group heading: the chevron, the name, and how many entries are under it.
    fn group_header(&self, row: &QuickRow, count: usize, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let folded = self.collapsed.contains(&row.group);
        let group = row.group.clone();
        let this_turn = self
            .chevron_turn
            .try_borrow()
            .ok()
            .and_then(|map| map.get(&row.group).copied());
        h_flex()
            // An id, because a row that can be clicked has to be a stateful element for
            // GPUI to route the click to it.
            .id(SharedString::from(format!("quick-group-{}", row.group)))
            .w_full()
            .h(px(26.))
            .gap_1()
            .px_1()
            .items_center()
            .rounded_sm()
            .hover(|this| this.bg(theme.muted))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                // The view folds it *and* reports it: the shell redraws nothing, but a
                // second view of the same list — a future sidebar — would need to know.
                if !this.collapsed.remove(&group) {
                    this.collapsed.insert(group.clone());
                }
                // Before the repaint: this is what tells the header's chevron that
                // the coming frame is a turn, not a re-render.
                super::chevron::bump_turn(&this.chevron_turn, &group);
                this.pending = Some(QuickAction::ToggleGroup(group.clone()));
                cx.notify();
            }))
            .child(super::chevron::folding_chevron(
                &row.group,
                folded,
                this_turn,
                theme.muted_foreground,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(row.group.clone())),
            )
            .when(folded && count > 0, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(count.to_string())),
                )
            })
            .into_any_element()
    }

    /// One command: the name over what it runs.
    fn item(&self, row: &QuickRow, position: usize, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let command = row.command.clone();
        let send_enter = row.send_enter;
        // The flash rides on this click's epoch, so re-renders that happen for
        // other reasons replay nothing and a fresh click starts a fresh flash.
        let flash = self
            .flash
            .filter(|(flashed, _, at)| *flashed == position && at.elapsed() < FLASH_TIME);
        let primary = theme.primary;
        let row = v_flex()
            // Keyed by position rather than by name: two entries may legitimately have the
            // same name in different groups, and an id has to be one of a kind.
            .id(SharedString::from(format!("quick-item-{position}")))
            .w_full()
            .gap_0p5()
            .px_2()
            .py_1()
            .rounded_sm()
            .hover(|this| this.bg(theme.muted))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.pending = Some(QuickAction::Run {
                    command: command.clone(),
                    send_enter,
                });
                // The click's receipt: the row flashes while the terminal is
                // still only about to echo, so "did that do anything" is
                // answered at the point that was pressed.
                this.flash_epoch += 1;
                this.flash = Some((
                    position,
                    this.flash_epoch,
                    std::time::Instant::now(),
                ));
                cx.notify();
            }))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_sm()
                    .child(SharedString::from(row.name.clone())),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(row.command.clone())),
            );
        match flash {
            Some((_, epoch, _)) => row
                .with_animation(
                    SharedString::from(format!("quick-flash-{epoch}")),
                    Animation::new(FLASH_TIME),
                    move |row, delta| row.bg(primary.opacity(0.4 * (1.0 - delta))),
                )
                .into_any_element(),
            None => row.into_any_element(),
        }
    }
}

impl Render for QuickCommandsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let rows = self.rows();

        let mut body: Vec<AnyElement> = Vec::new();
        if rows.is_empty() {
            body.push(
                v_flex()
                    .w_full()
                    .py_6()
                    .items_center()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(crate::i18n::t("暂无快速命令", "No quick commands yet"))
                    .into_any_element(),
            );
        }
        for (index, row) in rows.iter().enumerate() {
            // How many entries follow this heading before the next one: the count is what
            // makes a folded group readable as "there is something in here".
            let count = rows[index..]
                .iter()
                .skip(1)
                .take_while(|next| !next.header)
                .count();
            let folded = self.collapsed.contains(&row.group);
            if row.header {
                body.push(self.group_header(row, count, cx));
            }
            if !folded && row.index.is_some() {
                body.push(self.item(row, index, cx));
            }
        }

        v_flex()
            .size_full()
            .bg(theme.background)
            .when(self.chrome, |this| this.child(self.header(cx)))
            .child(
                v_flex()
                    .id("quick-commands")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .gap_0p5()
                    .p_1()
                    .children(body),
            )
            .child(
                // The manager's door along the bottom, as in the original: the panel is
                // for using commands, and the one thing you do *to* them lives at its
                // edge rather than among them.
                div()
                    .w_full()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        Button::new("quick-manage")
                            .icon(Icon::new(IconName::Settings2))
                            .label(crate::i18n::t("管理快速命令", "Manage commands"))
                            .ghost()
                            .w_full()
                            .on_click(cx.listener(|this, _, _, _| {
                                this.pending = Some(QuickAction::Manage);
                            })),
                    ),
            )
    }
}
