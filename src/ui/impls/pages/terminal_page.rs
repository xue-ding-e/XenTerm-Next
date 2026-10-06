//! The terminal workspace page: the tab strip, the panes, the command bar and
//! the file dock — the part of the window the user actually works in.
//!
//! Everything here used to live on the shell, which is why the shell was
//! nearly four thousand lines: a tab is not a window-level thing, it is a
//! thing this page owns. The page keeps the shell's old doctrine where the
//! doctrine was earned — the pane width is *derived*, never measured, and a
//! click is carried out at the top of the next frame, never inside the render
//! that received it — and reports to the shell only what crosses the page's
//! border: a connect, a tab change the other pages have to follow, a status
//! line note.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, PopupMenuItem},
    popover::Popover,
    v_flex,
    ActiveTheme as _,
    Disableable as _,
    Sizable as _,
};
use gpui_kit::{div, prelude::*, px, rgb, rgba, Animation, AnimationExt as _, Context, Entity, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, SharedString, Subscription, Window};

use std::time::Duration;

use gpui_kit::assets::IconName;

use crate::session::protocol::SessionCommand;

use super::super::{panes, DockEdge, HistoryAction, HistoryView, QuickAction,
    QuickCommandsView, SftpPanelView, SidebarAction, SidebarView, TerminalSettings,
    TerminalView, TransferListView};

/// One open tab: its id, its title, and the view drawing it.
pub(crate) struct Tab {
    pub(crate) id: crate::core::TabId,
    /// The title, from `core::TabMeta` so a rename works the same way here as there.
    pub(crate) meta: crate::core::TabMeta,
    view: Entity<TerminalView>,
}

/// What a tab's own menu asked for.
///
/// Recorded rather than performed because a menu handler only receives an `App`: it
/// cannot reach this view's context, so the tab writes the request down and the page
/// carries it out at the top of the next frame — the same arrangement every panel in
/// this shell uses, and the reason a click cannot re-enter the render it arrived in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TabAction {
    /// Rename the tab, in place.
    Rename(String),
    /// Open the same session again, as a second tab.
    Duplicate(String),
    Close(String),
    /// Close every tab but this one.
    CloseOthers(String),
    /// Move this tab one place towards the start.
    MoveLeft(String),
    /// Move this tab one place towards the end.
    MoveRight(String),
    /// Put the next tab beside this one, in a pane of its own.
    Split(String),
    /// Put the next tab below this one, in a pane of its own.
    SplitDown(String),
}

/// What this page asks the shell to do. The page owns its tabs, its panes and
/// its command bar; the shell is what holds the dialogs, the detached windows
/// and the other pages, and this is the whole of what crosses between.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TerminalAction {
    /// Open `session_id` under `tab_id`, and connect it. The shell resolves
    /// whether the session is monitored and carries the connect out through
    /// [`TerminalPage::open_session_tab`].
    Connect { tab_id: String, session_id: String },
    /// The active tab changed. The shell points the other pages and the
    /// detached windows at it — the file page, the monitor page, the tunnels
    /// page, the session list's highlight.
    ActiveTabChanged(Option<String>),
    /// The dock's manage button was pressed: the quick-command manager is a
    /// dialog, and the dialog queue is the shell's.
    OpenQuickManager,
    /// The tab strip's tunnel button was pressed: the forwards of the active
    /// session, as a dialog.
    OpenTunnels,
    /// The tab strip's `+` (or Ctrl+K anywhere): the quick-connect palette,
    /// a dialog the shell owns.
    OpenQuickConnect,
    /// The landing view's buttons: a brand-new session in the editor, and an
    /// import of the hosts `~/.ssh/config` names.
    NewConnection,
    ImportConfig,
    /// The resource sidebar's buttons: a detached window is the shell's to
    /// open, focus and follow.
    ShowProcesses,
    ShowSystemInfo,
}

thread_local! {
    /// Sinks waiting for the connect that will use them.
    ///
    /// The view is built by `cx.new`, which takes a closure that cannot touch the
    /// page being constructed, and a connect needs both. Rather than a field that
    /// only ever holds entries between two adjacent lines, this holds them for
    /// exactly as long as it takes `open_session_tab` to reach the connect below.
    static PENDING_SINKS: RefCell<std::collections::HashMap<String, std::sync::Arc<dyn crate::core::EventSink>>> =
        RefCell::new(std::collections::HashMap::new());
}

/// Which tab the keyboard should select next.
///
/// `None` when there is nothing to select: no tabs at all, or a single one, where every
/// keystroke would land on the tab already showing and the strip would look broken.
/// An unknown `current` starts from the beginning, so a stale active id still cycles
/// rather than doing nothing.
///
/// Cycling wraps at both ends, unlike moving a tab: the end of the strip is where the
/// user expects to arrive back at the start.
fn next_tab_index(len: usize, current: Option<usize>, reverse: bool) -> Option<usize> {
    if len < 2 {
        return None;
    }
    let forward = |index: usize| (index + 1) % len;
    let back = |index: usize| (index + len - 1) % len;
    Some(match current {
        Some(index) if index < len => {
            if reverse {
                back(index)
            } else {
                forward(index)
            }
        }
        // Nothing active, or an id that is no longer in the strip: the first tab going
        // forward, the last going back, which is where each direction starts from.
        _ => {
            if reverse {
                len - 1
            } else {
                0
            }
        }
    })
}

#[cfg(test)]
mod next_tab_tests {
    use super::next_tab_index;

    #[test]
    fn cycling_wraps_in_both_directions() {
        assert_eq!(next_tab_index(3, Some(0), false), Some(1));
        assert_eq!(next_tab_index(3, Some(1), false), Some(2));
        assert_eq!(
            next_tab_index(3, Some(2), false),
            Some(0),
            "the last tab forward comes back to the first"
        );
        assert_eq!(next_tab_index(3, Some(0), true), Some(2));
        assert_eq!(next_tab_index(3, Some(2), true), Some(1));
    }

    #[test]
    fn a_strip_that_cannot_cycle_says_so() {
        assert_eq!(next_tab_index(0, None, false), None);
        assert_eq!(next_tab_index(1, Some(0), false), None);
        assert_eq!(next_tab_index(1, Some(0), true), None);
    }

    /// A stale active id — a tab that was closed while it was active — still cycles,
    /// rather than making the shortcut do nothing until something else is clicked.
    #[test]
    fn an_unknown_current_starts_from_the_near_end() {
        assert_eq!(next_tab_index(3, None, false), Some(0));
        assert_eq!(next_tab_index(3, None, true), Some(2));
        assert_eq!(next_tab_index(3, Some(9), false), Some(0));
        assert_eq!(next_tab_index(3, Some(9), true), Some(2));
    }
}

/// The terminal workspace.
pub(crate) struct TerminalPage {
    state: crate::ui::SessionState,
    /// The window's width, as the shell measured it this frame. The pane
    /// width is derived from it — see `render_workspace` for why a measurement
    /// would not do.
    window_width: Rc<Cell<f32>>,
    action: Rc<RefCell<Option<TerminalAction>>>,
    /// The open tabs, in strip order, and which one is showing.
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active_tab: Option<String>,
    /// The pane tree: which tabs share the terminal area and how it is divided.
    panes: crate::layout::Layout,
    /// The pane a click asked for, drained at the start of the next frame.
    pane_focus: Rc<RefCell<Option<u64>>>,
    /// The terminal area's measured size, in pixels, as of the last frame that drew it.
    pane_area: Rc<Cell<(f32, f32, f32, f32)>>,
    /// The area's width as the *panes* are laid out with it, which is derived and not
    /// the canvas's.
    pane_width: Rc<Cell<f32>>,
    /// A press on the pane area, and the pointer while it is held.
    pane_press: Rc<RefCell<Option<(f32, f32)>>>,
    pane_pointer: Rc<RefCell<Option<(f32, f32)>>>,
    /// The splitter being resized, as (id, axis start, axis length, vertical).
    pane_grab: Option<(u64, f32, f32, bool)>,
    /// The tab being renamed, and the field doing it.
    pub(crate) renaming: Option<(String, Entity<InputState>)>,
    _rename_subscription: Option<Subscription>,
    /// What a tab's menu asked for, drained at the top of the frame.
    tab_action: Rc<RefCell<Option<TabAction>>>,
    /// The remote directory panel, docked under or beside the panes.
    ///
    /// Draws the listing store, which the session's events fill, and reports what the
    /// user asked for back to the shell — the shell is what holds the session handle,
    /// so the panel cannot send an SFTP command itself.
    sftp: Entity<SftpPanelView>,
    /// Hiding the dock only releases its layout space; the SFTP session and
    /// transfers keep running and the same listing is restored on reopen.
    pub(crate) sftp_collapsed: bool,
    /// The transfer list, drawn as a popover off the tab strip's transfer button.
    transfers: Entity<TransferListView>,
    /// The quick-command dock, as a popover off the command line.
    quick: Entity<QuickCommandsView>,
    /// The command-history dropdown, which sits above the command bar.
    history: Entity<HistoryView>,
    /// The command bar's input, owned by the widget library.
    command: Entity<InputState>,
    _command_subscription: Subscription,
    /// What the settings say the terminals look like, as of the last frame.
    appearance: TerminalSettings,
    /// The resource sidebar, on the left of the workspace: the active session's
    /// machine on top, this machine below. A panel and not a page because it is
    /// something you glance at *while* working in the terminal beside it — the
    /// arrangement the original had and FinalShell has.
    sidebar: Entity<SidebarView>,
    /// The sidebar's current column width, as of the last change. The pane
    /// width is derived from it, so it is watched through the subscription
    /// below and cached here rather than read per frame: a 1 Hz resource
    /// repaint must not re-layout the terminal beside it.
    sidebar_width: f32,
    /// Keeps the width watch alive. Dropping it would leave the panes drawn
    /// against a width the sidebar had outgrown.
    _sidebar_subscription: Subscription,
    /// The tab being dragged along the strip, and the pointer x it started at.
    /// A drag under a few pixels is a click, not a drag.
    /// The strip, as a child view: cached between page-state changes, so a
    /// terminal frame never rebuilds it. See `tab_strip.rs`.
    tab_strip: Entity<super::super::TabStripView>,
    /// The sidebar column's left and right edge in window coordinates, recorded
    /// by its canvas each frame. A six-pixel drag band cannot receive a press in
    /// this window — the pane splitter measured that — so the gesture lives on
    /// the workspace container and hit-tests against this edge instead.
    sidebar_edge: Rc<Cell<(f32, f32)>>,
    /// The bottom dock's top edge, for the dock's own drag, by the same doctrine.
    dock_edge: Rc<Cell<f32>>,
    /// The sidebar drag, while it is one: where it started and how wide the
    /// column was.
    sidebar_drag: Option<(f32, f32)>,
    /// The dock drag: where it started and how tall the dock was.
    dock_drag: Option<(f32, f32)>,
    /// Every chip's window-coords x range, recorded by its own canvas each
    /// frame — the same derive-don't-measure pattern the pane area uses. Read
    /// during a drag to decide which chip the pointer is over.
    chip_bounds: Rc<std::cell::RefCell<Vec<(String, f32, f32)>>>,
}

impl TerminalPage {
    /// The page, starting with **no session at all**: a blank terminal is a
    /// question the window answers worse than the landing view does — there,
    /// quick connect and the local shells are one click away, and the pane
    /// area is not pretending a dead tab is content.
    pub(crate) fn new(
        state: crate::ui::SessionState,
        window_width: Rc<Cell<f32>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let appearance = TerminalSettings::from_store(&state.store.borrow());

        let sftp = cx.new(SftpPanelView::new);
        let sftp_collapsed = state.store.borrow().collapse_sftp_default();
        let transfers = cx.new(|cx| TransferListView::new(state.transfers.clone(), cx));
        let quick_edge = DockEdge::from_setting(&state.store.borrow().quick_panel_dock());
        let quick = cx.new(|_| QuickCommandsView::new(state.store.clone(), quick_edge, true));
        let history = cx.new(|cx| HistoryView::new(state.store.clone(), window, cx));
        let command = cx.new(|cx| InputState::new(window, cx));

        // The resource sidebar. It reads the same shared status map the sessions
        // write and samples this machine itself; the collapse preference and the
        // saved width come off the store, so it is handed one rather than
        // reaching for a global.
        let sidebar = cx.new(|cx| SidebarView::new(state.statuses.clone(), state.store.clone(), cx));
        let sidebar_width = sidebar.read(cx).width();
        // The column's width is the panel's own decision, so the page watches it
        // rather than making it. Only a change re-lays out the workspace: the
        // panel repaints every second while a session is live, and re-rendering
        // the terminal beside it at that rate would make the panel cost what the
        // terminal costs.
        let _sidebar_subscription = cx.observe(&sidebar, |this, sidebar, cx| {
            let width = sidebar.read(cx).width();
            if (width - this.sidebar_width).abs() > f32::EPSILON {
                this.sidebar_width = width;
                cx.notify();
            }
        });

        // Enter in the command bar sends. Subscribed rather than polled for the
        // same reason the session list is: the input owns its own key handling
        // and the page cannot see the key.
        let _command_subscription = cx.subscribe_in(
            &command,
            window,
            |this: &mut Self, _input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.send_command(window, cx);
                }
            },
        );

        let sidebar_edge = Rc::new(Cell::new((0.0, 0.0)));
        let dock_edge = Rc::new(Cell::new(0.0));
        // The queues the strip shares with this page, so its menus and clicks can
        // fill what this page's drains read without either holding the other.
        let action = Rc::new(RefCell::new(None));
        let tab_action = Rc::new(RefCell::new(None));
        let chip_bounds = Rc::new(std::cell::RefCell::new(Vec::new()));
        // The strip is a child view of this page: it reads this page through the
        // accessors and redraws when this page is notified, never when a terminal
        // frame is — that is the whole point of it being its own entity.
        // `cx` here is this page's own construction context, so `page_handle` is
        // the page the strip observes.
        let page_handle = cx.entity();
        let tab_strip = cx.new(|strip_cx| {
            super::super::TabStripView::new(
                &page_handle,
                action.clone(),
                tab_action.clone(),
                chip_bounds.clone(),
                strip_cx,
            )
        });

        Self {
            state,
            window_width,
            action,
            tabs: Vec::new(),
            active_tab: None,
            panes: crate::layout::Layout::new(Vec::new(), String::new()),
            pane_focus: Rc::new(RefCell::new(None)),
            pane_area: Rc::new(Cell::new((0.0, 0.0, 0.0, 0.0))),
            pane_width: Rc::new(Cell::new(0.0)),
            pane_press: Rc::new(RefCell::new(None)),
            pane_pointer: Rc::new(RefCell::new(None)),
            pane_grab: None,
            renaming: None,
            _rename_subscription: None,
            tab_action,
            tab_strip,
            sidebar_edge,
            dock_edge,
            sidebar_drag: None,
            dock_drag: None,
            sftp,
            sftp_collapsed,
            transfers,
            quick,
            history,
            command,
            _command_subscription,
            appearance,
            sidebar,
            sidebar_width,
            _sidebar_subscription,
            chip_bounds,
        }
    }

    // ------------------------------------------------------------------
    // What the shell reads and calls.
    // ------------------------------------------------------------------

    /// The next thing this page is asking the shell for, if any.
    pub(crate) fn take_action(&self) -> Option<TerminalAction> {
        self.action.borrow_mut().take()
    }

    /// Ask the shell for something, from code that is not this page's own
    /// tree — the command palette runs through here for the actions whose
    /// natural home is a page-owned queue.
    pub(crate) fn request(&mut self, action: TerminalAction, cx: &mut Context<Self>) {
        *self.action.borrow_mut() = Some(action);
        cx.notify();
    }

    /// The active tab's id, for the drains that act on "the session showing".
    /// The session phase a chip's dot shows: 0 dialling, 1 live, 2 ended.
    pub(crate) fn tab_state(&self, id: &str) -> u8 {
        self.state
            .statuses
            .lock()
            .ok()
            .and_then(|map| map.get(id).map(|status| status.state))
            .unwrap_or(0)
    }

    /// Whether duplicating this tab names a session a second connection can open.
    pub(crate) fn tab_duplicable(&self, id: &str) -> bool {
        let store = self.state.store.borrow();
        store.get(id).is_some()
            || crate::app::session_models::builtin_local_sessions(store.wsl_profiles())
                .iter()
                .any(|builtin| builtin.id == id)
    }

    /// The rename in progress, as (tab id, its field), for the chip that shows it.
    pub(crate) fn renaming(&self) -> Option<(String, Entity<InputState>)> {
        self.renaming.clone()
    }

    pub(crate) fn active_tab_id(&self) -> Option<String> {
        self.active_tab.clone()
    }

    /// The tab strip's own file dock, whose actions the shell drains alongside
    /// the files page's panel.
    pub(crate) fn dock_panel(&self) -> &Entity<SftpPanelView> {
        &self.sftp
    }

    pub(crate) fn set_sftp_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if self.sftp_collapsed != collapsed {
            self.sftp_collapsed = collapsed;
            cx.notify();
        }
    }

    /// Whether the active tab can drive the file dock at all.
    ///
    /// Only a session with an SFTP channel registers a handle, so a local shell, a
    /// serial line or a telnet tab has no directory to show — and a dock rendered for
    /// one is an empty frame ("目录为空" over a blank tree column) that reads as
    /// broken rather than as absent. The dock is only mounted when this is true.
    pub(crate) fn sftp_available(&self) -> bool {
        self.active_tab
            .as_ref()
            .is_some_and(|id| match self.state.sftp_handles.lock() {
                Ok(map) => map.contains_key(id),
                Err(_) => false,
            })
    }

    /// The transfer list, whose actions the shell drains alongside the transfers
    /// page's list.
    pub(crate) fn transfers(&self) -> &Entity<TransferListView> {
        &self.transfers
    }

    /// The resource panel, for the command palette's show/hide — the page lays
    /// the column out at the width the panel reports, so the fold is the
    /// panel's own state to change.
    pub(crate) fn sidebar_entity(&self) -> &Entity<SidebarView> {
        &self.sidebar
    }

    /// Whether a tab with this id is open.
    pub(crate) fn has_tab(&self, id: &str) -> bool {
        self.tabs.iter().any(|tab| tab.id.as_str() == id)
    }

    /// The title of `tab_id`'s tab — the session name, or the user's rename.
    pub(crate) fn tab_title(&self, tab_id: Option<&str>) -> Option<String> {
        let id = tab_id?;
        self.tabs
            .iter()
            .find(|tab| tab.id.as_str() == id)
            .map(|tab| tab.meta.title().to_string())
    }

    /// The tabs other than the active one, as (id, title): the menu labels are
    /// what the user recognises, and the ids are what a cross-session copy needs.
    pub(crate) fn other_tabs(&self) -> Vec<(String, String)> {
        self.tabs
            .iter()
            .filter(|tab| Some(tab.id.as_str()) != self.active_tab.as_deref())
            .map(|tab| (tab.id.as_str().to_string(), tab.meta.title().to_string()))
            .collect()
    }

    /// Open `session_id` under `tab_id`, and connect it.
    ///
    /// The tab id is separate from the session id because a *duplicate* is a second tab
    /// of one session: the same configuration, a new tab id, and therefore a second
    /// connection, which is what duplicating a tab asks for. Everything keyed by tab id —
    /// the buffer, the render gate, the resource samples — is per connection, so a second
    /// tab must not share them.
    pub(crate) fn open_session_tab(
        &mut self,
        tab_id: &str,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        // A row names either a saved session or one of the built-in local shells
        // (PowerShell / CMD / WSL). The built-ins have no saved configuration to read —
        // they are constructed on demand from the machine's own shells — so the store is
        // the first place to look and not the only one.
        let session = {
            let store = self.state.store.borrow();
            store.get(session_id).cloned().or_else(|| {
                crate::app::session_models::builtin_local_sessions(store.wsl_profiles())
                    .into_iter()
                    .find(|builtin| builtin.id == session_id)
            })
        };
        let Some(session) = session else {
            tracing::warn!(
                "the list offered session {session_id}, which is neither saved nor a built-in shell"
            );
            return;
        };

        let title = session.name.clone();
        let tab = Self::open_tab(&self.state, tab_id, &title, self.appearance.clone(), cx);

        // Whether the session is asked for remote resource samples follows the panel
        // that draws them: folded away means nobody is looking, and a live session
        // would otherwise run a remote sampler for the life of the window.
        let monitoring = !self.sidebar.read(cx).is_collapsed();
        if let Some(sink) = PENDING_SINKS.with(|sinks| sinks.borrow_mut().remove(tab_id)) {
            self.state.connect(tab_id, session, sink, monitoring);
        }

        self.panes.add_tab(tab_id.to_string());
        self.tabs.push(tab);
        self.set_active_tab(Some(tab_id.to_string()), cx);
    }

    /// Show the tab for `session_id`, opening one if it is not already open.
    ///
    /// Reuse rather than always-open, so clicking the same session twice focuses it
    /// instead of stacking identical tabs.
    pub(crate) fn activate_or_open(&mut self, session_id: &str, cx: &mut Context<Self>) {
        if let Some(existing) = self.tabs.iter().find(|tab| tab.id.as_str() == session_id) {
            let id = existing.id.as_str().to_string();
            self.set_active_tab(Some(id), cx);
            return;
        }
        self.open_session_tab(session_id, session_id, cx);
    }

    /// Refresh the quick-command dock's rows after the manager saved.
    pub(crate) fn refresh_quick(&mut self, cx: &mut Context<Self>) {
        self.quick.update(cx, |quick, cx| quick.refresh(cx));
    }

    // ------------------------------------------------------------------
    // Drag to reorder the strip.
    // ------------------------------------------------------------------



    /// Reorder the strip live while a chip is dragged: when the pointer
    /// crosses onto another chip, the dragged tab takes that chip's place.
    /// A movement under four pixels is a click settling, not a drag.

    /// Re-read the active tab's listing into the dock.
    pub(crate) fn refresh_dock(&mut self, cx: &mut Context<Self>) {
        let tab_id = self.active_tab.clone();
        let other_tabs = self.other_tabs();
        sync_panel(&self.sftp, tab_id.as_deref(), other_tabs, &self.state, cx);
    }
    /// Make `id` the tab showing: the dock and the resource sidebar follow it,
    /// and the shell is told so it can point the detached windows and the
    /// session list's highlight at it.
    pub(crate) fn set_active_tab(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.active_tab = id.clone();
        // The pane tree has its own notion of "which tab this leaf shows"; the
        // strip's active tab and the leaf's must agree or clicking a chip
        // switches the dock and the sidebar while the grid keeps playing the
        // previous session.
        if let Some(id) = &id {
            if let Some(leaf_id) = self.panes.leaf_of_tab(id) {
                self.panes.focused = leaf_id;
                if let Some(leaf) = self.panes.leaf_mut(leaf_id) {
                    leaf.active = id.clone();
                }
            }
        }
        self.refresh_dock(cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_active(id.clone(), cx));
        *self.action.borrow_mut() = Some(TerminalAction::ActiveTabChanged(id));
        cx.notify();
    }

    // ------------------------------------------------------------------
    // The frame.
    // ------------------------------------------------------------------

    fn drain_pane_focus(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.pane_focus.borrow_mut().take() else {
            return;
        };
        if self.panes.focused == id {
            return;
        }
        self.panes.focused = id;
        if let Some(leaf) = self.panes.leaf(id) {
            // Focusing a pane also makes its tab the active one: everything beside
            // the area — the file panel, the monitor page — is about the session
            // the user is looking at, and after a split that is the pane they just
            // clicked.
            self.set_active_tab(Some(leaf.active.clone()), cx);
        }
    }

    /// Carry out a splitter drag: the press decides which splitter, the pointer where.
    ///
    /// The hit test is against `flatten`'s own splitter rects, at the **derived** width
    /// the panes are drawn at rather than the canvas's — the canvas reports the window's
    /// width, 1040 against 613, which put every handle's rect 214 pixels away from the
    /// handle and made three drags miss. A press that lands on no splitter is not a drag
    /// at all, which is what leaves a click in the middle of a pane focusing that pane.
    fn drain_pane_drag(&mut self, cx: &mut Context<Self>) {
        const SPLITTER_BAND: f32 = 5.0;
        let (origin_x, origin_y, _, height) = self.pane_area.get();
        let width = self.pane_width.get();
        if let Some((x, y)) = self.pane_press.borrow_mut().take() {
            let (px, py) = (x - origin_x, y - origin_y);
            let (_, splitters) = self.panes.flatten(0.0, 0.0, width, height);
            self.pane_grab = crate::layout::Layout::splitter_at(&splitters, px, py, SPLITTER_BAND)
                .map(|index| {
                    let s = &splitters[index];
                    (s.split_id, s.axis_start, s.axis_len, s.vertical)
                });
        }
        let Some(grab) = self.pane_grab else {
            return;
        };
        let Some((x, y)) = *self.pane_pointer.borrow() else {
            // Released: the next press starts a new drag rather than resuming this one.
            self.pane_grab = None;
            return;
        };
        let pos = if grab.3 { x - origin_x } else { y - origin_y };
        self.panes.set_ratio(grab.0, grab.1, grab.2, pos);
        cx.notify();
    }

    /// Perform whatever a tab's menu asked for since the last frame.
    ///
    /// A rename starts an input in the chip rather than a dialog: the field appears where
    /// the title was, Enter keeps the name, and clicking away abandons it — which is how
    /// an inline rename behaves everywhere else, and the only option here because the
    /// widget library reports no Escape for a single-line field.
    fn drain_tab_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A tab whose session ended can ask to be reconnected — Enter on it is the request —
        // and this is where that is honoured. The view can tell that a session is gone but
        // cannot start one; the shell can. The tab id is the session id, so this is the same
        // call the session list makes when a row is clicked.
        let wanting: Vec<String> = self
            .tabs
            .iter()
            .filter(|tab| tab.view.update(cx, |view, _| view.take_reconnect_request()))
            .map(|tab| tab.id.as_str().to_string())
            .collect();
        for id in wanting {
            *self.action.borrow_mut() = Some(TerminalAction::Connect {
                tab_id: id.clone(),
                session_id: id,
            });
        }
        loop {
            let Some(action) = self.tab_action.borrow_mut().take() else {
                return;
            };
            match action {
                TabAction::Rename(id) => {
                    let Some(tab) = self.tabs.iter().find(|tab| tab.id.as_str() == id) else {
                        continue;
                    };
                    let current = tab.meta.title().to_string();
                    let input = cx.new(|cx| {
                        InputState::new(window, cx).default_value(current)
                    });
                    // Enter keeps what was typed; losing focus abandons it. A rename is a
                    // deliberate act, and a field that committed on a stray click would
                    // rename a tab while the user was reaching for something else.
                    self._rename_subscription = Some(cx.subscribe_in(
                        &input,
                        window,
                        |this: &mut Self,
                         _,
                         event: &InputEvent,
                         _,
                         cx| {
                            match event {
                                InputEvent::PressEnter { .. } => this.commit_rename(cx),
                                InputEvent::Blur => {
                                    this.renaming = None;
                                    this._rename_subscription = None;
                                    cx.notify();
                                }
                                _ => {}
                            }
                        },
                    ));
                    self.renaming = Some((id, input));
                    cx.notify();
                }
                // Duplicating opens a second connection to the same session: a fresh tab
                // id, because everything keyed by tab id is per connection. The connect
                // itself is the shell's — it decides whether the session is monitored.
                TabAction::Duplicate(id) => {
                    *self.action.borrow_mut() = Some(TerminalAction::Connect {
                        tab_id: uuid::Uuid::new_v4().to_string(),
                        session_id: id,
                    });
                }
                TabAction::Close(id) => self.close_tab(&id, cx),
                TabAction::MoveLeft(id) => self.move_tab(&id, -1, cx),
                TabAction::MoveRight(id) => self.move_tab(&id, 1, cx),
                TabAction::Split(id) => self.split_pane(&id, false, cx),
                TabAction::SplitDown(id) => self.split_pane(&id, true, cx),
                TabAction::CloseOthers(id) => {
                    let others: Vec<String> = self
                        .tabs
                        .iter()
                        .filter(|tab| tab.id.as_str() != id)
                        .map(|tab| tab.id.as_str().to_string())
                        .collect();
                    for other in others {
                        self.close_tab(&other, cx);
                    }
                }
            }
        }
    }

    /// Perform whatever the quick-command dock asked for since the last frame.
    fn drain_quick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(action) = self.quick.update(cx, |quick, _| quick.take_action()) {
            match action {
                QuickAction::Run {
                    command,
                    send_enter,
                } => {
                    if send_enter {
                        let (history, bytes) = crate::terminal::encode_command_bar_input(&command);
                        self.send_raw(&bytes);
                        if let Some(line) = history {
                            // Into the same history the command bar writes, because it is
                            // the same question later: "what did I run recently".
                            let mut store = self.state.store.borrow_mut();
                            store.push_command_history(line);
                            if let Err(error) = store.save() {
                                tracing::warn!("could not save the command history: {error:#}");
                            }
                        }
                    } else {
                        // Typed into the bar rather than sent: the flag means "let me
                        // finish this one by hand", so the command goes where finishing it
                        // happens.
                        let value = SharedString::from(command);
                        self.command
                            .update(cx, |input, cx| input.set_value(value, window, cx));
                    }
                    cx.notify();
                }
                QuickAction::Manage => {
                    *self.action.borrow_mut() = Some(TerminalAction::OpenQuickManager);
                }
                QuickAction::Close => {
                    // Nothing to close: the dock is a popover, and a popover closes by
                    // clicking outside it. The action is still reported — and ignored —
                    // rather than removed, because the dock's own header is what offers
                    // it and the header is hidden when the shell hosts it.
                }
                // The view already folded it; nothing to carry out. Kept in the enum so
                // the fold is reported the same way every other interaction is, which is
                // what a second view of the list would need.
                QuickAction::ToggleGroup(_) => {}
            }
        }
    }

    /// Perform whatever the history dropdown asked for since the last frame.
    ///
    /// The panel owns no session and no input box, so recalling a command is reported
    /// rather than done: this page holds the command bar.
    fn drain_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(action) = self.history.update(cx, |history, _| history.take_action()) {
            match action {
                HistoryAction::Recall(command) => {
                    let value = SharedString::from(command);
                    self.command
                        .update(cx, |input, cx| input.set_value(value, window, cx));
                    // Focused, because the next thing the user does is edit or send it,
                    // and both need the caret in the box. The popover closes itself: a
                    // click inside it is a click outside the trigger.
                    self.command.update(cx, |input, cx| input.focus(window, cx));
                }
                HistoryAction::Delete(index) => {
                    let mut store = self.state.store.borrow_mut();
                    store.remove_command_history(index);
                    if let Err(error) = store.save() {
                        tracing::warn!("could not save the command history: {error:#}");
                    }
                }
                HistoryAction::Clear => {
                    let mut store = self.state.store.borrow_mut();
                    store.clear_command_history();
                    if let Err(error) = store.save() {
                        tracing::warn!("could not save the command history: {error:#}");
                    }
                }
            }
            cx.notify();
        }
    }

    /// Notice a change to the terminal appearance, and hand it to every tab.
    ///
    /// Compared rather than pushed because the settings are a file: the settings page,
    /// the CLI and a second window can all change it, and a value read once per frame
    /// catches every one of them. This page holds the tabs, so it is also the only
    /// thing that can tell them.
    fn sync_appearance(&mut self, cx: &mut Context<Self>) {
        let current = TerminalSettings::from_store(&self.state.store.borrow());
        if current == self.appearance {
            return;
        }
        self.appearance = current.clone();
        for tab in &self.tabs {
            tab.view
                .update(cx, |view, cx| view.set_appearance(current.clone(), cx));
        }
    }

    /// Keep the name the rename field holds, and put the title back.
    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some((id, input)) = self.renaming.take() else {
            return;
        };
        self._rename_subscription = None;
        let typed = input.read(cx).value().trim().to_string();
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id.as_str() == id) {
            // An empty field lifts the rename rather than naming the tab nothing: the
            // title falls back to what the session is called, which is what clearing a
            // rename means.
            tab.meta.set_override((!typed.is_empty()).then_some(typed));
        }
        cx.notify();
    }

    /// Close a tab and end its session.
    pub(crate) fn close_tab(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id.as_str() == tab_id) else {
            return;
        };
        // The view is dropped with this binding, which is what stops the pump: the
        // tab's channel sender lives in the entity, and losing it is the signal the
        // task is waiting on.
        let _tab = self.tabs.remove(index);
        self.panes.remove_tab(tab_id);
        self.panes.prune();

        // The session ends with the tab: dropping the handle's sender is what stops the
        // pump. Any auth prompt the tab was parked on is answered by that same drop —
        // the view holds the prompt, and the prompt holds the responder's sender, so
        // losing the view resolves the `oneshot` the auth flow is waiting on. The
        // per-tab stores are retired with it — the resource sidebar and the detached
        // windows read them by tab id, and a closed tab's entry is a dead session
        // still answering those reads, which is what left the sidebar describing a
        // connection the user had already closed.
        self.state.end_tab(tab_id);

        if self.active_tab.as_deref() == Some(tab_id) {
            // The neighbour, not the first tab: closing the third of five should show
            // the second or the fourth, which is where the eye already is.
            let next = self
                .tabs
                .get(index.saturating_sub(1))
                .or_else(|| self.tabs.first())
                .map(|tab| tab.id.as_str().to_string());
            // Through `set_active_tab`, not a bare assignment: the active tab is
            // read by the dock, the resource sidebar and the shell's followers,
            // and this is the one path that re-points every one of them — the
            // bare assignment this replaced left the sidebar describing the tab
            // that had just closed.
            self.set_active_tab(next, cx);
            return;
        }
        cx.notify();
    }

    /// Send the command bar's text to the active session, then clear the bar.
    fn send_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.command.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let (history, bytes) = crate::terminal::encode_command_bar_input(&text);
        let target = self.active_tab.clone();
        if let Some(tab_id) = target {
            if let Some(handle) = self.state.handles.borrow().get(&tab_id) {
                let _ = handle
                    .commands
                    .send(SessionCommand::RawInput(bytes));
            }
        }
        // Recorded whether or not a session took it: the history answers "what did I type
        // into this box", and a command that went nowhere because no session was open is
        // exactly the one you want back.
        if let Some(line) = history {
            let mut store = self.state.store.borrow_mut();
            store.push_command_history(line);
            if let Err(error) = store.save() {
                tracing::warn!("could not save the command history: {error:#}");
            }
        }
        self.command
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.history.update(cx, |history, cx| history.refresh(cx));
        // The bar empties, which is the receipt: the text went somewhere and is no
        // longer here to be sent twice.
        cx.notify();
    }

    /// Send bytes to the active session, if there is one.
    fn send_raw(&self, bytes: &[u8]) {
        let Some(tab_id) = self.active_tab.as_deref() else {
            return;
        };
        if let Some(handle) = self.state.handles.borrow().get(tab_id) {
            let _ = handle
                .commands
                .send(SessionCommand::RawInput(bytes.to_vec()));
        }
    }

    /// Move a tab one place along the strip.
    ///
    /// The order of `tabs` is the order on screen, so this is a swap and nothing else. An
    /// edge is not an error: a tab already first asked to move left stays first, because
    /// the alternative — wrapping to the end — is a way to lose a tab among twenty.
    fn move_tab(&mut self, id: &str, step: isize, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id.as_str() == id) else {
            return;
        };
        let Some(target) = index.checked_add_signed(step) else {
            return;
        };
        if target >= self.tabs.len() {
            return;
        }
        self.tabs.swap(index, target);
        cx.notify();
    }

    /// Give this tab's pane a neighbour, moving the next tab into it.
    ///
    /// The model is asked to split, and it answers with the new leaf's id — or with nothing,
    /// which is the case where there is no other tab to move and a split would be a pane
    /// holding the same session twice. Doing nothing is the right answer there: a second
    /// view of one session is two cursors on one shell, which is not what splitting is for.
    fn split_pane(&mut self, id: &str, vertical: bool, cx: &mut Context<Self>) {
        let Some(other) = self
            .tabs
            .iter()
            .find(|tab| tab.id.as_str() != id)
            .map(|tab| tab.id.as_str().to_string())
        else {
            return;
        };
        let dir = if vertical {
            crate::layout::Dir::Vertical
        } else {
            crate::layout::Dir::Horizontal
        };
        if self
            .panes
            .split(self.panes.focused, dir, &other, false)
            .is_none()
        {
            return;
        }
        // The active tab follows the move, because the pane the user was looking at is
        // the one that just changed shape.
        self.set_active_tab(Some(other), cx);
    }

    /// The keyboard split: put the next tab in a pane beside — or below — the one
    /// the user is looking at. Run now rather than queued, because an action
    /// handler is already event-phase code; the menu keeps its queue and lands
    /// in [`Self::split_pane`].
    pub(crate) fn split_active_tab(&mut self, vertical: bool, cx: &mut Context<Self>) {
        let Some(id) = self.active_tab.clone() else {
            return;
        };
        self.split_pane(&id, vertical, cx);
    }

    /// The keyboard's walk around the split panes, in layout order.
    ///
    /// The focused pane is what the splitter ring draws and what clicking a pane
    /// sets; this is the same slot the click fills, moved by a chord instead of
    /// by the mouse. With one pane there is nothing to move around.
    pub(crate) fn cycle_pane(&mut self, cx: &mut Context<Self>) {
        let (_, _, w, h) = self.pane_area.get();
        let panes = self.panes.flatten(0.0, 0.0, self.pane_width.get(), h).0;
        if panes.len() < 2 {
            return;
        }
        let current = panes.iter().position(|rect| rect.focused);
        let next = match current {
            Some(index) => (index + 1) % panes.len(),
            None => 0,
        };
        *self.pane_focus.borrow_mut() = Some(panes[next].id);
        cx.notify();
    }

    /// Select the next tab, or the previous one for a reverse cycle.
    ///
    /// The keyboard counterpart of clicking a chip. Unlike moving a tab, cycling *wraps*:
    /// the end of the strip is where the user expects to come back to the start.
    pub(crate) fn cycle_tab(&mut self, reverse: bool, cx: &mut Context<Self>) {
        let current = self
            .active_tab
            .as_ref()
            .and_then(|id| self.tabs.iter().position(|tab| tab.id.as_str() == id));
        let Some(index) = next_tab_index(self.tabs.len(), current, reverse) else {
            return;
        };
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        self.set_active_tab(Some(tab.id.as_str().to_string()), cx);
    }

    /// Open a tab for `tab_id`, with its own channel and its own view.
    ///
    /// A channel per tab, because the channel is what carries a tab's events to the view
    /// drawing it: one queue for the window would deliver every session's output to
    /// every view. The gates stay shared — they are keyed by tab id — so a producer's
    /// ticket and its view's flush still meet.
    fn open_tab(
        state: &crate::ui::SessionState,
        tab_id: &str,
        title: &str,
        appearance: TerminalSettings,
        cx: &mut Context<Self>,
    ) -> Tab {
        let (ui, messages) = tokio::sync::mpsc::unbounded_channel();
        let sink = state.sink_for(ui);

        let tab_id = tab_id.to_string();
        // The page handle, captured before the view's own construction context
        // shadows `cx`: a state transition inside the view notifies this page, so
        // the cached strip hears about the dot it has to repaint.
        let page = cx.entity().downgrade();
        let view = cx.new(|cx| {
            TerminalView::new(
                tab_id.clone(),
                appearance,
                state.handles.clone(),
                state.sftp_listings.clone(),
                state.sftp_trees.clone(),
                state.statuses.clone(),
                state.transfers.clone(),
                state.tunnels.clone(),
                state.opened_file.clone(),
                messages,
                Some(page),
                cx,
            )
        });

        // The screen the session parses into has to be in the shared map before
        // anything connects, or the first frame would draw a different buffer from the
        // one the pump fills.
        if let Ok(mut bufs) = state.bufs.lock() {
            bufs.insert(tab_id.clone(), view.read(cx).buffer().clone());
        }

        // The sink is stashed until the connect, which is what pairs it with a tab.
        PENDING_SINKS.with(|sinks| {
            sinks.borrow_mut().insert(tab_id.clone(), sink);
        });

        Tab {
            id: crate::core::TabId::new(tab_id),
            meta: crate::core::TabMeta::new(crate::core::TabKind::Terminal, title.to_string()),
            view,
        }
    }

    // ------------------------------------------------------------------
    // The tree.
    // ------------------------------------------------------------------

    /// The resource sidebar as a fixed-width column, shared by the with-session
    /// workspace and the landing view so the panel sits in the same place either
    /// way. It draws its own collapsed strip when folded, so it is mounted
    /// whether or not anything is connected, and the column width does the
    /// talking.
    ///
    /// The column's width is the animated one and clips; the panel inside is
    /// laid out at its target width, so the fold slides the workspace edge over
    /// a full-sized panel instead of squeezing its rows frame by frame.
    fn render_sidebar_column(&self, border: gpui_kit::Hsla, cx: &Context<Self>) -> gpui_kit::Div {
        let content_width = self.sidebar.read(cx).content_width();
        let sidebar_edge = self.sidebar_edge.clone();
        div()
            .w(px(self.sidebar_width))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(border)
            .overflow_hidden()
            .relative()
            // The column's own edges, recorded for the next frame's drag
            // hit-test: a band cannot receive a press in this window, so the
            // workspace container answers for it against this measurement —
            // the same arrangement the pane splitter settled on.
            .child(
                gpui_kit::canvas(
                    move |bounds, _, _| {
                        sidebar_edge.set((
                            f32::from(bounds.origin.x),
                            f32::from(bounds.origin.x) + f32::from(bounds.size.width),
                        ));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .w(px(content_width))
                    .h_full()
                    .child(self.sidebar.clone()),
            )
            .child(
                // The grab band, drawn rather than wired: it shows where the
                // edge is draggable and what the cursor means there, while the
                // press itself is answered by the workspace container.
                div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(px(RESIZE_BAND * 2.0))
                    .cursor_col_resize()
                    .hover(|this| this.bg(border)),
            )
    }

    /// The landing view: what can be started from an empty window, centred.
    /// Quick connect — the palette, also on Ctrl+K — and the machine's own
    /// shells, one click each. This is what the window opens onto instead of a
    /// blank "终端" tab that no one asked for.
    fn render_empty_state(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(44.))
                    .text_color(theme.muted)
                    .child(IconName::Terminal),
            )
            .child(
                div()
                    .text_lg()
                    .child(crate::i18n::t("未连接", "Not connected")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(crate::i18n::t(
                        "Ctrl+K 快速连接；也可以新建会话，或从文件导入配置。",
                        "Ctrl+K for quick connect; or create a session, or import from a file.",
                    )),
            )
            .child(
                // Stacked, not a row: three doors of equal weight read better
                // as a list, and a column keeps the landing narrow.
                v_flex()
                    .gap_2()
                    .mt_2()
                    .w(px(240.))
                    .child(
                        Button::new("empty-quick-connect")
                            .icon(IconName::Plus)
                            .label(crate::i18n::t("快速连接", "Quick connect"))
                            .primary()
                            .large()
                            .on_click({
                                let action = self.action.clone();
                                move |_, _, _| {
                                    *action.borrow_mut() =
                                        Some(TerminalAction::OpenQuickConnect);
                                }
                            }),
                    )
                    .child(
                        Button::new("empty-new-connection")
                            .icon(IconName::FilePlus)
                            .label(crate::i18n::t("新建连接", "New connection"))
                            .ghost()
                            .large()
                            .on_click({
                                let action = self.action.clone();
                                move |_, _, _| {
                                    *action.borrow_mut() = Some(TerminalAction::NewConnection);
                                }
                            }),
                    )
                    .child(
                        Button::new("empty-import")
                            .icon(IconName::Download)
                            .label(crate::i18n::t("导入配置", "Import config"))
                            .ghost()
                            .large()
                            .on_click({
                                let action = self.action.clone();
                                move |_, _, _| {
                                    *action.borrow_mut() = Some(TerminalAction::ImportConfig);
                                }
                            }),
                    ),
            )
    }

    fn render_workspace(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // The strip is a child entity now: this hands the element a reference
        // rather than building the tree, which is what keeps a terminal frame
        // from paying for the strip. See `tab_strip.rs`.
        let tab_strip = self.tab_strip.clone();
        let border = cx.theme().border;

        // No session open: the landing view — quick connect and the local
        // shells, centred — in place of a pane area pretending a dead tab is
        // content. The sidebar stays: this machine's own numbers are worth a
        // glance with nothing connected, and the FinalShell arrangement keeps
        // them there either way.
        if self.tabs.is_empty() {
            let empty = self.render_empty_state(cx);
            return h_flex()
                .size_full()
                .min_h_0()
                .overflow_hidden()
                .child(self.render_sidebar_column(border, cx))
                .child(
                    v_flex()
                        .h_full()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .child(tab_strip)
                        .child(empty),
                );
        }

        let command_bar = self.render_command_bar(cx);
        let sftp = self.sftp.clone();
        let sidebar_column = self.render_sidebar_column(border, cx);
        let sidebar_width = self.sidebar_width;
        // The pane area's width, as arithmetic rather than as a measurement: the
        // window's width less the navigation rail, less the resource sidebar's
        // column, and less the file panel when it docks to the right. Measured
        // from the canvas instead, the area reported the *window's* width, and a
        // split then computed its panes against a width nobody was drawn at.
        let sftp_right = self.state.store.borrow().sftp_panel_on_the_right();
        // The dock is the user's to collapse, and only a session with an SFTP
        // channel has anything to put in it: a local shell tab mounts no dock at
        // all rather than an empty frame.
        let dock_shown = !self.sftp_collapsed && self.sftp_available();
        let pane_w = (self.window_width.get()
            - super::super::tokens::RAIL_WIDTH
            - sidebar_width
            - file_panel_width(sftp_right, !dock_shown))
        .max(0.0);
        self.pane_width.set(pane_w);
        let (_, _, _, pane_h) = self.pane_area.get();
        let pane_area = self.pane_area.clone();
        // One element per pane, each showing the tab that pane has active. Built here
        // rather than asked for through a callback because an element is not `Clone`:
        // a callback can only be written for the single-pane case, which is the case
        // that stops being true the moment anyone splits.
        let pane_contents: Vec<gpui_kit::AnyElement> = self
            .panes
            .flatten(0.0, 0.0, pane_w, pane_h)
            .0
            .iter()
            .map(|rect| {
                self.tabs
                    .iter()
                    .find(|tab| tab.id.as_str() == rect.active)
                    .map(|tab| tab.view.clone().into_any_element())
                    .unwrap_or_else(|| div().size_full().into_any_element())
            })
            .collect();
        let has_panes = self
            .active_tab
            .as_ref()
            .is_some_and(|id| self.tabs.iter().any(|tab| tab.id.as_str() == id));
        let border = cx.theme().border;
        // The terminal's own background, from the same palette the grid paints
        // with. The pane area and the command bar take it too: the terminal is
        // dark whatever the window theme says, and in a light window a light
        // strip behind or under a dark grid read as two different surfaces
        // colliding.
        let background = super::super::terminal::rgba_to_hsla(
            crate::terminal::terminal_background(true),
        );
        let pane_focus = self.pane_focus.clone();
        let pane_press = self.pane_press.clone();
        let pane_pointer = self.pane_pointer.clone();

        // The resize drags live here, on the container, because a five-pixel
        // band never receives a press in this window — the pane splitter
        // measured that — while the container receives the press, the moves
        // and the release. The bands the sidebar and the dock draw say where
        // to grab; the hit-test below says what a grab does.
        h_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    let (x, y) = (
                        f32::from(event.position.x),
                        f32::from(event.position.y),
                    );
                    // The sidebar's right edge, only while the column is shown —
                    // a folded column is the fold animation's to move, not the
                    // drag's.
                    let (left, right) = this.sidebar_edge.get();
                    if right > left
                        && this.sidebar_width > 40.0
                        && x >= right - RESIZE_BAND
                        && x <= right + RESIZE_BAND
                    {
                        this.sidebar_drag = Some((x, this.sidebar_width));
                        return;
                    }
                    // The bottom dock's top edge, only while it docks below.
                    let top = this.dock_edge.get();
                    let dock_below = !this
                        .state
                        .store
                        .borrow()
                        .sftp_panel_on_the_right()
                        && !this.sftp_collapsed
                        && this.sftp_available();
                    if dock_below && top > 0.0 && y >= top - RESIZE_BAND && y <= top + RESIZE_BAND
                    {
                        this.dock_drag = Some((y, this.strip_height()));
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if let Some((start_x, start_width)) = this.sidebar_drag {
                    let width = start_width + (f32::from(event.position.x) - start_x);
                    this.sidebar.update(cx, |sidebar, cx| sidebar.set_width(width, cx));
                }
                if let Some((start_y, start_height)) = this.dock_drag {
                    let height = start_height - (f32::from(event.position.y) - start_y);
                    let clamped = height.clamp(DOCK_MIN_HEIGHT, DOCK_MAX_HEIGHT);
                    let mut store = this.state.store.borrow_mut();
                    if (store.quick_panel_height() - clamped).abs() > 0.5 {
                        store.set_quick_panel_height(clamped);
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if this.sidebar_drag.take().is_some() {
                        // The moves wrote the width into the store's cache; the
                        // release is what pays for the write to disk.
                        let _ = this.state.store.borrow_mut().save();
                        cx.notify();
                    }
                    if this.dock_drag.take().is_some() {
                        let _ = this.state.store.borrow_mut().save();
                        cx.notify();
                    }
                }),
            )

            .child(sidebar_column)
            .child(
                v_flex()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(tab_strip)
                    .child(
                        // The command line owns a separate row below the terminal.
                        // The pane area and the file panel form a
                        // column when the panel is docked to the bottom, a row when it
                        // is docked to the right.
                        div()
                            .flex()
                            .when(sftp_right, |this| this.flex_row())
                            .when(!sftp_right, |this| this.flex_col())
                            .flex_1()
                            .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .relative()
                            .overflow_hidden()
                            // The pane area is terminal-dark in both window
                            // themes: a light window behind a dark grid read as
                            // two surfaces colliding wherever the grid left a
                            // gap.
                            .bg(background)
                            // Everything above the command line, and nothing
                            // else. The canvas here measures the box the panes
                            // are drawn in, so shrinking it by the bar's row is
                            // what makes the grid's last line end *above* the
                            // input instead of behind it: a pane rect is the
                            // size its terminal really has, and the terminal
                            // sizes its rows from that. The command line used to
                            // float over this box, which hid exactly the line the
                            // session had just written.
                            .child(
                                div()
                                    .id("terminal-pane-area")
                                    .debug_selector(|| "terminal-pane-area".to_string())
                                    .flex_1()
                                    .min_w_0()
                                    .min_h_0()
                                    .relative()
                                    .overflow_hidden()
                                    .child(
                                        // What this area measured, recorded for the
                                        // next frame's pane rects. A prepaint callback
                                        // is the only place an element's size is known.
                                        // Schedule a frame only when the size changes,
                                        // so an idle terminal adopts a toggled dock
                                        // without waiting for new session output.
                                        gpui_kit::canvas(
                                            move |bounds, window, _cx| {
                                                let size = bounds.size;
                                                let (_, _, old_width, old_height) = pane_area.get();
                                                if (old_width, old_height)
                                                    != (f32::from(size.width), f32::from(size.height))
                                                {
                                                    window.request_animation_frame();
                                                }
                                                pane_area.set((
                                                    f32::from(bounds.origin.x),
                                                    f32::from(bounds.origin.y),
                                                    f32::from(size.width),
                                                    f32::from(size.height),
                                                ));
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .inset_0(),
                                    )
                                    // The panes fill that measured box rather than the
                                    // whole container, so the rect a click and a
                                    // splitter drag resolve against is the rect the
                                    // terminal is drawn at.
                                    .when(has_panes, |this| {
                                        this.child(panes::render_layout(
                                            &self.panes,
                                            pane_w,
                                            pane_h,
                                            border,
                                            cx.theme().accent,
                                            pane_contents,
                                            pane_focus,
                                            pane_press,
                                            pane_pointer,
                                        ))
                                    }),
                            )
                            // The command line, in flow rather than anchored: it
                            // takes a row of its own along the bottom edge and
                            // the pane layer above keeps the rest. The bar itself
                            // is untouched — same background, same hairline, same
                            // controls — and its `flex_shrink_0` is what keeps
                            // that row exactly as tall as the bar.
                            .child(command_bar),
                    )
                    .when(dock_shown, |this| {
                        let dock_edge = self.dock_edge.clone();
                        this.child(
                        // A strip off the bottom, or a column at the right: the same
                        // panel, and the same children inside it either way.
                        div()
                            .id("file-panel-dock")
                            .debug_selector(|| "file-panel-dock".to_string())
                            .when(sftp_right, |this| this.h_full().w(px(STRIP_WIDTH)))
                            .when(!sftp_right, |this| {
                                this.w_full().h(px(self.strip_height()))
                            })
                            .min_w_0()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .relative()
                            // The bottom dock's height is the user's to drag: record
                            // the top edge for the container's hit-test, and draw the
                            // band that says so. A right-docked column is resized by
                            // nothing yet — its width is the strip's own constant.
                            .when(!sftp_right, |this| {
                                this.child(
                                    gpui_kit::canvas(
                                        move |bounds, _, _| {
                                            dock_edge.set(f32::from(bounds.origin.y));
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                            })
                            .when(!sftp_right, |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .top_0()
                                        .h(px(RESIZE_BAND * 2.0))
                                        .cursor_row_resize()
                                        .hover(|this| this.bg(border)),
                                )
                            })
                            // The border goes on the edge that faces the output.
                            .when(sftp_right, |this| this.border_l_1())
                            .when(!sftp_right, |this| this.border_t_1())
                            .border_color(border)
                            // The file panel, and only it: the command dock is a popover
                            // off the command line now, so there is nothing left to switch
                            // between and no tab row to spend a line on.
                            .child(sftp),
                        )
                    }),
                ),
            )
    }

    /// The tab strip: one chip per session, with the transfers toggle and the
    /// tunnel dialog at its right end — session-scoped doors, kept off the
    /// command line and off the navigation rail, which is for pages.

    /// The command line: type a line, send it to the active session.
    ///
    /// It owns a row below the terminal — see `render_workspace` — and carries
    /// exactly the two things reached for while
    /// typing: the quick commands and what has been run before, both as popovers, because
    /// each is opened to pick one line and then closed.
    fn render_command_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Terminal-dark, in both window themes: the bar sits below the pane
        // area, and a light strip under a dark grid would read as two
        // surfaces colliding. The input loses its own chrome
        // (`appearance(false)`) and takes this background, so the bar and the
        // grid read as one surface.
        let background = super::super::terminal::rgba_to_hsla(
            crate::terminal::terminal_background(true),
        );
        let light = rgb(0xd4d4d4);
        let command = self.command.clone();
        h_flex()
            .id("terminal-command-bar")
            .debug_selector(|| "terminal-command-bar".to_string())
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .gap_2()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(background)
            .bg(background)
            // The press stays here. The pane layer underneath re-takes keyboard
            // focus on every mouse-down — "a click goes back to the terminal" —
            // and without stopping the propagation a click on the input focused
            // the field and then handed it straight back to the grid, so the
            // box silently typed into the shell.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&command)
                        // An element id, because without one the input has no hitbox
                        // and a click on it never focuses it — which reads as a text
                        // field that silently ignores the keyboard.
                        .id("command-input")
                        .aria_label(crate::i18n::t(
                            "输入命令，回车发送",
                            "Type a command, Enter to send",
                        ))
                        .appearance(false)
                        // `rgba` hex is RRGGBBAA: 0xffffff14 is white at 8%
                        // alpha over the bar's dark. (A first draft wrote the
                        // alpha first — 0x14ffffff — which is opaque cyan, and
                        // the box glowed like a highlighter.)
                        .bg(rgba(0xffffff14))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgba(0xffffff1f))
                        // The toolkit's focused ring is a saturated accent —
                        // on the terminal-dark bar it glows. The caret already
                        // says where typing lands; the field keeps only its
                        // hairline.
                        .focus_bordered(false)
                        .text_color(light),
                ),
            )
            .child(
                Popover::new("command-quick")
                    .trigger(
                        Button::new("command-quick-trigger")
                            .icon(IconName::Zap)
                            .ghost()
                            .text_color(light)
                            .tooltip(crate::i18n::t("快捷指令", "Quick commands"))
                            .accessibility_label(crate::i18n::t("快捷指令", "Quick commands")),
                    )
                    .content({
                        let quick = self.quick.clone();
                        move |_, _, _| {
                            div()
                                .w(px(300.))
                                .h(px(360.))
                                .child(quick.clone().into_any_element())
                        }
                    }),
            )
            .child(
                Popover::new("command-history")
                    .trigger(
                        Button::new("command-history-trigger")
                            // The filter is cleared when the list opens: a search that
                            // survives being closed makes it look empty next time.
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.history
                                    .update(cx, |history, cx| history.reset_filter(window, cx));
                                cx.notify();
                            }))
                            .icon(IconName::Menu)
                            .ghost()
                            .text_color(light)
                            .tooltip(crate::i18n::t("历史指令", "Command history"))
                            .accessibility_label(crate::i18n::t("历史指令", "Command history")),
                    )
                    .content({
                        let history = self.history.clone();
                        move |_, _, _| {
                            div()
                                .w(px(420.))
                                .h(px(320.))
                                .child(history.clone().into_any_element())
                        }
                    }),
            )
            .child(
                Button::new("send-command")
                    .icon(IconName::Send)
                    .label(crate::i18n::t("发送", "Send"))
                    .primary()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.send_command(window, cx);
                    })),
            )
    }

    /// How tall the panel strip under the terminal is, as the settings have it.
    ///
    /// Read per frame rather than cached: it is a setting the settings page writes, and a
    /// copy taken at startup would ignore a change until the window was reopened.
    fn strip_height(&self) -> f32 {
        self.state.store.borrow().quick_panel_height()
    }
}

impl Render for TerminalPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // First thing in the frame, because this is the first point in it where a
        // `Context` exists to read the panels with — a click that arrived during the
        // previous frame's input pass is carried out here.
        self.drain_pane_focus(cx);
        self.drain_pane_drag(cx);
        self.drain_tab_actions(window, cx);
        self.drain_quick(window, cx);
        self.drain_history(window, cx);
        self.sync_appearance(cx);
        // The sidebar's two doors: a detached window is the shell's to open,
        // so the panel's request travels out through the action queue.
        while let Some(action) = self.sidebar.update(cx, |sidebar, _| sidebar.take_action()) {
            match action {
                SidebarAction::ShowProcesses => {
                    *self.action.borrow_mut() = Some(TerminalAction::ShowProcesses);
                }
                SidebarAction::ShowSystemInfo => {
                    *self.action.borrow_mut() = Some(TerminalAction::ShowSystemInfo);
                }
            }
        }
        self.render_workspace(cx)
    }
}

use super::super::tokens::DOCK_STRIP_WIDTH as STRIP_WIDTH;
use super::super::tokens::{DOCK_MAX_HEIGHT, DOCK_MIN_HEIGHT, RESIZE_BAND};

fn file_panel_width(right: bool, collapsed: bool) -> f32 {
    if right && !collapsed {
        STRIP_WIDTH
    } else {
        0.0
    }
}

/// Point the dock at `tab_id`: the listing it shows, the tree beside it, and
/// the other sessions a cross-session copy can name. `tab_id` of `None` leaves
/// the panel showing what it has — there is nothing to describe.
fn sync_panel(
    panel: &Entity<SftpPanelView>,
    tab_id: Option<&str>,
    other_tabs: Vec<(String, String)>,
    state: &crate::ui::SessionState,
    cx: &mut Context<TerminalPage>,
) {
    let Some(tab_id) = tab_id else {
        return;
    };
    let listing = state.listing(tab_id);
    // The tree the session last sent for this tab. Empty until it sends one,
    // which is what the panel draws as an empty column rather than as a wrong
    // one.
    let tree = state
        .sftp_trees
        .lock()
        .ok()
        .and_then(|map| map.get(tab_id).cloned())
        .unwrap_or_default();
    let generation = listing.generation();
    panel.update(cx, |panel, cx| {
        panel.set_targets(other_tabs, cx);
        panel.set_tree(tree, cx);
        panel.set_listing(listing, cx);
        // Recorded last: if the store changes between the read above and this
        // update, the drain's comparison sees the stale generation and syncs
        // again next frame rather than swallowing the change.
        panel.mark_synced(tab_id, generation, cx);
    });
}

#[cfg(test)]
mod dock_tests {
    use super::*;
    use gpui_kit::gpui::TestAppContext;
    use gpui_kit::Modifiers;
    use std::sync::{Arc, Mutex};

    fn fixture_state(
        collapsed: bool,
        right: bool,
    ) -> (
        crate::ui::SessionState,
        tokio::sync::mpsc::UnboundedReceiver<SessionCommand>,
    ) {
        let mut store = crate::config::ConfigStore {
            path: std::env::temp_dir().join(format!("xenterm-dock-{}.db", uuid::Uuid::new_v4())),
            backup_dir: None,
            cache: crate::config::ConfigFile::default(),
            key: [7; 32],
            keyring_enabled: false,
            saved_state: Mutex::new(crate::config::SavedState::default()).into(),
        };
        store.set_collapse_sftp_default(collapsed);
        store.set_sidebar_collapsed(true);
        store.set_sftp_panel_dock(if right { "right" } else { "bottom" }.into());
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let state = crate::ui::SessionState::new(
            runtime.clone(),
            Arc::new(Mutex::new(std::collections::HashMap::new())),
            Rc::new(RefCell::new(store)),
        );
        let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
        state.handles.borrow_mut().insert(
            "fixture".into(),
            crate::session::protocol::SessionHandle {
                tab_id: "fixture".into(),
                commands,
                join: runtime.spawn(async {}),
            },
        );
        // The dock is only mounted for a session that can drive it, so the fixture
        // registers an SFTP channel too — a real remote tab always has one.
        let (sftp_commands, _sftp_receiver) = tokio::sync::mpsc::unbounded_channel();
        state.sftp_handles.lock().unwrap().insert(
            "fixture".into(),
            crate::sftp::SftpHandle {
                commands: sftp_commands,
                join: runtime.spawn(async {}),
            },
        );
        (state, receiver)
    }

    #[test]
    fn hidden_file_docks_never_reserve_terminal_columns() {
        assert_eq!(file_panel_width(true, false), STRIP_WIDTH);
        assert_eq!(file_panel_width(true, true), 0.0);
        assert_eq!(file_panel_width(false, false), 0.0);
        assert_eq!(file_panel_width(false, true), 0.0);
    }

    /// The dock's new contract: a session without an SFTP channel — a local shell,
    /// a serial line — mounts no dock at all, because a dock over a blank tree and a
    /// "目录为空" line reads as broken rather than as absent.
    #[gpui_kit::gpui::test]
    fn a_session_without_an_sftp_channel_mounts_no_dock(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (state, _receiver) = fixture_state(false, false);
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let width = Rc::new(Cell::new(f32::from(window.viewport_size().width)));
            let mut page = TerminalPage::new(state, width, window, cx);
            // A tab with a session handle but deliberately no SFTP handle: the
            // shape a local shell's tab has.
            let tab = TerminalPage::open_tab(
                &page.state,
                "local",
                "Local",
                page.appearance.clone(),
                cx,
            );
            PENDING_SINKS.with(|sinks| sinks.borrow_mut().remove("local"));
            page.tabs.push(tab);
            page.active_tab = Some("local".into());
            page.panes = crate::layout::Layout::new(vec!["local".into()], "local".into());
            page
        });
        for _ in 0..3 {
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        }
        assert!(
            cx.debug_bounds("file-panel-dock").is_none(),
            "a session with no SFTP channel shows no file dock"
        );
        assert!(!view.read_with(cx, |page, _| page.sftp_available()));
    }

    #[gpui_kit::gpui::test]
    fn file_panel_startup_visibility_uses_the_saved_preference(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        for collapsed in [false, true] {
            let (state, _receiver) = fixture_state(collapsed, false);
            let (view, cx) = cx.add_window_view(move |window, cx| {
                TerminalPage::new(state, Rc::new(Cell::new(1280.)), window, cx)
            });
            assert_eq!(view.read_with(cx, |page, _| page.sftp_collapsed), collapsed);
        }
    }

    #[gpui_kit::gpui::test]
    fn file_panel_close_and_reopen_reclaims_space_and_keeps_the_terminal(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        for right in [false, true] {
            let (state, mut receiver) = fixture_state(false, right);
            let (view, cx) = cx.add_window_view(move |window, cx| {
                let width = Rc::new(Cell::new(f32::from(window.viewport_size().width)));
                let mut page = TerminalPage::new(state, width, window, cx);
                // A real terminal view and buffer, but no local process, network,
                // saved credentials or connection worker.
                let tab = TerminalPage::open_tab(
                    &page.state,
                    "fixture",
                    "Fixture",
                    page.appearance.clone(),
                    cx,
                );
                PENDING_SINKS.with(|sinks| sinks.borrow_mut().remove("fixture"));
                page.tabs.push(tab);
                page.active_tab = Some("fixture".into());
                page.panes = crate::layout::Layout::new(vec!["fixture".into()], "fixture".into());
                page
            });
            // Pane geometry is measured by the previous frame's canvas.
            for _ in 0..3 {
                cx.update(|window, cx| {
                    window.simulate_next_frame(cx);
                    window.draw(cx).clear(cx);
                });
            }
            let before = cx
                .debug_bounds("terminal-pane-area")
                .expect("terminal is drawn");
            let bar = cx
                .debug_bounds("terminal-command-bar")
                .expect("command bar is drawn");
            assert!(
                before.origin.y + before.size.height <= bar.origin.y,
                "terminal output must stop above the command bar"
            );
            assert!(cx.debug_bounds("file-panel-dock").is_some());
            while receiver.try_recv().is_ok() {}
            let original_panel = view.read_with(cx, |page, _| page.sftp.entity_id());
            let original_terminal = view.read_with(cx, |page, _| page.tabs[0].view.entity_id());

            let close = cx
                .debug_bounds("sftp-collapse")
                .expect("file panel has a close button");
            cx.simulate_click(close.center(), Modifiers::default());
            view.update(cx, |page, cx| {
                let action = page.sftp.update(cx, |panel, _| panel.take_action());
                assert_eq!(action, Some(crate::ui::PanelAction::Collapse));
                // The same local-only action performed by Shell::drain_panel.
                page.set_sftp_collapsed(true, cx);
            });
            for _ in 0..3 {
                cx.update(|window, cx| {
                    window.simulate_next_frame(cx);
                    window.draw(cx).clear(cx);
                });
            }
            assert!(cx.debug_bounds("file-panel-dock").is_none());
            let hidden = cx
                .debug_bounds("terminal-pane-area")
                .expect("terminal stays visible");
            if right {
                assert!(hidden.size.width > before.size.width);
            } else {
                assert!(hidden.size.height > before.size.height);
            }
            let bar = cx.debug_bounds("terminal-command-bar").unwrap();
            assert!(hidden.origin.y + hidden.size.height <= bar.origin.y);
            let mut resized = false;
            while let Ok(command) = receiver.try_recv() {
                assert!(
                    !matches!(command, SessionCommand::Close),
                    "hiding must not close the terminal"
                );
                resized |= matches!(command, SessionCommand::Resize(_, _));
            }
            assert!(resized, "an idle terminal must receive the new PTY size");

            let toggle = cx
                .debug_bounds("toggle-sftp-panel")
                .expect("reopen control remains available");
            cx.simulate_click(toggle.center(), Modifiers::default());
            for _ in 0..3 {
                cx.update(|window, cx| {
                    window.simulate_next_frame(cx);
                    window.draw(cx).clear(cx);
                });
            }
            assert!(cx.debug_bounds("file-panel-dock").is_some());
            let reopened = cx.debug_bounds("terminal-pane-area").unwrap();
            assert_eq!(reopened.size, before.size, "reopening restores the dock extent");
            let mut resized_back = false;
            while let Ok(command) = receiver.try_recv() {
                assert!(!matches!(command, SessionCommand::Close));
                resized_back |= matches!(command, SessionCommand::Resize(_, _));
            }
            assert!(resized_back, "reopening also resizes the idle PTY");
            // Repeated tab-bar toggles use the same state and keep the reopen
            // control reachable even when the panel itself is absent.
            for collapsed in [true, false] {
                let toggle = cx.debug_bounds("toggle-sftp-panel").unwrap();
                cx.simulate_click(toggle.center(), Modifiers::default());
                for _ in 0..3 {
                    cx.update(|window, cx| {
                        window.simulate_next_frame(cx);
                        window.draw(cx).clear(cx);
                    });
                }
                assert_eq!(cx.debug_bounds("file-panel-dock").is_none(), collapsed);
            }
            view.read_with(cx, |page, _| {
                assert!(!page.sftp_collapsed);
                assert_eq!(page.sftp.entity_id(), original_panel);
                assert_eq!(page.tabs[0].view.entity_id(), original_terminal);
                assert_eq!(page.active_tab.as_deref(), Some("fixture"));
            });
        }
    }
}
