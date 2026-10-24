//! The sessions page: the saved connections and the built-in local shells.
//!
//! A row's click is a page switch as much as a connect: the shell opens the
//! session in the terminal page and brings that page forward, which is the
//! behaviour the old left-column tab could not express — the list was where
//! the terminal should be, and the best it could do was shrink itself.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::component::{h_flex, list::ListEvent, v_flex, ActiveTheme as _, WindowExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    canvas, div, px, DispatchPhase, Entity, FontWeight, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, SharedString, Subscription, Window,
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
    Move {
        id: String,
        group: String,
    },
    ManageGroups,
    /// The management pane's doors: import from a file, export to one.
    ImportConfig,
    ExportConfig,
}

pub(crate) struct SessionsPage {
    list: Entity<SessionListView>,
    list_bounds: Rc<Cell<(f32, f32)>>,
    resize: Option<(f32, f32)>,
    preview_width: Option<f32>,
    store: Rc<RefCell<ConfigStore>>,
    action: Rc<RefCell<Option<SessionsAction>>>,
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
            list_bounds: Rc::new(Cell::new((0.0, 0.0))),
            resize: None,
            preview_width: None,
            store,
            action,
            _confirm_subscription,
            _new_session_subscription,
        }
    }

    /// Persist a completed gesture once. A failed save restores the preference
    /// and leaves the store's existing error channel for the shell to display.
    fn finish_resize(&mut self) {
        if self.resize.take().is_none() {
            return;
        }
        let Some(width) = self.preview_width.take() else {
            return;
        };
        let mut store = self.store.borrow_mut();
        let before = store.cache.sessions_sidebar_width;
        store.set_sessions_sidebar_width(width);
        if let Err(error) = store.save() {
            store.cache.sessions_sidebar_width = before;
            tracing::warn!("could not save session list width: {error:#}");
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

/// Count explicit empty folders as well as groups inferred from saved sessions.
/// Built-in system/ungrouped headings are not user-created folders.
fn management_counts(store: &ConfigStore) -> (usize, usize) {
    let sessions = store.sessions();
    let groups: std::collections::HashSet<&str> = sessions
        .iter()
        .map(|session| session.group.trim())
        .chain(store.groups().iter().map(|group| group.trim()))
        .filter(|group| !group.is_empty() && !matches!(*group, "system" | "default"))
        .collect();
    (sessions.len(), groups.len())
}

/// Preserve space for the management pane without rewriting the saved preference
/// when a smaller window temporarily constrains the layout.
fn fitted_list_width(preferred: f32, available: f32) -> f32 {
    let preferred = if preferred.is_finite() {
        preferred.max(240.0)
    } else {
        380.0
    };
    preferred.min(super::super::tokens::sidebar_width_limit(available))
}

impl Render for SessionsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let available = f32::from(window.viewport_size().width) - super::super::tokens::RAIL_WIDTH;
        let width = fitted_list_width(
            self.preview_width
                .unwrap_or_else(|| self.store.borrow().sessions_sidebar_width()),
            available,
        );
        let bounds = self.list_bounds.clone();
        let weak = cx.entity().downgrade();

        // The full-page layout splits the two jobs this page has: the list on
        // the left is for finding and connecting (a click connects, same as
        // the palette), and the pane on the right is for everything
        // administrative — creating, importing, exporting, grouping — which
        // the narrow column could never house.
        let (session_count, group_count) = management_counts(&self.store.borrow());

        h_flex()
            .size_full()
            .relative()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if window.has_active_dialog(cx) {
                        return;
                    }
                    let (left, right) = this.list_bounds.get();
                    let x = f32::from(event.position.x);
                    if right > left && (x - right).abs() <= 5.0 {
                        this.resize = Some((x, right - left));
                        this.preview_width = Some(right - left);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let on_move = weak.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            let _ = on_move.update(cx, |this, cx| {
                                let Some((start, width)) = this.resize else {
                                    return;
                                };
                                if event.pressed_button != Some(MouseButton::Left)
                                    || window.has_active_dialog(cx)
                                {
                                    // A release outside this window must not leave a sticky drag.
                                    this.resize = None;
                                    this.preview_width = None;
                                } else {
                                    let available = f32::from(window.viewport_size().width)
                                        - super::super::tokens::RAIL_WIDTH;
                                    this.preview_width = Some(fitted_list_width(
                                        width + f32::from(event.position.x) - start,
                                        available,
                                    ));
                                }
                                cx.notify();
                            });
                        });
                        let on_up = weak.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase != DispatchPhase::Capture || event.button != MouseButton::Left
                            {
                                return;
                            }
                            let _ = on_up.update(cx, |this, cx| {
                                if window.has_active_dialog(cx) {
                                    this.resize = None;
                                    this.preview_width = None;
                                } else {
                                    if let Some((start, width)) = this.resize {
                                        let available = f32::from(window.viewport_size().width)
                                            - super::super::tokens::RAIL_WIDTH;
                                        this.preview_width = Some(fitted_list_width(
                                            width + f32::from(event.position.x) - start,
                                            available,
                                        ));
                                    }
                                    this.finish_resize();
                                }
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .id("sessions-sidebar")
                    .debug_selector(|| "sessions-sidebar".to_string())
                    .relative()
                    .w(px(width))
                    .h_full()
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(border)
                    .overflow_hidden()
                    .child(
                        canvas(
                            move |rect, _, _| {
                                bounds.set((
                                    f32::from(rect.origin.x),
                                    f32::from(rect.origin.x + rect.size.width),
                                ));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(self.list.clone())
                    .child(
                        div()
                            .id("sessions-sidebar-resize")
                            .absolute()
                            .right_0()
                            .top_0()
                            .w(px(5.0))
                            .h_full()
                            .cursor_col_resize()
                            .hover(|bar| bar.bg(border)),
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
                        crate::i18n::t("Ctrl+K 随时快速连接", "Ctrl+K connects from anywhere"),
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
                .on_click(move |_, window, _| {
                    if let Some(action) = queued.borrow_mut().take() {
                        *queue.borrow_mut() = Some(action);
                        // The shell drains this intent at the next frame. A
                        // queue write alone must not wait for an unrelated click.
                        window.refresh();
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

#[cfg(test)]
mod layout_tests {
    use super::fitted_list_width;

    #[test]
    fn sessions_width_preserves_management_room_and_rejects_nonfinite() {
        assert_eq!(fitted_list_width(500.0, 1000.0), 500.0);
        assert_eq!(fitted_list_width(1100.0, 1800.0), 1100.0);
        assert_eq!(fitted_list_width(f32::MAX, 1800.0), 1520.0);
        assert_eq!(fitted_list_width(640.0, 800.0), 520.0);
        assert_eq!(fitted_list_width(640.0, 100.0), 0.0);
        assert_eq!(fitted_list_width(f32::NAN, 1000.0), 380.0);
        assert_eq!(fitted_list_width(380.0, f32::INFINITY), 0.0);
        assert_eq!(fitted_list_width(-100.0, 1000.0), 240.0);
    }
    use super::*;
    use gpui_kit::gpui::{TestAppContext, VisualTestContext};

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
    }

    #[gpui_kit::gpui::test]
    fn sessions_sidebar_drag_only_persists_on_release_and_reopens(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let cache = crate::config::ConfigFile::default();
        let store = Rc::new(RefCell::new(ConfigStore {
            path: directory.path().join("sessions.db"),
            backup_dir: None,
            saved_state: std::sync::Mutex::new(crate::config::SavedState::of_cache(&cache)).into(),
            cache,
            key: [7; 32],
            keyring_enabled: false,
        }));
        assert_eq!(management_counts(&store.borrow()), (0, 0));
        store.borrow_mut().cache.groups = vec!["QA Alpha".into(), "QA Beta".into()];
        assert_eq!(management_counts(&store.borrow()), (0, 2));
        let shared = store.clone();
        let mut page = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| SessionsPage::new(shared, window, cx));
            page = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let page = page.unwrap();
        cx.simulate_resize(gpui_kit::size(px(1280.0), px(800.0)));
        draw(cx);
        let (left, right) = page.read_with(cx, |page, _| page.list_bounds.get());
        assert!(
            (right - left - 380.0).abs() <= 1.0,
            "measured list bounds {left}..{right}"
        );
        let start = gpui_kit::point(px(right - 1.0), px(200.0));
        let end = gpui_kit::point(start.x + px(60.0), start.y);
        cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
        draw(cx);
        cx.simulate_mouse_move(end, MouseButton::Left, Default::default());
        draw(cx);
        assert!(page.read_with(cx, |page, _| page.preview_width.unwrap()) > 430.0);
        assert_eq!(store.borrow().sessions_sidebar_width(), 380.0);
        assert!(!store.borrow().path.exists());
        cx.simulate_mouse_up(end, MouseButton::Left, Default::default());
        draw(cx);
        assert!(store.borrow().sessions_sidebar_width() > 430.0);
        assert!(store.borrow().path.exists());
        let connection = rusqlite::Connection::open(&store.borrow().path).unwrap();
        let settings: String = connection
            .query_row("SELECT value FROM meta WHERE key='settings'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
        assert!(settings["sessions_sidebar_width"].as_f64().unwrap() > 430.0);
        assert!(page.read_with(cx, |page, _| page.resize.is_none()
            && page.preview_width.is_none()));
        let before = store.borrow().sessions_sidebar_width();
        // A release outside the window cancels the next stale motion.
        let (_, right) = page.read_with(cx, |page, _| page.list_bounds.get());
        let start = gpui_kit::point(px(right - 1.0), px(200.0));
        cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
        draw(cx);
        cx.simulate_mouse_move(start, None, Default::default());
        draw(cx);
        assert_eq!(store.borrow().sessions_sidebar_width(), before);
        assert!(page.read_with(cx, |page, _| page.resize.is_none()));
        // A blocked profile write must visibly roll back, not claim a save.
        store.borrow_mut().path = directory.path().to_path_buf();
        page.update(cx, |page, _| {
            page.resize = Some((0.0, before));
            page.preview_width = Some(600.0);
            page.finish_resize();
        });
        assert_eq!(store.borrow().sessions_sidebar_width(), before);
        assert!(store.borrow().persistence_error().is_some());
    }

    #[gpui_kit::gpui::test]
    fn sessions_sidebar_wide_drag_restores_after_viewport_resize(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let cache = crate::config::ConfigFile::default();
        let store = Rc::new(RefCell::new(ConfigStore {
            path: directory.path().join("sessions.db"), backup_dir: None,
            saved_state: std::sync::Mutex::new(crate::config::SavedState::of_cache(&cache)).into(),
            cache, key: [7; 32], keyring_enabled: false,
        }));
        let shared = store.clone();
        let mut page = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| SessionsPage::new(shared, window, cx));
            page = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let page = page.unwrap();
        for scale in [1.0, 1.5, 2.0] {
            cx.simulate_scale_factor_change(scale);
            cx.simulate_resize(gpui_kit::size(px(1920.), px(900.)));
            draw(cx);
            let (left, right) = page.read_with(cx, |page, _| page.list_bounds.get());
            let start = gpui_kit::point(px(right - 1.), px(200.));
            let end = gpui_kit::point(start.x + px(1100. - (right - left)), start.y);
            cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
            draw(cx);
            assert!(page.read_with(cx, |page, _| page.resize.is_some()), "rendered divider starts the actual drag");
            cx.simulate_mouse_move(end, MouseButton::Left, Default::default());
            draw(cx);
            cx.simulate_mouse_up(end, MouseButton::Left, Default::default());
            draw(cx);
            assert!((store.borrow().sessions_sidebar_width() - 1100.).abs() <= 1., "sessions sidebar must persist a 1100px drag beyond the old fixed cap: actual={}", store.borrow().sessions_sidebar_width());
            for (window_width, expected) in [(800., 464.), (1920., 1100.)] {
                cx.simulate_resize(gpui_kit::size(px(window_width), px(900.)));
                draw(cx);
                let (left, right) = page.read_with(cx, |page, _| page.list_bounds.get());
                assert!((right - left - expected).abs() <= 1., "scale={scale}, bounds={left}..{right}, expected={expected}");
                assert!(right < window_width, "divider must remain inside the viewport");
                assert!((store.borrow().sessions_sidebar_width() - 1100.).abs() <= 1., "temporary fit must not overwrite the preference");
            }
        }
        let connection = rusqlite::Connection::open(&store.borrow().path).unwrap();
        let settings: String = connection.query_row(
            "SELECT value FROM meta WHERE key='settings'", [], |row| row.get(0),
        ).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
        assert!((settings["sessions_sidebar_width"].as_f64().unwrap() - 1100.).abs() <= 1.);
    }
}
