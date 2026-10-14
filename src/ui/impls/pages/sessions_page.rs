//! The sessions page: the saved connections and the built-in local shells.
//!
//! A row's click is a page switch as much as a connect: the shell opens the
//! session in the terminal page and brings that page forward, which is the
//! behaviour the old left-column tab could not express — the list was where
//! the terminal should be, and the best it could do was shrink itself.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::{
    h_flex, list::ListEvent, v_flex, ActiveTheme as _, WindowExt as _,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, Entity, FontWeight, MouseDownEvent, MouseMoveEvent, SharedString,
    Subscription, Window,
};

use crate::config::ConfigStore;
use crate::ui::{SessionListAction, SessionListEvent, SessionListView};

/// What the page asks the shell to do. The list's own [`SessionListAction`] is
/// folded in here — the page owns the subscription that catches its events, so
/// the shell drains one queue and not two channels of different shapes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionsAction {
    /// A row was confirmed: open this session in the terminal page.
    Open(String),
    /// The header's new-session button.
    NewSession,
    /// A row's context menu, verbatim.
    Edit(String),
    Duplicate(String),
    Delete(String),
    Move { id: String, group: String },
    ManageGroups,
    /// The management pane's doors: import from a file, export to one.
    ImportConfig,
    ExportConfig,
}

pub(crate) struct SessionsPage {
    list: Entity<SessionListView>,
    store: Rc<RefCell<ConfigStore>>,
    action: Rc<RefCell<Option<SessionsAction>>>,
    /// A list-column drag in flight: the x the press landed at, and the width
    /// the column had then. The delta between the two is the resize, so the
    /// drag never needs to know where the column sits on screen.
    width_drag: Option<(f32, f32)>,
    /// A click is an event on the list widget's state, and a subscription is
    /// what turns it into an entry in the queue above. Dropped subscriptions
    /// leave a list that renders and does nothing.
    _confirm_subscription: Subscription,
    _new_session_subscription: Subscription,
}

impl SessionsPage {
    pub(crate) fn new(
        store: Rc<RefCell<ConfigStore>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = cx.new(|cx| SessionListView::new(store.clone(), None, window, cx));
        let action = Rc::new(RefCell::new(None));

        // The row's index travels out through the queue and is resolved by the
        // shell, which is what can open a session — the page cannot.
        let confirm_action = action.clone();
        let confirm_list = list.clone();
        let list_state = list.read(cx).list().clone();
        let _confirm_subscription = cx.subscribe_in(
            &list_state,
            window,
            move |_: &mut Self, _state, event: &ListEvent, _, cx| {
                let ListEvent::Confirm(ix) = event else {
                    return;
                };
                let Some(session_id) = confirm_list.read(cx).session_at(*ix, cx) else {
                    // A group heading, not a session. Nothing to open.
                    return;
                };
                *confirm_action.borrow_mut() = Some(SessionsAction::Open(session_id));
            },
        );

        let new_action = action.clone();
        let _new_session_subscription = cx.subscribe_in(
            &list,
            window,
            move |_: &mut Self, _list, event: &SessionListEvent, _, _| {
                if *event == SessionListEvent::NewSession {
                    *new_action.borrow_mut() = Some(SessionsAction::NewSession);
                }
            },
        );

        Self {
            list,
            store,
            action,
            width_drag: None,
            _confirm_subscription,
            _new_session_subscription,
        }
    }

    /// The active-session highlight, pointed at whatever the terminal page is
    /// showing.
    pub(crate) fn set_active(&mut self, active: Option<String>, cx: &mut Context<Self>) {
        self.list.update(cx, |list, cx| list.set_active(active, cx));
    }

    /// Rebuild the rows from the store, after anything outside the list changed
    /// it — a session saved in the editor, a group renamed, an import.
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.list.update(cx, |list, cx| list.reload(cx));
    }

    /// The next thing the page is asking for, if any. The list's own menu
    /// actions are folded into the same queue, so the shell drains one place.
    pub(crate) fn take_action(&mut self, cx: &mut Context<Self>) -> Option<SessionsAction> {
        if let Some(pending) = self.action.borrow_mut().take() {
            return Some(pending);
        }
        let menu = self.list.update(cx, |list, cx| list.take_action(cx))?;
        Some(match menu {
            SessionListAction::Edit(id) => SessionsAction::Edit(id),
            SessionListAction::Duplicate(id) => SessionsAction::Duplicate(id),
            SessionListAction::Delete(id) => SessionsAction::Delete(id),
            SessionListAction::Move { id, group } => SessionsAction::Move { id, group },
            SessionListAction::ManageGroups => SessionsAction::ManageGroups,
        })
    }
}

impl Render for SessionsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        // The stored width, narrowed to what this window can actually spare:
        // the management pane keeps a 360 px floor before the list takes its
        // share. The narrowed value is not persisted — a small window is a
        // moment, not a preference (#98).
        let stored = self.store.borrow().session_list_width();
        let viewport = f32::from(window.viewport_size().width);
        let width = stored.min((viewport - 360.0).max(240.0)).max(240.0);
        let dragging = self.width_drag.is_some();

        // The full-page layout splits the two jobs this page has: the list on
        // the left is for finding and connecting (a click connects, same as
        // the palette), and the pane on the right is for everything
        // administrative — creating, importing, exporting, grouping — which
        // the narrow column could never house.
        let (session_count, group_count) = {
            let store = self.store.borrow();
            let sessions = store.sessions();
            // The manager's own definition of a group — explicit folders
            // included — so the counts on this page and in the manager's card
            // are the same number, empty folders and all (#98).
            let groups = crate::config::named_display_groups(store.groups(), sessions);
            (sessions.len(), groups.len())
        };

        h_flex()
            .size_full()
            .overflow_hidden()
            // The drag's motion and release are read here rather than on the
            // 6-px handle: a strip that narrow never receives a press in this
            // window (measured when the pane splitter was built), while the
            // area receives both and knows what the drag means.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let Some((press_x, start_width)) = this.width_drag else {
                    return;
                };
                let delta = f32::from(event.position.x) - press_x;
                let next = (start_width + delta).clamp(240.0, 640.0);
                if this.store.borrow().session_list_width() != next {
                    this.store.borrow_mut().set_session_list_width(next);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.width_drag.take().is_none() {
                        return;
                    }
                    // Persist the settled width; a save that fails rolls the
                    // stored value back so the next frame shows what the file
                    // actually says rather than a width only memory remembers.
                    // The pre-save value, for a rollback the file's own
                    // keeper would disagree with.
                    let rollback = this.store.borrow().cache.session_list_width;
                    if this.store.borrow_mut().save().is_err() {
                        this.store.borrow_mut().set_session_list_width(rollback);
                        tracing::warn!("could not save the session-list width; rolled back");
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(width))
                    .h_full()
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(border)
                    .overflow_hidden()
                    .child(self.list.clone()),
            )
            .child(
                // The handle itself: a hairline that reads as the column's
                // edge, wide enough to press. A modal on top owns the pointer,
                // so a press while one is open never starts a drag.
                div()
                    .id("sessions-width-handle")
                    .w(px(6.0))
                    .h_full()
                    .flex_shrink_0()
                    .cursor_col_resize()
                    .when(dragging, |this| this.bg(cx.theme().accent.opacity(0.4)))
                    .hover(|this| this.bg(cx.theme().muted))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            if window.has_active_dialog(cx) {
                                return;
                            }
                            this.width_drag = Some((
                                f32::from(event.position.x),
                                this.store.borrow().session_list_width(),
                            ));
                        }),
                    ),
            )
            .child(
                v_flex()
                    .id("sessions-management")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .p_8()
                    .gap_4()
                    .child(self.management_header(session_count, group_count, cx))
                    .child(self.management_actions(cx)),
            )
    }
}

impl SessionsPage {
    /// The management pane's heading: what this page administers, with the
    /// counts as a one-line inventory.
    fn management_header(
        &self,
        session_count: usize,
        group_count: usize,
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let muted_fg = cx.theme().muted_foreground;
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(crate::i18n::t(
                        "会话管理",
                        "Session management",
                    ))),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted_fg)
                    .child(SharedString::from(format!(
                        "{} {} · {} {} · {}",
                        session_count,
                        crate::i18n::t("个会话", "sessions"),
                        group_count,
                        crate::i18n::t("个分组", "groups"),
                        crate::i18n::t(
                            "Ctrl+K 随时快速连接",
                            "Ctrl+K connects from anywhere"
                        ),
                    ))),
            )
            .into_any_element()
    }

    /// The management pane's actions: one bordered entry per job, each with a
    /// one-line description of what it opens.
    fn management_actions(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;

        let queue = self.action.clone();
        let entry = |id: &'static str,
                     icon: gpui_kit::assets::IconName,
                     label: &'static str,
                     description: &'static str,
                     action: SessionsAction| {
            let queued = Rc::new(RefCell::new(Some(action)));
            let queue = queue.clone();
            div()
                .id(SharedString::from(id))
                .w_full()
                .cursor_pointer()
                .rounded_md()
                .border_1()
                .border_color(border)
                .px_3()
                .py_2p5()
                .hover(|this| this.bg(cx.theme().accent))
                .on_click(move |_, _, _| {
                    if let Some(action) = queued.borrow_mut().take() {
                        *queue.borrow_mut() = Some(action);
                    }
                })
                .child(
                    h_flex()
                        .items_center()
                        .gap_3()
                        .child(div().text_color(muted_fg).child(icon))
                        .child(
                            v_flex()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(SharedString::from(label)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted_fg)
                                        .child(SharedString::from(description)),
                                ),
                        ),
                )
                .into_any_element()
        };

        v_flex()
            .gap_2()
            .children(vec![
            entry(
                "mgmt-new",
                gpui_kit::assets::IconName::FilePlus,
                crate::i18n::t("新建连接", "New connection"),
                crate::i18n::t(
                    "创建一个会话配置：SSH、Telnet、串口或本地终端。",
                    "Create a session: SSH, Telnet, serial or local.",
                ),
                SessionsAction::NewSession,
            ),
            entry(
                "mgmt-import",
                gpui_kit::assets::IconName::Download,
                crate::i18n::t("导入配置", "Import config"),
                crate::i18n::t(
                    "从 JSON 导出或 OpenSSH 配置文件导入主机。",
                    "Import hosts from a JSON export or an OpenSSH config.",
                ),
                SessionsAction::ImportConfig,
            ),
            entry(
                "mgmt-export",
                gpui_kit::assets::IconName::Upload,
                crate::i18n::t("导出配置", "Export config"),
                crate::i18n::t(
                    "把全部会话导出为一个 JSON 文件。",
                    "Export every session to a JSON file.",
                ),
                SessionsAction::ExportConfig,
            ),
            entry(
                "mgmt-groups",
                gpui_kit::assets::IconName::FolderCog,
                crate::i18n::t("分组管理", "Group management"),
                crate::i18n::t(
                    "创建、重命名和删除会话分组。",
                    "Create, rename and delete session groups.",
                ),
                SessionsAction::ManageGroups,
            ),
        ])
        .into_any_element()
    }
}
