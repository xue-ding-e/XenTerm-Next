//! The session list: what you can connect to, grouped and searchable.
//!
//! The first view migrated to GPUI Kit, and first because it is the most used and
//! because it exercises the shell's layering hardest: a list beside a terminal is
//! where a shell built for one pane has to become a shell built for several.
//!
//! The projection is not reimplemented. `crate::core::SessionRow` is built by
//! `crate::app::session_models`, which owns the grouping, the search matching and the
//! group-header bookkeeping and names no toolkit type — so this view renders the same
//! rows that projection defines, in the same order, because it is the same function.
//!
//! Folder headings support toolkit drag-and-drop ordering. Session moves
//! between folders remain explicit menu actions; dragging a folder changes only
//! its display order, never session membership.

use std::rc::Rc;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        list::{List, ListDelegate, ListItem, ListState},
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
        ActiveTheme as _, Icon, IndexPath,
    },
    div,
    prelude::*,
    px, AnyElement, App, Context, Entity, FontWeight, IntoElement, Render, SharedString, Window,
};

use gpui_kit::assets::IconName;

use crate::config::ConfigStore;
use crate::core::SessionRow;

/// The full sidebar uses one uniform item slot for both group headers and
/// two-line sessions. GPUI Kit measures a single item and section-header height;
/// mixing absent headers with expanded headers leaves gaps or overlaps.
const FULL_ROW_HEIGHT: f32 = 48.;

/// A typed, profile-scoped payload: unrelated windows cannot reorder one
/// another's stores merely because their group names happen to match.
#[derive(Clone)]
struct DraggedSessionGroup {
    name: String,
    store: Rc<std::cell::RefCell<ConfigStore>>,
}

impl Render for DraggedSessionGroup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_sm()
            .bg(cx.theme().muted)
            .child(Icon::new(IconName::Folder).size_3())
            .child(SharedString::from(self.name.clone()))
    }
}

/// A fold is a saved preference. Keep the displayed state unchanged on a
/// failed save; ConfigStore retains a sanitized persistence_error for the
/// shell's existing warning channel, so no raw path or credential is logged.
fn persist_group_fold(
    store: &Rc<std::cell::RefCell<ConfigStore>>,
    group: &str,
    collapsed: bool,
) -> bool {
    let mut store = store.borrow_mut();
    let before = store.cache.collapsed_session_groups.clone();
    store.set_session_group_collapsed(group, collapsed);
    if store.save().is_err() {
        store.cache.collapsed_session_groups = before;
        return false;
    }
    true
}

/// The second line of a row: what it connects to.
///
/// A serial session has a device and a framing where a host has an address, which is
/// why the projection carries both spellings rather than making the view decide.
fn row_detail(row: &SessionRow) -> SharedString {
    if !row.serial_detail.is_empty() {
        return row.serial_detail.clone().into();
    }
    if row.user.is_empty() {
        return row.host.clone().into();
    }
    format!("{}@{}:{}", row.user, row.host, row.port).into()
}

/// Whether this row is a session rather than a group heading.
///
/// Both are rows in the projection and only one is connectable. Empty ids mark the
/// headings — the convention `crate::app::session_models` documents — so this test
/// lives in one place instead of being respelled at each action.
fn is_connectable(row: &SessionRow) -> bool {
    !row.id.is_empty()
}

/// An icon for the row, by what it connects to.
///
/// From the full Lucide catalog rather than the component crate's compatibility
/// subset, so the icon can be the one that means this thing: a terminal for a
/// built-in local shell, a plug for a serial device, a server for a saved host.
fn session_icon(row: &SessionRow) -> IconName {
    if row.builtin {
        IconName::SquareTerminal
    } else if !row.serial_detail.is_empty() {
        IconName::Plug
    } else {
        IconName::Server
    }
}

/// The list's rows and its selection.
///
/// Owns the store rather than a snapshot because the list's own search box filters
/// through `perform_search`, and re-running the projection is the only way to filter
/// that cannot disagree with the unfiltered list: the matching rules live in the
/// projection (`session_matches_normalized_query`), and a second filter here would be
/// a second set of them.
pub(crate) struct SessionListDelegate {
    store: Rc<std::cell::RefCell<ConfigStore>>,
    rows: Vec<SessionRow>,
    /// What a row's context menu asked for, waiting for the shell.
    ///
    /// A cell rather than a channel because the menu handler is synchronous and runs on
    /// this thread; a channel would be a queue with one producer and one consumer that
    /// are never concurrent.
    pending: Rc<std::cell::RefCell<Option<SessionListAction>>>,
    /// The session the terminal is showing, so the list marks it.
    active: Option<String>,
    /// Mirrors toolkit selection for keyboard confirmation of full-mode headers.
    selected: Option<IndexPath>,
    /// The last search the list asked for.
    ///
    /// Kept because a group toggle has to rebuild the rows and the rebuild must honour
    /// the same filter: rebuilding with an empty query while a search is active would
    /// quietly show rows the user had filtered away. The list has no public getter for
    /// its own query, so the value is captured where it is handed over.
    query: String,
    /// Palette shape: one-line rows and divider-word headings. The delegate
    /// draws the rows, so the flag lives here rather than on the view.
    compact: bool,
    /// Group boundaries within `rows`: one span per display group. In the full
    /// sidebar item zero is always the group's header; following items map to
    /// real sessions. Empty groups therefore keep their actual header without
    /// a fake placeholder item. Compact palettes retain separate headings.
    sections: Vec<SectionSpan>,
    /// The list entity holding this delegate, set right after construction.
    /// Section headers' click handlers receive only an `App`, so folding or
    /// opening a group goes through this handle to rebuild the rows.
    list: Option<gpui_kit::WeakEntity<ListState<Self>>>,
    /// How many times each group has turned, which is what makes its chevron
    /// swing rather than swap. Shared with the click handlers, which receive
    /// an `App` and so cannot reach `&mut self`.
    chevron_turn: super::chevron::TurnCounter,
}

/// One display group's slice of the flat `rows` vector.
struct SectionSpan {
    group: String,
    start: usize,
    len: usize,
    /// Cached with the projection so virtual-list item lookup stays O(1).
    session_count: usize,
    collapsed: bool,
}

impl SessionListDelegate {
    fn new(
        store: Rc<std::cell::RefCell<ConfigStore>>,
        active: Option<String>,
        compact: bool,
    ) -> Self {
        let rows = rows_for(&store, "");
        let mut delegate = Self {
            store,
            rows,
            active,
            selected: None,
            query: String::new(),
            pending: Rc::new(std::cell::RefCell::new(None)),
            compact,
            sections: Vec::new(),
            list: None,
            chevron_turn: super::chevron::new_turn_counter(),
        };
        delegate.rebuild_sections();
        delegate
    }

    /// Recompute the group spans from the flat rows: a new span starts where
    /// a row's group differs from the previous row's. The projection orders
    /// rows grouped, so transitions are the whole of the structure.
    fn rebuild_sections(&mut self) {
        let mut sections: Vec<SectionSpan> = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            match sections.last_mut() {
                Some(span) if span.group == row.group => {
                    span.len += 1;
                    span.session_count += usize::from(is_connectable(row));
                }
                _ => sections.push(SectionSpan {
                    group: row.group.clone(),
                    start: index,
                    len: 1,
                    session_count: usize::from(is_connectable(row)),
                    // The quick-connect palette always shows candidates; sidebar
                    // folds must not hide them or vary section-header heights.
                    collapsed: !self.compact && row.collapsed,
                }),
            }
        }
        self.sections = sections;
    }

    /// Re-read the rows and the group spans after the store changed.
    fn rebuild_after_store_change(&mut self) {
        self.rows = rows_for(&self.store, &self.query);
        self.rebuild_sections();
    }

    /// Remember the list entity, so section-header clicks can rebuild the
    /// rows through it.
    fn set_list(&mut self, list: gpui_kit::WeakEntity<ListState<Self>>) {
        self.list = Some(list);
    }

    fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    fn header_group_at(&self, ix: IndexPath) -> Option<&str> {
        if self.compact || ix.row != 0 {
            return None;
        }
        self.sections
            .get(ix.section)
            .map(|span| span.group.as_str())
    }

    fn section_session_count(&self, span: &SectionSpan) -> usize {
        span.session_count
    }

    fn visible_items_count(&self, section: usize) -> usize {
        let Some(span) = self.sections.get(section) else {
            return 0;
        };
        if span.collapsed && !self.searching() {
            1
        } else if self.compact {
            span.len
        } else {
            1 + self.section_session_count(span)
        }
    }

    /// Expanded and folded groups share this exact element and slot height.
    /// Only the chevron turns; no replacement two-line card changes its size.
    fn render_full_group_header(
        &self,
        section: usize,
        cx: &Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let span = self.sections.get(section)?;
        let group = span.group.clone();
        let group_for_toggle = group.clone();
        let searching = self.searching();
        let count = self.section_session_count(span);
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().muted;
        let store = self.store.clone();
        let list = self.list.clone();
        let chevron_turn = self.chevron_turn.clone();
        Some(
            ListItem::new(SharedString::from(format!("group-header-{group}")))
                .p_0()
                .h(px(FULL_ROW_HEIGHT))
                .child(
                    h_flex()
                        .id(SharedString::from(format!("group-hdr-{group}")))
                        .debug_selector({
                            let group = group.clone();
                            move || format!("group-hdr-{group}")
                        })
                        .w_full()
                        .min_w_0()
                        .h(px(FULL_ROW_HEIGHT))
                        .px_3()
                        .gap_2()
                        .items_center()
                        .rounded_sm()
                        .when(!searching, |this| {
                            this.cursor_pointer().hover(|this| this.bg(hover))
                        })
                        .when(!searching, |this| {
                            let target = group.clone();
                            let store = self.store.clone();
                            let list = self.list.clone();
                            this.on_drag(
                                DraggedSessionGroup { name: group.clone(), store: store.clone() },
                                |drag, _, _, cx| cx.new(|_| drag.clone()),
                            )
                            .drag_over::<DraggedSessionGroup>(|style, _, _, cx| {
                                style.bg(cx.theme().accent.opacity(0.2))
                            })
                            .on_drop(move |drag: &DraggedSessionGroup, window, cx| {
                                cx.stop_propagation();
                                if !Rc::ptr_eq(&store, &drag.store) {
                                    return;
                                }
                                // Commit only on drop. Leaving the target/cancelling
                                // never changes persisted order or folded state.
                                let changed = store.borrow_mut()
                                    .move_session_group_and_save(&drag.name, &target)
                                    .unwrap_or(false);
                                if changed {
                                    if let Some(list) = list.as_ref().and_then(|list| list.upgrade()) {
                                        list.update(cx, |state, cx| reload_list(state, window, cx));
                                    }
                                }
                            })
                        })
                        .on_click(move |_, window, cx| {
                            // The list wrapper emits Confirm for any item click,
                            // including disabled ListItems. Consume header clicks
                            // even during search so they can never connect.
                            cx.stop_propagation();
                            if searching {
                                return;
                            }
                            let collapsed = store
                                .borrow()
                                .collapsed_session_groups()
                                .is_some_and(|groups| groups.contains(&group_for_toggle));
                            if persist_group_fold(&store, &group_for_toggle, !collapsed) {
                                super::chevron::bump_turn(&chevron_turn, &group_for_toggle);
                            }
                            if let Some(list) = list.as_ref().and_then(|list| list.upgrade()) {
                                list.update(cx, |state, cx| reload_list(state, window, cx));
                            }
                        })
                        .child(super::chevron::folding_chevron(
                            &group,
                            span.collapsed && !searching,
                            self.chevron_turn
                                .try_borrow()
                                .ok()
                                .and_then(|map| map.get(&group).copied()),
                            muted,
                        ))
                        .child(Icon::new(IconName::Folder).size_3().text_color(muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(muted)
                                .child(SharedString::from(group)),
                        )
                        .child(div().flex_shrink_0().text_xs().text_color(muted).child(
                            SharedString::from(if count == 1 {
                                crate::i18n::t("1 个会话", "1 session").to_string()
                            } else {
                                format!("{count} {}", crate::i18n::t("个会话", "sessions"))
                            }),
                        )),
                ),
        )
    }

    /// The row at `ix`: `row` is relative to the section.
    fn row_at(&self, ix: IndexPath) -> Option<&SessionRow> {
        let span = self.sections.get(ix.section)?;
        if ix.row >= self.visible_items_count(ix.section) {
            return None;
        }
        let row = if self.compact {
            ix.row
        } else {
            ix.row.checked_sub(1)?
        };
        let row = self.rows.get(span.start + row)?;
        (self.compact || is_connectable(row)).then_some(row)
    }

    /// How many rows are showing, for the header.
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// The row at `ix` if it is a session rather than a heading.
    ///
    /// The one place the "row or heading" test is applied on the way out, so a heading
    /// can never be connected to even if a caller forgets to check.
    pub(crate) fn connectable_at(&self, ix: IndexPath) -> Option<String> {
        let span = self.sections.get(ix.section)?;
        if span.collapsed && !self.searching() {
            return None;
        }
        let row = self.row_at(ix)?;
        is_connectable(row).then(|| row.id.clone())
    }

    /// Resolve a stable session id after rows or group boundaries have moved.
    fn index_for_session(&self, id: &str) -> Option<IndexPath> {
        self.sections
            .iter()
            .enumerate()
            .find_map(|(section, span)| {
                if span.collapsed && !self.searching() {
                    return None;
                }
                self.rows[span.start..span.start + span.len]
                    .iter()
                    .position(|row| is_connectable(row) && row.id == id)
                    .map(|row| IndexPath::new(row + usize::from(!self.compact)).section(section))
            })
    }
}

/// Rebuild both projections in one list update, keeping selection attached to
/// session identity rather than an index that may now name a different row.
/// Neither setter scrolls or focuses, so an edit does not move the user's view.
fn reload_list(
    state: &mut ListState<SessionListDelegate>,
    window: &mut Window,
    cx: &mut Context<ListState<SessionListDelegate>>,
) {
    let selected_group = state
        .selected_index()
        .and_then(|ix| state.delegate().header_group_at(ix))
        .map(str::to_owned);
    let selected = state
        .selected_index()
        .and_then(|ix| state.delegate().connectable_at(ix));
    let right_clicked = state
        .right_clicked_index()
        .and_then(|ix| state.delegate().connectable_at(ix));
    state.delegate_mut().rebuild_after_store_change();
    let selected = selected
        .and_then(|id| state.delegate().index_for_session(&id))
        .or_else(|| {
            selected_group.and_then(|group| {
                state
                    .delegate()
                    .sections
                    .iter()
                    .position(|span| span.group == group)
                    .map(|section| IndexPath::new(0).section(section))
            })
        });
    let right_clicked = right_clicked.and_then(|id| state.delegate().index_for_session(&id));
    state.set_selected_index(selected, window, cx);
    state.set_right_clicked_index(right_clicked, window, cx);
    cx.notify();
}

impl ListDelegate for SessionListDelegate {
    type Item = ListItem;

    fn sections_count(&self, _cx: &App) -> usize {
        self.sections.len()
    }

    fn items_count(&self, section: usize, _cx: &App) -> usize {
        // A real full-mode header remains item zero even when the group is
        // folded or empty. No zero-item section or invisible filler is needed.
        self.visible_items_count(section)
    }

    /// A header is keyboard-selectable in full mode. Remember the index so
    /// Enter toggles it while session confirmation remains the shell's job.
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix;
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        if self.searching() {
            return;
        }
        let Some(group) = self
            .selected
            .and_then(|ix| self.header_group_at(ix))
            .map(str::to_owned)
        else {
            return;
        };
        // The toolkit emits Confirm even for headers; index zero remains a
        // header before and after this deferred toggle, so consumers always
        // resolve it to no session. Deferring also avoids borrowing ListState
        // recursively while its delegate is handling the key event.
        cx.defer_in(window, move |state, window, cx| {
            let store = &state.delegate().store;
            let collapsed = store
                .borrow()
                .collapsed_session_groups()
                .is_some_and(|groups| groups.contains(&group));
            if persist_group_fold(store, &group, !collapsed) {
                super::chevron::bump_turn(&state.delegate().chevron_turn, &group);
            }
            reload_list(state, window, cx);
        });
    }

    /// Filtering, done by rebuilding the projection with the query.
    ///
    /// The list calls this as its search box changes. Rebuilding rather than filtering
    /// the existing rows keeps one implementation of what a match is — the collapse
    /// rules, the expanded-groups-while-searching rule and the empty-folder
    /// placeholders all change with the query, which is exactly what
    /// `build_session_rows` already handles.
    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> gpui_kit::Task<()> {
        self.query = query.to_string();
        self.rebuild_after_store_change();
        if !self.compact && self.searching() {
            let query = self.query.clone();
            // ListState initially selects (0, 0) after calling this method.
            // That is now a header, so move to the first actual search match
            // afterward. Enter after typing keeps its normal connect action.
            cx.defer_in(window, move |state, window, cx| {
                if state.delegate().query != query {
                    return;
                }
                let first = state
                    .delegate()
                    .rows
                    .iter()
                    .find(|row| is_connectable(row))
                    .and_then(|row| state.delegate().index_for_session(&row.id));
                state.set_selected_index(first, window, cx);
                cx.notify();
            });
        }
        gpui_kit::Task::ready(())
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        if !self.compact && ix.row == 0 {
            return self.render_full_group_header(ix.section, cx);
        }
        let row = self.row_at(ix)?.clone();

        // Only compact mode renders the projection's empty-group placeholder.
        // Full mode includes the zero count in its real, always-visible header.
        if !is_connectable(&row) {
            return Some(
                ListItem::new(SharedString::from(format!("group-empty-{}", row.group)))
                    .disabled(true)
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(crate::i18n::t("（空分组）", "(empty group)")),
                    ),
            );
        }

        let is_active = self.active.as_deref() == Some(row.id.as_str());
        // Palette rows are one line: name, then the detail muted beside it. A
        // palette is scanned vertically for one match; two stacked lines per
        // row halve how many fit in the same height for information that only
        // matters once the match is found.
        let body = if self.compact {
            h_flex()
                .gap_2()
                .items_center()
                .child(Icon::new(session_icon(&row)).size_4())
                .child(
                    div()
                        .flex()
                        .min_w_0()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(SharedString::from(row.name.clone())),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(row_detail(&row)),
                        ),
                )
        } else {
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .items_center()
                .child(Icon::new(session_icon(&row)).size_4())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(SharedString::from(row.name.clone())),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(row_detail(&row)),
                        ),
                )
        };

        // row_at and index_for_session are the only translation between the
        // toolkit's item space and the pure projection. Menus use stable ids.
        let id = row.id.clone();
        let menu_id = row.id.clone();
        // The menu's handlers only receive an `App`, which cannot reach this view's
        // context. So a row records what was asked for and the shell drains it at the
        // top of the next frame — the same shape the SFTP panel uses for its actions,
        // rather than a weak-entity hop that would need the handler to hold one.
        //
        // The clones happen inside the outer closure rather than outside it: that
        // closure is a `Fn` and runs for every menu build, while the handlers it hands
        // out are `move` closures that must own what they capture.
        let pending = self.pending.clone();
        let row_id = row.id.clone();
        // Where this session could go: the groups in the store that are not the one it is
        // already in, and the ungrouped entry when it is in a group at all. Computed here
        // rather than in the menu closure because the closure runs while the menu is being
        // built, which is no place to borrow the store.
        let move_targets: Vec<String> = {
            let store = self.store.borrow();
            // The session's *stored* group, not the heading it is shown under: "default"
            // is the display name for the ungrouped rows, so a menu built from the
            // heading would offer an ungrouped session a move into the group it is
            // already not in.
            let current = store
                .get(&row.id)
                .map(|session| session.group.trim().to_string())
                .unwrap_or_default();
            let mut targets = Vec::new();
            if !current.is_empty() {
                targets.push(String::new());
            }
            for group in store.groups() {
                // Reserved names are not destinations: `system` belongs to the built-in
                // local shells and `default` is the ungrouped heading, so offering either
                // would be a menu entry that refuses to work.
                if !group.trim().is_empty()
                    && group.trim() != current
                    && !crate::config::is_reserved_session_group(group.trim())
                {
                    targets.push(group.clone());
                }
            }
            targets
        };
        // The menu hangs off the row's own content, not a wrapper: `context_menu`
        // makes its element relative and gives it an absolutely-positioned
        // child, so attaching it to an empty div would be a menu on a
        // zero-sized target that can never be right-clicked. It is also where
        // the original keeps these actions — a row is a thing you click to
        // open, and permanent buttons would make every row a toolbar.
        //
        // Built-in local shells are part of the interface, not the user's
        // data: edit, duplicate, delete and move would all either refuse or
        // corrupt them, so their rows carry no menu at all.
        let row_content = div()
            .id(SharedString::from(format!("row-menu-{menu_id}")))
            .w_full()
            // Own the row padding too, so its blank edges are part of the menu target.
            .px_3()
            .py_1()
            .when(!self.compact, |this| {
                this.h(px(FULL_ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .overflow_hidden()
            })
            .child(body)
            // The id names the element for the hit test and the debug selector
            // makes it findable by a test. Two different things, and the file
            // panel has now taught me that six times.
            .debug_selector({
                let selector = format!("row-menu-{menu_id}");
                move || selector.clone()
            });
        let row_content: AnyElement = if row.builtin {
            row_content.into_any_element()
        } else {
            row_content
                .context_menu(move |menu, _, _| {
                    let for_edit = pending.clone();
                    let for_duplicate = pending.clone();
                    let for_delete = pending.clone();
                    let edit_id = row_id.clone();
                    let duplicate_id = row_id.clone();
                    let delete_id = row_id.clone();
                    let move_targets = move_targets.clone();
                    let mut menu = menu
                        .item(PopupMenuItem::new(crate::i18n::t("编辑", "Edit")).on_click(
                            move |_, _, _| {
                                *for_edit.borrow_mut() =
                                    Some(SessionListAction::Edit(edit_id.clone()));
                            },
                        ))
                        .item(
                            PopupMenuItem::new(crate::i18n::t("复制", "Duplicate")).on_click(
                                move |_, _, _| {
                                    *for_duplicate.borrow_mut() =
                                        Some(SessionListAction::Duplicate(duplicate_id.clone()));
                                },
                            ),
                        )
                        .separator();
                    // The groups this session could move to: every group the
                    // store knows except the one it is already in, plus the
                    // ungrouped entry — and that one only for a session that is in
                    // a group, because moving out of nothing is not an action.
                    for choice in move_targets {
                        let target = choice.clone();
                        let id = row_id.clone();
                        let pending = pending.clone();
                        let label = if choice.is_empty() {
                            crate::i18n::t("默认分组", "Default").to_string()
                        } else {
                            choice
                        };
                        menu = menu.item(PopupMenuItem::new(SharedString::from(label)).on_click(
                            move |_, _, _| {
                                *pending.borrow_mut() = Some(SessionListAction::Move {
                                    id: id.clone(),
                                    group: target.clone(),
                                });
                            },
                        ));
                    }
                    menu.separator().item(
                        PopupMenuItem::new(crate::i18n::t("删除", "Delete")).on_click(
                            move |_, _, _| {
                                *for_delete.borrow_mut() =
                                    Some(SessionListAction::Delete(delete_id.clone()));
                            },
                        ),
                    )
                })
                .into_any_element()
        };
        Some(
            ListItem::new(SharedString::from(format!("session-{id}")))
                .selected(is_active)
                .p_0()
                .when(!self.compact, |this| this.h(px(FULL_ROW_HEIGHT)))
                .child(row_content),
        )
    }

    /// Compact palettes keep equal-height, non-folding divider headings. The
    /// full sidebar has no section-header slots: every group uses item zero.
    fn render_section_header(
        &mut self,
        section: usize,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        if !self.compact {
            return None::<AnyElement>;
        }
        let span = self.sections.get(section)?;
        let group = span.group.clone();
        let muted = cx.theme().muted_foreground;
        // Palette headings are navigation-free dividers. They never inherit
        // sidebar folds, so every section has the same real header height.
        Some(
            div()
                .id(SharedString::from(format!("group-hdr-{group}")))
                .debug_selector({
                    let group = group.clone();
                    move || format!("group-hdr-{group}")
                })
                .w_full()
                .pt_1p5()
                .pb_0p5()
                .px_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(muted)
                .child(SharedString::from(group.clone()))
                .into_any_element(),
        )
    }
}

/// Build the rows for `query` from the store.
pub(crate) fn rows_for(
    store: &Rc<std::cell::RefCell<ConfigStore>>,
    query: &str,
) -> Vec<SessionRow> {
    let store = store.borrow();
    let builtin = crate::app::session_models::builtin_local_sessions(store.wsl_profiles());
    crate::app::session_models::session_rows(
        store.sessions(),
        store.groups(),
        store.collapsed_session_groups(),
        &builtin,
        query,
        store.session_group_order(),
    )
}

/// The session list, as the shell holds it.
pub(crate) struct SessionListView {
    /// The list widget's own state: the delegate, the scroll position and the search
    /// box. Everything about the list that persists between frames lives here.
    list: Entity<ListState<SessionListDelegate>>,
    /// External changes arrive without a Window. Reconcile at the next render,
    /// where rows, sections and toolkit selection can be updated together.
    reload_pending: bool,
}

impl SessionListView {
    pub(crate) fn new(
        store: Rc<std::cell::RefCell<ConfigStore>>,
        active: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_inner(store, active, false, window, cx)
    }

    /// The palette's shape: rows only, no header. The shell draws its own
    /// hint bar under the list.
    pub(crate) fn new_compact(
        store: Rc<std::cell::RefCell<ConfigStore>>,
        active: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_inner(store, active, true, window, cx)
    }

    fn new_inner(
        store: Rc<std::cell::RefCell<ConfigStore>>,
        active: Option<String>,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = SessionListDelegate::new(store.clone(), active, compact);
        // `searchable` gives the list its own search box: type to filter, Escape to
        // clear.
        let list = cx.new(|cx| ListState::new(delegate, window, cx).searchable(true));
        // The delegate learns the entity it lives in, so a section header's
        // click (which only receives an `App`) can rebuild the rows through it.
        list.update(cx, |state, cx| {
            state.delegate_mut().set_list(cx.entity().downgrade());
        });
        Self {
            list,
            reload_pending: false,
        }
    }

    /// How many rows are showing, so the header can say whether a search is filtering.
    fn len(&self, cx: &App) -> usize {
        self.list.read(cx).delegate().len()
    }

    /// Rebuild the rows from the store.
    ///
    /// Used after anything outside the list changes what should be shown — a session
    /// saved in the editor, a group collapsed by a menu — because the projection is the
    /// only thing that knows which rows exist and in what order. Filtering is preserved,
    /// since a rebuild that ignored the active search would quietly reveal rows the user
    /// had filtered away.
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.reload_pending = true;
        cx.notify();
    }

    /// Set when a session is opened. The row keeps its highlight after the session ends,
    /// because the reconnect path wants to know which one died.
    pub(crate) fn set_active(&mut self, active: Option<String>, cx: &mut Context<Self>) {
        self.list.update(cx, |state, cx| {
            state.delegate_mut().active = active;
            cx.notify();
        });
    }

    /// The list's own state, so the shell can subscribe to its `ListEvent`s.
    ///
    /// A click reaches the shell as an event rather than a callback because the delegate
    /// cannot hold a reference to the shell that owns it — that would be a cycle. The
    /// shell subscribes and resolves the index here.
    pub(crate) fn list(&self) -> &Entity<ListState<SessionListDelegate>> {
        &self.list
    }

    /// Put the caret in the search box, so a list that just opened — the
    /// quick-connect palette — can be typed into at once: the whole point of
    /// the palette is Ctrl+K, a few letters, Enter.
    pub(crate) fn focus_search(&self, window: &mut Window, cx: &mut App) {
        self.list.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Which session `ix` names, if it names one rather than a group heading.
    pub(crate) fn session_at(&self, ix: IndexPath, cx: &App) -> Option<String> {
        self.list.read(cx).delegate().connectable_at(ix)
    }

    /// Ask for a new session.
    ///
    /// An event rather than a callback because the view cannot reach the shell that
    /// owns it — that would be a reference cycle. The shell subscribes, the same way it
    /// already does for a row click, and owns the dialog this opens.
    fn request_new_session(&mut self, cx: &mut Context<Self>) {
        cx.emit(SessionListEvent::NewSession);
    }

    /// Take whatever a row's context menu asked for.
    pub(crate) fn take_action(&mut self, cx: &App) -> Option<SessionListAction> {
        self.list.read(cx).delegate().pending.borrow_mut().take()
    }
}

/// What the list asks the shell to do through its event channel.
///
/// Only the header button uses this; a row's context menu records into
/// [`SessionListAction`] instead, because its handler has no view context to emit from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionListEvent {
    /// The user pressed the new-session button.
    NewSession,
}

impl gpui_kit::EventEmitter<SessionListEvent> for SessionListView {}

/// What a row's context menu asked for.
///
/// Recorded rather than emitted because the menu handler only receives an `App`: it
/// cannot reach this view's context to emit, and the shell that must act is not
/// reachable from there either. The shell drains this at the top of the frame, which is
/// the same arrangement the SFTP panel uses for its buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionListAction {
    Edit(String),
    Duplicate(String),
    Delete(String),
    /// Move a session into a group, or out of every group when `group` is empty.
    Move {
        id: String,
        group: String,
    },
    /// Open the group manager, where the folders are made, renamed and deleted.
    ManageGroups,
}

impl Render for SessionListView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.reload_pending) {
            self.list
                .update(cx, |state, cx| reload_list(state, window, cx));
        }
        let theme = cx.theme();
        let shown = self.len(cx);
        let muted = theme.muted_foreground;
        let sidebar = theme.sidebar;
        // The header's menu records where a row's menu records, so the shell has one
        // place to look for "the list asked for something".
        let pending_for_menu = self.list.read(cx).delegate().pending.clone();

        // Palette mode: the search box first, the rows second, nothing else —
        // no count, no menu, no new button. The shell's hint bar under the
        // list carries what those said, and a right-click on a row still
        // opens the row's menu, so editing stays one gesture away.
        if self.list.read(cx).delegate().compact {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .bg(theme.background)
                .child(div().flex_1().overflow_hidden().child(
                    List::new(&self.list).search_placeholder(crate::i18n::t(
                        "输入以过滤会话…",
                        "Type to filter sessions…",
                    )),
                ));
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(sidebar)
            .child(
                // The header the original has: what the panel is, and the one action
                // that adds to it. The count beside the title is not decoration — a
                // bare label is read once and never again, while a number answers
                // whether a search is currently hiding anything.
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .child(
                        // No title: the column's tab above says which panel this is, and a
                        // header repeating it spends a line saying nothing new. What is
                        // worth the line is how many are shown, and the three things you
                        // can do to them.
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("session-search-focus")
                                    .icon(IconName::Search)
                                    .ghost()
                                    .tooltip(crate::i18n::t("搜索会话", "Search sessions"))
                                    .accessibility_label(crate::i18n::t(
                                        "搜索会话",
                                        "Search sessions",
                                    ))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.focus_search(window, cx)
                                    })),
                            )
                            .child(div().text_xs().text_color(muted).child(SharedString::from(
                                format!("{} {}", shown, crate::i18n::t("项", "items")),
                            ))),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                // One menu instead of three buttons. Managing folders,
                                // importing a config and exporting the list are things a
                                // user does occasionally; a header strip that shows every
                                // one of them permanently spends its width on the tools
                                // rather than on what they act on — and the original
                                // keeps its occasional actions behind menus for exactly
                                // that reason.
                                // DropdownButton is a split button: its action
                                // half does not open the menu. Attach the menu
                                // directly so the visible ellipsis is the trigger.
                                Button::new("list-more-trigger")
                                    .icon(IconName::Ellipsis)
                                    .ghost()
                                    .tooltip(crate::i18n::t("更多", "More"))
                                    .accessibility_label(crate::i18n::t("更多", "More"))
                                    .dropdown_menu_with_anchor(
                                        gpui_kit::Anchor::TopRight,
                                        move |menu, _, _| {
                                            let groups = pending_for_menu.clone();
                                            menu.item(
                                                PopupMenuItem::new(crate::i18n::t(
                                                    "管理分组",
                                                    "Manage groups",
                                                ))
                                                .on_click(move |_, window, _| {
                                                    *groups.borrow_mut() =
                                                        Some(SessionListAction::ManageGroups);
                                                    // The shell drains this queue while
                                                    // rendering; updating a RefCell alone
                                                    // does not schedule that frame.
                                                    window.refresh();
                                                }),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                Button::new("new-session")
                                    .icon(IconName::Plus)
                                    .ghost()
                                    .tooltip(crate::i18n::t("新建会话", "New session"))
                                    .accessibility_label(crate::i18n::t("新建会话", "New session"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.request_new_session(cx)),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    // The list's own search field comes with the toolkit's English
                    // placeholder, which was the one string in this window not ours. It is
                    // set here, the way every other input in this shell sets its own.
                    .child(
                        List::new(&self.list)
                            .search_placeholder(crate::i18n::t("搜索…", "Search…")),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::{Focusable as _, ScrollStrategy, TestAppContext, VisualTestContext};

    pub(super) fn session(id: &str, name: &str, group: &str) -> crate::config::Session {
        let mut session = crate::config::Session::new_empty();
        session.id = id.into();
        session.name = name.into();
        session.host = "127.0.0.1".into();
        session.group = group.into();
        session
    }

    /// No configuration load, database write, keyring or real credentials.
    pub(super) fn store(
        sessions: Vec<crate::config::Session>,
    ) -> Rc<std::cell::RefCell<ConfigStore>> {
        let cache = crate::config::ConfigFile {
            sessions,
            ..Default::default()
        };
        Rc::new(std::cell::RefCell::new(ConfigStore {
            path: Default::default(),
            backup_dir: None,
            key: [0; 32],
            keyring_enabled: false,
            saved_state: std::sync::Mutex::new(crate::config::SavedState::of_cache(&cache)).into(),
            cache,
        }))
    }

    pub(super) fn open(
        cx: &mut TestAppContext,
        store: Rc<std::cell::RefCell<ConfigStore>>,
        compact: bool,
    ) -> (Entity<SessionListView>, &mut VisualTestContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(move |window, cx| {
            SessionListView::new_inner(store, None, compact, window, cx)
        });
        draw(cx);
        (view, cx)
    }

    pub(super) fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
    }

    pub(super) fn reload(view: &Entity<SessionListView>, cx: &mut VisualTestContext) {
        cx.update(|_, cx| view.update(cx, |view, cx| view.reload(cx)));
        draw(cx);
    }

    pub(super) fn list(
        view: &Entity<SessionListView>,
        cx: &mut VisualTestContext,
    ) -> Entity<ListState<SessionListDelegate>> {
        cx.update(|_, cx| view.read(cx).list.clone())
    }

    pub(super) fn index(
        list: &Entity<ListState<SessionListDelegate>>,
        id: &str,
        cx: &mut VisualTestContext,
    ) -> IndexPath {
        cx.update(|_, cx| {
            list.read(cx)
                .delegate()
                .index_for_session(id)
                .expect("visible fixture session")
        })
    }

    fn assert_projection(
        list: &Entity<ListState<SessionListDelegate>>,
        cx: &mut VisualTestContext,
    ) {
        cx.update(|_, cx| {
            let delegate = list.read(cx).delegate();
            let expected = rows_for(&delegate.store, &delegate.query);
            assert_eq!(delegate.rows.len(), expected.len());
            assert_eq!(
                delegate.sections.iter().map(|span| span.len).sum::<usize>(),
                expected.len()
            );
            for (section, span) in delegate.sections.iter().enumerate() {
                if !delegate.compact {
                    assert!(delegate
                        .row_at(IndexPath::new(0).section(section))
                        .is_none());
                    assert!(delegate
                        .connectable_at(IndexPath::new(0).section(section))
                        .is_none());
                }
                for row in 0..span.len {
                    let actual = &delegate.rows[span.start + row];
                    let expected = &expected[span.start + row];
                    assert_eq!(
                        (&actual.id, &actual.name, &actual.group),
                        (&expected.id, &expected.name, &expected.group)
                    );
                    if !span.collapsed && is_connectable(expected) {
                        let ix =
                            IndexPath::new(row + usize::from(!delegate.compact)).section(section);
                        assert_eq!(
                            delegate.row_at(ix).map(|row| row.id.as_str()),
                            Some(expected.id.as_str())
                        );
                        assert_eq!(delegate.index_for_session(&expected.id), Some(ix));
                    }
                }
                assert!(
                    delegate
                        .row_at(IndexPath::new(delegate.items_count(section, cx)).section(section))
                        .is_none(),
                    "an invalid row must not spill into the next section"
                );
            }
        });
    }

    /// The list draws every saved session in this small, deterministic fixture.
    /// Larger lists deliberately virtualize off-screen rows (covered below).
    #[gpui_kit::gpui::test]
    fn every_saved_session_gets_a_row(cx: &mut TestAppContext) {
        let store = store(vec![
            session("first", "First", ""),
            session("second", "Second", "ops"),
        ]);
        let (_, cx) = open(cx, store, false);
        assert!(cx.debug_bounds("row-menu-first").is_some());
        assert!(cx.debug_bounds("row-menu-second").is_some());
    }

    #[gpui_kit::gpui::test]
    fn reload_adds_rows_to_existing_and_new_sections_without_search(cx: &mut TestAppContext) {
        let store = store(vec![session("first", "First", "")]);
        let (view, cx) = open(cx, store.clone(), false);
        store.borrow_mut().upsert(session("second", "Second", ""));
        reload(&view, cx);
        assert!(
            cx.debug_bounds("row-menu-second").is_some(),
            "saving into an existing section must render immediately"
        );
        store.borrow_mut().upsert(session("third", "Third", "ops"));
        reload(&view, cx);
        assert!(
            cx.debug_bounds("row-menu-third").is_some(),
            "saving into a new section must render immediately"
        );
        assert_projection(&list(&view, cx), cx);
    }

    #[gpui_kit::gpui::test]
    fn reload_delete_rename_and_regroup_preserve_session_identity_and_focus(
        cx: &mut TestAppContext,
    ) {
        let store = store(vec![
            session("first", "First", "a"),
            session("second", "Second", "a"),
            session("third", "Third", "z"),
        ]);
        let (view, cx) = open(cx, store.clone(), false);
        let list = list(&view, cx);
        let selected = index(&list, "second", cx);
        let focus = cx.update(|window, cx| {
            view.update(cx, |view, cx| view.set_active(Some("second".into()), cx));
            list.update(cx, |state, cx| {
                state.set_selected_index(Some(selected), window, cx);
                state.set_right_clicked_index(Some(selected), window, cx);
                state.focus(window, cx);
                state.focus_handle(cx)
            })
        });
        // Removing the first section row shifts the selected row's numeric index.
        store.borrow_mut().remove("first");
        reload(&view, cx);
        assert_eq!(
            cx.update(|_, cx| list.read(cx).selected_index()),
            Some(index(&list, "second", cx))
        );
        store.borrow_mut().upsert(session("second", "Renamed", "z"));
        store.borrow_mut().rename_group("z", "renamed-group".into());
        reload(&view, cx);
        assert_projection(&list, cx);
        cx.update(|window, cx| {
            let state = list.read(cx);
            let selected = state.delegate().index_for_session("second");
            assert_eq!(state.selected_index(), selected);
            assert_eq!(state.right_clicked_index(), selected);
            assert_eq!(state.delegate().active.as_deref(), Some("second"));
            assert!(focus.is_focused(window));
        });
        // Deleting the selected session must not transfer Enter to its neighbour.
        store.borrow_mut().remove("second");
        reload(&view, cx);
        assert_projection(&list, cx);
        cx.update(|_, cx| {
            assert!(list.read(cx).selected_index().is_none());
            assert!(list.read(cx).right_clicked_index().is_none());
        });
    }

    #[gpui_kit::gpui::test]
    fn reload_keeps_filter_and_collapsed_group_state(cx: &mut TestAppContext) {
        let store = store(vec![
            session("keep", "Keep", "ops"),
            session("hidden", "Hidden", "ops"),
        ]);
        store.borrow_mut().set_session_group_collapsed("ops", true);
        let (view, cx) = open(cx, store.clone(), false);
        let list = list(&view, cx);
        cx.update(|window, cx| list.update(cx, |state, cx| state.set_query("keep", window, cx)));
        cx.run_until_parked();
        draw(cx);
        let selected = index(&list, "keep", cx);
        cx.update(|window, cx| {
            list.update(cx, |state, cx| {
                state.set_selected_index(Some(selected), window, cx)
            })
        });
        store
            .borrow_mut()
            .upsert(session("added", "Keep added", "ops"));
        reload(&view, cx);
        assert!(cx.debug_bounds("row-menu-added").is_some());
        assert!(cx.debug_bounds("row-menu-hidden").is_none());
        assert_projection(&list, cx);
        assert_eq!(
            cx.update(|_, cx| list.read(cx).delegate().query.clone()),
            "keep"
        );
        // Rename out of the active query, then restore the folded unfiltered view.
        store
            .borrow_mut()
            .upsert(session("keep", "No match", "ops"));
        reload(&view, cx);
        assert!(cx.update(|_, cx| list.read(cx).selected_index()).is_none());
        cx.update(|window, cx| list.update(cx, |state, cx| state.set_query("", window, cx)));
        cx.run_until_parked();
        draw(cx);
        cx.update(|_, cx| {
            let delegate = list.read(cx).delegate();
            let section = delegate
                .sections
                .iter()
                .position(|span| span.group == "ops")
                .unwrap();
            assert!(delegate.sections[section].collapsed);
            assert_eq!(delegate.items_count(section, cx), 1);
            assert!(delegate
                .connectable_at(IndexPath::new(0).section(section))
                .is_none());
        });
    }

    #[gpui_kit::gpui::test]
    fn reload_updates_virtualized_tail_without_resetting_scroll(cx: &mut TestAppContext) {
        let sessions = (0..100)
            .map(|n| session(&format!("item-{n}"), &format!("Item {n}"), "ops"))
            .collect();
        let store = store(sessions);
        let (view, cx) = open(cx, store.clone(), false);
        let list = list(&view, cx);
        assert!(
            cx.debug_bounds("row-menu-item-99").is_none(),
            "fixture tail must start outside the viewport"
        );
        let tail = index(&list, "item-99", cx);
        cx.update(|window, cx| {
            list.update(cx, |state, cx| {
                state.scroll_to_item(tail, ScrollStrategy::Top, window, cx)
            })
        });
        draw(cx);
        assert!(cx.debug_bounds("row-menu-item-99").is_some());
        let offset = cx.update(|_, cx| list.read(cx).scroll_handle().base_handle().offset());
        store
            .borrow_mut()
            .upsert(session("added-tail", "Added tail", "ops"));
        reload(&view, cx);
        assert_projection(&list, cx);
        assert_eq!(
            cx.update(|_, cx| list.read(cx).scroll_handle().base_handle().offset()),
            offset
        );
        let tail = index(&list, "added-tail", cx);
        cx.update(|window, cx| {
            list.update(cx, |state, cx| {
                state.scroll_to_item(tail, ScrollStrategy::Top, window, cx)
            })
        });
        draw(cx);
        assert!(
            cx.debug_bounds("row-menu-added-tail").is_some(),
            "new tail must be addressable through the virtual list"
        );
    }
}

#[cfg(test)]
#[path = "session_list_group_tests.rs"]
mod group_tests;

#[cfg(test)]
mod context_menu_tests {
    use super::*;
    use gpui_kit::gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
    use gpui_kit::{point, px};

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
    }

    /// A synthetic in-memory fixture, independent of the user's saved profile.
    fn check_row_menu(cx: &mut TestAppContext, compact: bool) {
        cx.update(gpui_kit::init);
        let mut session = crate::config::Session::new_empty();
        session.id = "menu-fixture".into();
        session.name = "Menu fixture".into();
        session.host = "127.0.0.1".into();
        let mut other = session.clone();
        other.id = "other-fixture".into();
        other.name = "Other fixture".into();
        let cache = crate::config::ConfigFile {
            sessions: vec![other, session],
            ..Default::default()
        };
        let store = Rc::new(std::cell::RefCell::new(ConfigStore {
            path: Default::default(),
            backup_dir: None,
            key: [0; 32],
            keyring_enabled: false,
            saved_state: std::sync::Mutex::new(crate::config::SavedState::of_cache(&cache)).into(),
            cache,
        }));
        let (view, cx) = cx.add_window_view(move |window, cx| {
            SessionListView::new_inner(store, None, compact, window, cx)
        });
        draw(cx);
        cx.update(|window, cx| view.update(cx, |view, cx| view.focus_search(window, cx)));
        // Keep A selected while right-clicking B, so action routing cannot
        // accidentally use keyboard selection instead of the clicked row.
        let list = cx.update(|_, cx| view.read(cx).list.clone());
        let selected = cx.update(|window, cx| {
            list.update(cx, |state, cx| {
                let selected = state.delegate().index_for_session("other-fixture").unwrap();
                state.set_selected_index(Some(selected), window, cx);
                selected
            })
        });
        draw(cx);
        let bounds = cx.debug_bounds("row-menu-menu-fixture").expect("saved row");
        assert!(bounds.size.width > px(100.));
        assert!(
            bounds.size.height > px(10.),
            "a context-menu target must contain the visible row"
        );
        // The text/icon area and the blank top/right padding must all work.
        for (position, keys, expected) in [
            (
                bounds.center(),
                "down enter",
                SessionListAction::Edit("menu-fixture".into()),
            ),
            (
                point(bounds.right() - px(1.), bounds.bottom() - px(1.)),
                "down down enter",
                SessionListAction::Duplicate("menu-fixture".into()),
            ),
            (
                point(bounds.left() + px(1.), bounds.top() + px(1.)),
                "down down down enter",
                SessionListAction::Delete("menu-fixture".into()),
            ),
        ] {
            cx.simulate_mouse_move(position, None, Modifiers::default());
            cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
            cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
            draw(cx);
            assert!(
                cx.update(|_, cx| view.update(cx, |view, cx| view.take_action(cx)))
                    .is_none(),
                "opening the menu alone must not execute an action"
            );
            cx.simulate_keystrokes(keys);
            draw(cx);
            let action = cx.update(|_, cx| view.update(cx, |view, cx| view.take_action(cx)));
            assert_eq!(action, Some(expected));
            assert_eq!(
                cx.update(|_, cx| list.read(cx).selected_index()),
                Some(selected)
            );
        }
    }

    #[gpui_kit::gpui::test]
    fn full_session_row_context_menu_covers_content_and_padding(cx: &mut TestAppContext) {
        check_row_menu(cx, false);
    }

    #[gpui_kit::gpui::test]
    fn compact_session_row_context_menu_covers_content_and_padding(cx: &mut TestAppContext) {
        check_row_menu(cx, true);
    }
}
