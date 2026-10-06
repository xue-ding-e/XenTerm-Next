//! The resource panel: what the host on the other end of the active session is doing,
//! and what this machine is doing, in one column.
//!
//! ## Where the numbers come from
//!
//! Two stores, neither of them this view's. A session's samples are written into
//! [`crate::resource::TabStatus`] by the event path (`super::view::apply_event`), and
//! this machine's are written by the sampler task this view starts. Both are
//! `Arc<Mutex<_>>` because their producers run off the UI thread — the same arrangement
//! the shell uses, which is the point: the two panels are two projections of one set of
//! stores rather than two measurements of the same machine. Everything that
//! decides *what* a number means (a used fraction, an auto-scaled history, which NIC is
//! on top, how a size is spelled) lives in `crate::resource`, so the two panels cannot
//! disagree about it.
//!
//! ## Why it polls
//!
//! Nothing pushes to this view. A remote sample arrives on the tab's own channel and is
//! handled by the terminal view that owns it, and the local sampler is a task with no
//! route into a sibling view. So the panel reads the stores once a second, and repaints
//! only when what it would draw has actually changed — a panel that repainted every
//! second would keep the GPU awake to show the same numbers.
//!
//! ## What is deliberately absent
//!
//! Docking to any of the four edges, and dragging the panel by its header, are absent:
//! this shell lays out a fixed right-hand column, so a panel that could be dragged to
//! the bottom edge would be one that cannot actually go there. The collapse button is
//! kept, because collapsing is a decision about this column and does not need a dock.
//!
//! The System-information and Processes buttons appear only for a live remote session,
//! which is the only kind with a process sample and a detailed probe behind it
//! (`super::system_info_window`, `super::process_window`): a button that opened an empty
//! window would be worse than no button — the rule the SFTP panel follows for its
//! directory tree.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        popover::Popover,
        progress::Progress,
        v_flex, ActiveTheme as _, Icon, Sizable as _, Theme,
    },
    div,
    prelude::*,
    px, relative, Animation, AnimationExt as _, AnyElement, Context, FontWeight, Hsla, IntoElement,
    Render, SharedString, Task, WeakEntity, Window,
};

// The full Lucide catalog rather than the component library's short default list: the
// icons this panel needs for a data panel (a pulse line, an arrow up) are in the
// catalog and not in the curated subset, and the shell already loads the whole bundle.
use gpui_kit::assets::IconName;

use std::time::Duration;

use crate::config::ConfigStore;
use crate::resource::system::{format_bytes_per_sec, format_mem};
use crate::resource::{
    connection_host, DiskUsage, LocalSnap, NetHist, SystemSampler, SystemSnapshot, TabStatus,
    TabStatuses, NET_HISTORY_LEN,
};

use super::tokens::{SIDEBAR_COLLAPSED_WIDTH as COLLAPSED_WIDTH, SIDEBAR_MAX_WIDTH as MAX_WIDTH, SIDEBAR_MIN_WIDTH as MIN_WIDTH};
/// The height of one throughput graph.
const GRAPH_HEIGHT: f32 = 44.0;
/// How long folding the column out or in takes. Short enough to read as one
/// motion rather than a process; long enough that the eye sees the column
/// move instead of the workspace snapping.
const ANIM_FOR: std::time::Duration = std::time::Duration::from_millis(180);
/// The animation's tick. One resize per frame is the same cost the window
/// already pays while being dragged by its edge — the terminal grid reflows
/// from measured bounds either way.
const ANIM_STEP: std::time::Duration = std::time::Duration::from_millis(16);
/// How long the "copied" confirmation stays up after a host is copied.
const COPIED_FOR: std::time::Duration = std::time::Duration::from_millis(1400);

/// What the panel draws, resolved once per frame from the two stores.
///
/// A plain snapshot rather than a set of accessors, because the same value is used for
/// two things: to draw, and to decide whether drawing again is worth it. `PartialEq` is
/// what makes the second one free.
#[derive(Clone, PartialEq)]
struct Resources {
    title: SharedString,
    /// 0 grey / 1 green (live) / 2 yellow (dropped) — the header dot's colour.
    conn_state: u8,
    conn_text: SharedString,
    /// The host to copy, or empty when there is nothing worth copying.
    conn_host: String,
    /// Whether the process monitor has anything to show for this tab: a live remote
    /// session, whose sample carries a process table. A local shell's does not.
    proc_available: bool,
    /// Whether the detailed probe has a subject here — the same live remote session, for
    /// the same reason: `sys` is filled by a remote probe and nothing else.
    system_info_available: bool,
    cpu: f32,
    mem: f32,
    swap: f32,
    mem_detail: SharedString,
    swap_detail: SharedString,
    top_up: SharedString,
    top_down: SharedString,
    /// Already auto-scaled to 0..1; see [`crate::resource::normalized_history`].
    top_history: Vec<f32>,
    /// Whether the top graph belongs to a remote session at all. A local tab draws
    /// one graph — this machine's — rather than the same machine twice.
    top_is_remote: bool,
    /// A remote session is the subject but no sample is flowing (still connecting,
    /// or the session dropped): the top graph shows an honest placeholder instead
    /// of whatever numbers would fill the space.
    top_waiting: bool,
    show_selector: bool,
    ifaces: Vec<SharedString>,
    selected: SharedString,
    bot_up: SharedString,
    bot_down: SharedString,
    bot_history: Vec<f32>,
    disks: Vec<DiskUsage>,
}

impl Resources {
    /// What a panel with nothing to report shows. Every field is the value this panel
    /// starts a frame with, so an un-connected window draws the same thing every time.
    fn idle() -> Self {
        Self {
            title: crate::i18n::t("本机资源", "Local resources").into(),
            conn_state: 0,
            conn_text: crate::i18n::t("未连接", "Not connected").into(),
            conn_host: String::new(),
            proc_available: false,
            system_info_available: false,
            cpu: 0.0,
            mem: 0.0,
            swap: 0.0,
            mem_detail: SharedString::default(),
            swap_detail: SharedString::default(),
            top_up: SharedString::default(),
            top_down: SharedString::default(),
            top_history: vec![0.0; NET_HISTORY_LEN],
            top_is_remote: false,
            top_waiting: false,
            show_selector: false,
            ifaces: Vec::new(),
            selected: SharedString::default(),
            bot_up: SharedString::default(),
            bot_down: SharedString::default(),
            bot_history: vec![0.0; NET_HISTORY_LEN],
            disks: Vec::new(),
        }
    }
}

/// What the panel wants the shell to do.
///
/// The shell owns the windows, so the panel reports intent rather than opening one: the
/// process monitor and the system-information window are the shell's, not children of
/// this column. Same shape as the SFTP panel's actions, and for the same reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidebarAction {
    /// Open — or bring forward — the process monitor for the active session.
    ShowProcesses,
    /// Open — or bring forward — the system-information window for the active session.
    ShowSystemInfo,
}

/// The resource panel.
pub(crate) struct SidebarView {
    /// Per-tab status: the remote half of everything below.
    statuses: TabStatuses,
    /// This machine's latest sample and its throughput history.
    local: LocalSnap,
    local_net_hist: NetHist,
    /// Which tab's status the remote half reads. The shell sets it on every tab change,
    /// because the shell is what knows which tab is showing.
    active: Option<String>,
    /// The saved sessions, for the one preference this panel owns (collapsed).
    store: Rc<RefCell<ConfigStore>>,
    collapsed: bool,
    /// The column's width when expanded, from the config.
    width: f32,
    /// The width the column is drawn at right now, which animates between
    /// [`COLLAPSED_WIDTH`] and `width` on every fold. The shell's layout reads
    /// this one, while the panel's own content is laid out at the target (see
    /// [`Self::content_width`]) and clipped — so the rows inside are not
    /// squeezed frame by frame while the column moves.
    shown_width: f32,
    /// The running width animation, replaced (and so cancelled) if the user
    /// folds again mid-flight.
    anim: Option<Task<()>>,
    /// What was drawn last, for the change check the poll does.
    shown: Resources,
    /// The last action an interaction produced, drained by the shell.
    ///
    /// Recorded rather than acted on because the panel does not own a window: a
    /// `cx.listener` can open a dialog in *this* window, but a second window is the
    /// shell's to create.
    pending: Option<SidebarAction>,
    /// Whether the "copied" confirmation is up.
    copied: bool,
    /// The one-shot task that takes the confirmation down again.
    ///
    /// Held rather than detached so a second copy replaces the first one's timer
    /// instead of leaving two tasks racing to clear the same flag.
    clear_copied: Option<Task<()>>,
    /// The sampler and the poll, kept alive for as long as the panel is.
    _sample: Task<()>,
}

impl SidebarView {
    pub(crate) fn new(
        statuses: TabStatuses,
        store: Rc<RefCell<ConfigStore>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let local: LocalSnap = Arc::new(Mutex::new(SystemSnapshot::default()));
        let local_net_hist: NetHist = Arc::new(Mutex::new(vec![0.0; NET_HISTORY_LEN]));

        // Hosted as a tab of the left column, the panel is not a column of its own and
        // cannot be collapsed: the tab that shows it is also what hides it, and a panel
        // folded to a chevron inside its own tab is a tab with nothing in it.
        let width = store.borrow().sidebar_width().clamp(MIN_WIDTH, MAX_WIDTH);
        // The fold is a remembered preference: a panel the user folded away stays
        // folded on the next launch.
        let collapsed = store.borrow().sidebar_collapsed().unwrap_or(false);

        let sample = Self::spawn_sampler(local.clone(), local_net_hist.clone(), cx);

        Self {
            statuses,
            local,
            local_net_hist,
            active: None,
            store,
            collapsed,
            width,
            shown_width: if collapsed { COLLAPSED_WIDTH } else { width },
            anim: None,
            shown: Resources::idle(),
            pending: None,
            copied: false,
            clear_copied: None,
            _sample: sample,
        }
    }

    /// The 1 Hz task: sample this machine, then repaint if anything the panel draws moved.
    ///
    /// The sample itself runs on the background executor: `sysinfo` refreshes every core's
    /// counters and blocks for a few milliseconds, which is not time to spend on the
    /// foreground thread while a terminal is painting.
    ///
    /// While collapsed it does not sample at all. That is this panel's policy and it
    /// is the honest one: the graph is not on screen, and a machine whose resource panel
    /// is folded away should not be enumerating its own disks once a second to fill in a
    /// line nobody can see.
    fn spawn_sampler(
        local: LocalSnap,
        local_net_hist: NetHist,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let sampler = Arc::new(Mutex::new(SystemSampler::new()));
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(SystemSampler::recommended_interval())
                    .await;

                // A failed read means the view is gone, which is the normal end of this
                // task's life rather than an error: the window closed.
                let collapsed = this.update(cx, |view, _| view.collapsed).unwrap_or(true);
                if collapsed {
                    continue;
                }

                let sampler = sampler.clone();
                let snapshot = cx
                    .background_executor()
                    .spawn(async move {
                        let mut sampler = sampler.lock().unwrap_or_else(|e| e.into_inner());
                        sampler.sample()
                    })
                    .await;

                // Written before the repaint, so the frame that follows draws the sample
                // this tick took rather than the previous one.
                if let Ok(mut stored) = local.lock() {
                    *stored = snapshot.clone();
                }
                if let Ok(mut history) = local_net_hist.lock() {
                    crate::resource::push_ring(&mut history, snapshot.net_bytes_per_sec as f32);
                }

                let alive = this.update(cx, |view, cx| {
                    let next = view.project();
                    if next != view.shown {
                        view.shown = next;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
    }

    /// Take the next action the panel asked for, if any.
    pub(crate) fn take_action(&mut self) -> Option<SidebarAction> {
        self.pending.take()
    }

    /// The tab whose status the remote half reads.
    pub(crate) fn set_active(&mut self, tab: Option<String>, cx: &mut Context<Self>) {
        self.active = tab;
        self.refresh(cx);
    }

    /// Re-read the stores now, for the shell to call after it has changed something the
    /// panel draws — a connect, a close, a tab switch.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let next = self.project();
        if next != self.shown {
            self.shown = next;
            cx.notify();
        }
    }

    /// The width the shell should reserve for the column this frame — the
    /// animated one, so folding moves the workspace instead of snapping it.
    pub(crate) fn width(&self) -> f32 {
        self.shown_width
    }

    /// Set the column's width: the drag's write path, and the settings page's.
    ///
    /// Clamped to the same bounds the constructor reads, persisted to the store
    /// the panel was handed, and notified so the page's observer re-lays the
    /// workspace out. The save itself is the caller's — a drag calls this on
    /// every move and saves once, on release.
    pub(crate) fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let clamped = width.clamp(MIN_WIDTH, MAX_WIDTH);
        if (self.shown_width - clamped).abs() < 0.5 {
            return;
        }
        self.store.borrow_mut().set_sidebar_width(clamped);
        self.shown_width = clamped;
        cx.notify();
    }

    /// The width the panel's *content* is laid out at this frame: the target
    /// width, never the animated one. The shell puts the panel in a clipping
    /// container of the animated width, so the column slides over a
    /// full-sized panel rather than squeezing one — the same way a window
    /// edge slides over a terminal, and for the same reason: rows that
    /// reflow every frame read as jitter, not motion.
    pub(crate) fn content_width(&self) -> f32 {
        if self.showing_strip() {
            COLLAPSED_WIDTH
        } else {
            self.width
        }
    }

    /// Whether what is on the column this frame is the folded strip: only
    /// once the fold has actually arrived, so the slide keeps showing the
    /// panel sliding away under the workspace edge.
    fn showing_strip(&self) -> bool {
        self.collapsed && (self.shown_width - COLLAPSED_WIDTH).abs() < 0.5
    }

    /// Whether the panel is folded away.
    ///
    /// The shell asks before it starts a session: a folded panel means nobody is looking
    /// at the remote sample, and asking the host for one anyway would be a remote command
    /// per session for the life of the window (#127).
    pub(crate) fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    /// Resolve everything the panel draws from the two stores.
    ///
    /// The branch structure is `crate::app::sidebar::refresh_sidebar`'s, deliberately: a
    /// local shell tab shows this machine under its own name, a live remote session shows
    /// the server, and a connecting or dropped one keeps the server's heading with no
    /// numbers rather than showing the local machine's under a remote host's name. What
    /// is *not* duplicated is the arithmetic — the fractions, the formatted sizes and the
    /// NIC choice all come from `crate::resource`.
    fn project(&self) -> Resources {
        let snapshot = self
            .local
            .lock()
            .map(|snap| snap.clone())
            .unwrap_or_default();
        let local_history = self
            .local_net_hist
            .lock()
            .map(|history| crate::resource::normalized_history(&history))
            .unwrap_or_else(|_| vec![0.0; NET_HISTORY_LEN]);

        let mut view = Resources::idle();
        // The machine this process runs on: the bottom graph always, and the whole panel
        // whenever the active tab is not a live remote session.
        let bot_up = format_bytes_per_sec(snapshot.net_tx_per_sec);
        let bot_down = format_bytes_per_sec(snapshot.net_rx_per_sec);
        let local_disks = crate::resource::disk_usage(&snapshot.disks);
        view.bot_up = bot_up.clone().into();
        view.bot_down = bot_down.clone().into();
        view.bot_history = local_history.clone();

        let show_local = |view: &mut Resources| {
            view.title = crate::i18n::t("本机资源", "Local resources").into();
            view.cpu = snapshot.cpu_percent;
            view.mem = snapshot.mem_percent;
            view.swap = snapshot.swap_percent;
            view.mem_detail = format_mem(snapshot.mem_used_mib, snapshot.mem_total_mib).into();
            view.swap_detail = format_mem(snapshot.swap_used_mib, snapshot.swap_total_mib).into();
            view.disks = local_disks.clone();
        };

        let status: Option<TabStatus> = self
            .active
            .as_deref()
            .and_then(|tab| self.statuses.lock().ok()?.get(tab).cloned());

        match status {
            // A local shell tab: the connection state in the header is real, but the
            // resources under it are this machine's — their own status carries no CPU or
            // memory at all, because there is no monitor channel behind it. One graph
            // draws it: the top graph would be this machine twice.
            Some(status) if status.is_local => {
                view.conn_state = conn_state(status.state);
                view.conn_text = connection_text(&status);
                view.conn_host = connection_host(&status.host).into();
                show_local(&mut view);
            }
            // A live remote session: the server's numbers, and the server's NIC on top.
            Some(status) if status.state == 1 => {
                view.conn_state = 1;
                view.conn_text = status.host.clone().into();
                view.conn_host = connection_host(&status.host).into();
                view.proc_available = true;
                view.system_info_available = true;
                view.title = crate::i18n::t("服务器资源", "Server resources").into();
                view.top_is_remote = true;
                view.cpu = status.cpu;
                view.mem = fraction(status.mem_used_kib, status.mem_total_kib);
                view.swap = fraction(status.swap_used_kib, status.swap_total_kib);
                view.mem_detail =
                    format_mem(status.mem_used_kib / 1024, status.mem_total_kib / 1024).into();
                view.swap_detail =
                    format_mem(status.swap_used_kib / 1024, status.swap_total_kib / 1024).into();
                let (name, rx, tx) = crate::resource::selected_iface(&status);
                view.top_up = format_bytes_per_sec(tx).into();
                view.top_down = format_bytes_per_sec(rx).into();
                view.top_history = crate::resource::normalized_history(&status.net_hist);
                view.show_selector = !status.net.is_empty();
                view.selected = name.into();
                view.ifaces = status
                    .net
                    .iter()
                    .map(|(name, _, _)| name.as_str().into())
                    .collect();
                view.disks = crate::resource::disk_usage(&status.disks);
            }
            // Dropped or still connecting: the heading stays the server's — that is what
            // the panel is about — and the top graph holds a placeholder rather than
            // this machine's numbers, which would read as the server's. This machine's
            // own graph stays underneath, because that half was never the server's.
            Some(status) => {
                view.conn_state = conn_state(status.state);
                view.conn_text = connection_text(&status);
                view.conn_host = connection_host(&status.host).into();
                view.title = crate::i18n::t("服务器资源", "Server resources").into();
                view.top_is_remote = true;
                view.top_waiting = true;
            }
            // Nothing open: this machine, honestly labelled.
            None => {
                show_local(&mut view);
            }
        }

        view
    }

    /// Copy the connection's host, and show that it happened.
    ///
    /// The confirmation is in the panel rather than in a toast: it belongs next to the
    /// thing that was copied, and a toast for a clipboard write would be an interruption
    /// out of proportion to the action.
    fn copy_host(&mut self, host: String, cx: &mut Context<Self>) {
        if host.is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(host));
        self.copied = true;
        self.clear_copied = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            let _ = this.update(cx, |view, cx| {
                view.copied = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Fold the panel away, or bring it back, and remember which.
    pub(crate) fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if self.collapsed == collapsed {
            return;
        }
        self.collapsed = collapsed;
        {
            let mut store = self.store.borrow_mut();
            store.set_sidebar_collapsed(collapsed);
            // Off the click's thread: a save re-derives the whole disk copy —
            // keyring writes, encryption, two files — and a preference toggle
            // has no business stalling the frame it landed in.
            store.save_in_background();
        }
        // The sample the panel was showing is stale by however long it was folded away,
        // and the store still holds it. Re-reading now means the graph resumes where the
        // machine actually is rather than growing a cliff when the next tick lands.
        self.refresh(cx);
        self.animate_width_to(
            if collapsed { COLLAPSED_WIDTH } else { self.width },
            cx,
        );
    }

    /// Slide the column between its folded and unfolded width.
    ///
    /// Driven by a task rather than `with_animation` so it can be retargeted
    /// from wherever the user clicks mid-flight: replacing `anim` drops the
    /// previous task, and the slide resumes from wherever the width actually
    /// is. Each tick re-lays the workspace out, which is one terminal grid
    /// resize per frame — the cost a window resize already pays, spread over
    /// 180ms instead of landing as one snap.
    fn animate_width_to(&mut self, target: f32, cx: &mut Context<Self>) {
        let start = self.shown_width;
        if (target - start).abs() < 0.5 {
            self.shown_width = target;
            self.anim = None;
            cx.notify();
            return;
        }
        let begun = std::time::Instant::now();
        self.anim = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(ANIM_STEP).await;
                let t = (begun.elapsed().as_secs_f32() / ANIM_FOR.as_secs_f32()).min(1.0);
                // Ease-out cubic: fast where the eye is following, settling
                // where it lands.
                let eased = 1.0 - (1.0 - t).powi(3);
                let width = start + (target - start) * eased;
                let alive = this.update(cx, |view, cx| {
                    view.shown_width = width;
                    cx.notify();
                });
                if alive.is_err() || t >= 1.0 {
                    break;
                }
            }
        }));
    }

    /// The header: the connection's state, and the button that folds the column away.
    fn header(&self, cx: &Context<Self>) -> AnyElement {
        let shown = self.shown.clone();
        let theme = cx.theme();
        let dot = match shown.conn_state {
            1 => theme.success,
            2 => theme.warning,
            _ => theme.muted_foreground,
        };
        let host = shown.conn_host.clone();
        let copyable = !host.is_empty();
        let copied = self.copied;
        // A dialling session is not a quiet one: grey means "nothing known" only when
        // there is no session to know about. With one mid-connect, the dot pulses —
        // the same fact the tab strip's dot carries, so both panels tell the same lie
        // about nothing.
        let dialling = shown.conn_state == 0 && !host.is_empty();
        let dot_element = if dialling {
            div()
                .size_2()
                .flex_shrink_0()
                .rounded_full()
                .bg(theme.warning)
                .with_animation(
                    "sidebar-conn-dot",
                    Animation::new(Duration::from_millis(1200)).repeat(),
                    |dot, delta| {
                        let pulse = 0.5 - 0.5 * (std::f32::consts::TAU * delta).cos();
                        dot.opacity(0.35 + 0.65 * pulse)
                    },
                )
                .into_any_element()
        } else {
            div()
                .size_2()
                .flex_shrink_0()
                .rounded_full()
                .bg(dot)
                .into_any_element()
        };

        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(dot_element)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::i18n::t("状态", "Status")),
                    )
                    .child(
                        Button::new("collapse-sidebar")
                            .icon(Icon::new(IconName::ChevronRight))
                            .ghost()
                            .tooltip(crate::i18n::t(
                                "收起资源面板",
                                "Collapse the resource panel",
                            ))
                            .accessibility_label(crate::i18n::t(
                                "收起资源面板",
                                "Collapse the resource panel",
                            ))
                            .on_click(cx.listener(|this, _, _, cx| this.set_collapsed(true, cx))),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .id("sidebar-conn")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .px_1()
                            .py_0p5()
                            .rounded(theme.radius)
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .when(copyable, |this| {
                                this.cursor_pointer()
                                    .hover(|this| this.bg(theme.accent))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.copy_host(host.clone(), cx)
                                    }))
                            })
                            .child(shown.conn_text),
                    )
                    .when(copied, |this| {
                        // A fade in, not a pop: the confirmation mounts fresh
                        // each time `copied` turns true, so the animation runs
                        // per copy, and the label sits still for the rest of
                        // its 1400ms.
                        this.child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(theme.success)
                                .with_animation(
                                    "sidebar-copied",
                                    Animation::new(Duration::from_millis(150)),
                                    |label, delta| label.opacity(delta),
                                )
                                .child(crate::i18n::t("已复制", "Copied")),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The resource heading and the three bars under it.
    fn stats(&self, cx: &Context<Self>) -> AnyElement {
        let shown = self.shown.clone();
        let theme = cx.theme();
        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(shown.title),
                    )
                    // Only for a live remote session, which is the only session kind with
                    // a process sample and a detailed probe behind it: buttons that opened
                    // empty windows for a local shell would be offering something the app
                    // cannot do.
                    .when(shown.system_info_available, |this| {
                        this.child(
                            Button::new("sidebar-system-info")
                                .icon(Icon::new(IconName::Info))
                                .small()
                                .ghost()
                                .tooltip(crate::i18n::t("系统信息", "System information"))
                                .accessibility_label(crate::i18n::t(
                                    "系统信息",
                                    "System information",
                                ))
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.pending = Some(SidebarAction::ShowSystemInfo);
                                })),
                        )
                    })
                    .when(shown.proc_available, |this| {
                        this.child(
                            Button::new("sidebar-processes")
                                .icon(Icon::new(IconName::Activity))
                                .small()
                                .ghost()
                                .tooltip(crate::i18n::t("进程", "Processes"))
                                .accessibility_label(crate::i18n::t("进程", "Processes"))
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.pending = Some(SidebarAction::ShowProcesses);
                                })),
                        )
                    }),
            )
            .child(stat_row(
                "sidebar-cpu",
                SharedString::from("CPU"),
                shown.cpu,
                SharedString::default(),
                // The theme's chart palette rather than `primary`: these three bars are a
                // small data visualisation, and `primary` is the theme's *interactive*
                // colour (the Send button, a selected tab). A measurement drawn in the
                // same colour as a button is what makes a panel look shouty.
                theme.chart_2,
                theme,
            ))
            .child(stat_row(
                "sidebar-mem",
                crate::i18n::t("内存", "Memory").into(),
                shown.mem,
                shown.mem_detail,
                theme.success,
                theme,
            ))
            .child(stat_row(
                "sidebar-swap",
                crate::i18n::t("交换", "Swap").into(),
                shown.swap,
                shown.swap_detail,
                theme.warning,
                theme,
            ))
            .into_any_element()
    }

    /// The two throughput graphs: the active session's NIC above, this machine below.
    ///
    /// The session's graph is drawn only when a remote session is the subject — a local
    /// tab gets one graph, not this machine twice — and while the session's sample has
    /// yet to flow (connecting, or dropped) the space says so instead of inventing
    /// numbers.
    fn networks(&self, cx: &Context<Self>) -> AnyElement {
        let shown = self.shown.clone();
        let theme = cx.theme();
        let selector = shown.show_selector.then(|| self.iface_picker(cx));

        v_flex()
            .w_full()
            .gap_2()
            .when(shown.top_is_remote, |this| {
                if shown.top_waiting {
                    this.child(
                        div()
                            .w_full()
                            .h(px(GRAPH_HEIGHT))
                            .rounded_sm()
                            .border_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(crate::i18n::t("暂无远程数据", "No remote data yet")),
                    )
                } else {
                    this.child(net_graph(
                        shown.top_up,
                        shown.top_down,
                        &shown.top_history,
                        selector,
                        theme,
                    ))
                }
            })
            .child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(crate::i18n::t("本机", "Local")),
                    )
                    .child(net_graph(
                        shown.bot_up,
                        shown.bot_down,
                        &shown.bot_history,
                        None,
                        theme,
                    )),
            )
            .into_any_element()
    }

    /// The NIC selector: which interface the top graph follows.
    ///
    /// A popover rather than a `Select`, because the list is short, unsorted and has no
    /// search to do — and a `Select` would bring a delegate, a list state and a filter box
    /// for five interface names.
    fn iface_picker(&self, cx: &Context<Self>) -> AnyElement {
        let shown = self.shown.clone();
        // Copied out rather than borrowed: the popover's content closure has to be
        // `'static`, so it cannot hold a reference into the context it is built from.
        // `Hsla` and `Pixels` are `Copy`, so this costs nothing.
        let accent = cx.theme().accent;
        let muted = cx.theme().muted;
        let radius = cx.theme().radius;
        let weak: WeakEntity<Self> = cx.entity().downgrade();

        Popover::new("sidebar-iface")
            .trigger(
                Button::new("sidebar-iface-trigger")
                    .icon(Icon::new(IconName::ChevronDown))
                    .label(shown.selected.clone())
                    .small()
                    .ghost()
                    .tooltip(crate::i18n::t("选择网卡", "Choose an interface")),
            )
            .content(move |_, _, _| {
                let names = shown.ifaces.clone();
                let selected = shown.selected.clone();
                v_flex()
                    .min_w(px(140.))
                    .gap_0p5()
                    .children(names.into_iter().map(|name| {
                        let weak = weak.clone();
                        let chosen = name.clone();
                        let is_selected = name == selected;
                        h_flex()
                            .id(SharedString::from(format!("iface-{name}")))
                            .w_full()
                            .gap_2()
                            .items_center()
                            .px_2()
                            .py_1()
                            .rounded(radius)
                            .text_xs()
                            .cursor_pointer()
                            .when(is_selected, |this| this.bg(accent))
                            .hover(|this| this.bg(muted))
                            .child(div().flex_1().min_w_0().truncate().child(name))
                            .when(is_selected, |this| {
                                this.child(Icon::new(IconName::Check).size_3())
                            })
                            .on_click(move |_, _, cx| {
                                if let Some(view) = weak.upgrade() {
                                    view.update(cx, |view, cx| {
                                        view.select_iface(chosen.to_string());
                                        cx.notify();
                                    });
                                }
                            })
                    }))
            })
            .into_any_element()
    }

    /// Record which interface the user picked, in the same place the session's own
    /// samples are read from.
    ///
    /// Written into the shared status rather than into this view, because it is a fact
    /// about the session: the next sample's history grows from that interface's rates,
    /// and every other view of the same tab reads the same field.
    fn select_iface(&mut self, name: String) {
        let Some(tab) = self.active.clone() else {
            return;
        };
        if let Ok(mut statuses) = self.statuses.lock() {
            if let Some(status) = statuses.get_mut(&tab) {
                status.selected_iface = name;
                // The graph is about one interface's history, so switching interfaces
                // starts it over rather than splicing two links' rates into one line.
                status.net_hist = vec![0.0; NET_HISTORY_LEN];
            }
        }
        self.shown = self.project();
    }

    /// The filesystem list: mount, how full, and the figure behind the fill.
    fn disks(&self, cx: &Context<Self>) -> AnyElement {
        let shown = self.shown.clone();
        let theme = cx.theme();
        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(crate::i18n::t("路径", "Path")),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .child(crate::i18n::t("可用/大小", "Free/Total")),
                    ),
            )
            .child(
                v_flex()
                    .id("sidebar-disks")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_0p5()
                    .children(shown.disks.iter().map(|disk| disk_row(disk, theme))),
            )
            .into_any_element()
    }

    /// What the panel becomes when it is folded away: one button that brings it back.
    fn collapsed_strip(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .items_center()
            .pt_2()
            .bg(theme.sidebar)
            .border_l_1()
            .border_color(theme.border)
            .child(
                Button::new("expand-sidebar")
                    .icon(Icon::new(IconName::ChevronLeft))
                    .ghost()
                    .tooltip(crate::i18n::t("展开资源面板", "Expand the resource panel"))
                    .accessibility_label(crate::i18n::t(
                        "展开资源面板",
                        "Expand the resource panel",
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.set_collapsed(false, cx))),
            )
            .into_any_element()
    }
}

impl Render for SidebarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The strip only once the fold has arrived. While the width is still
        // sliding the panel keeps rendering at its full width and the shell
        // clips the column down over it — a strip that popped in at the first
        // frame would make the fold read as a content swap, not a slide.
        if self.showing_strip() {
            return self.collapsed_strip(cx);
        }

        let theme = cx.theme();
        let header = self.header(cx);
        let stats = self.stats(cx);
        let networks = self.networks(cx);
        let disks = self.disks(cx);

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .border_l_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    // One step tighter than a page: this column lives beside the
                    // work all day, and its four sections are furniture, not an
                    // article. Half a Tailwind step less per gap is the difference
                    // between a monitor and a poster.
                    .gap_1p5()
                    .p_2()
                    .child(header)
                    .child(divider(theme))
                    .child(stats)
                    .child(divider(theme))
                    .child(networks)
                    .child(divider(theme))
                    .child(disks),
            )
            .child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .pb_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .text_center()
                    .child(SharedString::from(format!(
                        "xenterm v{}",
                        env!("CARGO_PKG_VERSION")
                    ))),
            )
            .into_any_element()
    }
}

/// One CPU / Memory / Swap row: the label, a bar with its share inside it, and the
/// figure behind the bar.
///
/// The percentage sits *in* the bar rather than beside it because the row has three
/// things to say and only the width of a 220px column to say them in; putting the number
/// where the fill already is also makes the bar and the number read as one statement.
fn stat_row(
    id: &'static str,
    label: SharedString,
    percent: f32,
    detail: SharedString,
    color: Hsla,
    theme: &Theme,
) -> impl IntoElement {
    let percent = percent.clamp(0.0, 1.0);
    h_flex()
        .w_full()
        .gap(px(6.))
        .items_center()
        .child(
            // Wide enough for the longest label in either language ("Memory"), because a
            // clipped label is a row whose meaning depends on the language you read it in.
            div()
                .w(px(46.))
                .flex_shrink_0()
                .text_xs()
                .truncate()
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    Progress::new(id)
                        .value(percent * 100.0)
                        .color(color)
                        .h(px(8.))
                        .w_full(),
                ),
        )
        // The number reads from its own column rather than from inside the fill:
        // on a near-empty bar a label drawn inside it sat on blank track, and the
        // three rows' figures disagreed about where to look.
        .child(
            div()
                .w(px(34.))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!("{:.0}%", percent * 100.0))),
        )
        .child(
            // And wide enough for a used/total figure at its longest: `4.7G/31.6G`. The
            // figure is right-aligned against the panel's edge so the three rows' numbers
            // line up as one column, which is what makes them comparable at a glance.
            div()
                .w(px(68.))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_xs()
                .truncate()
                .text_color(theme.muted_foreground)
                .child(detail),
        )
}

/// One throughput graph: the two rates, an optional selector, and the history under them.
fn net_graph(
    up: SharedString,
    down: SharedString,
    history: &[f32],
    selector: Option<AnyElement>,
    theme: &Theme,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_1()
        .child(
            h_flex()
                .w_full()
                .gap_1()
                .items_center()
                // Arrows, not the words "up" and "down": the two rates are read by
                // position and colour far more often than by reading a label.
                .child(
                    Icon::new(IconName::ArrowUp)
                        .size_3()
                        .text_color(theme.success),
                )
                .child(div().text_xs().text_color(theme.muted_foreground).child(up))
                .child(
                    Icon::new(IconName::ArrowDown)
                        .size_3()
                        .text_color(theme.chart_2),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(down),
                )
                .child(div().flex_1())
                .children(selector),
        )
        .child(sparkline(history, theme))
}

/// The history as vertical bars, one per sample, scaled against the window's own peak.
///
/// Bars rather than a line, matching the original: at 60 samples across ~180px a line is
/// a smear, while bars stay countable — which is what makes a spike read as one sample or
/// as a sustained rate.
fn sparkline(history: &[f32], theme: &Theme) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(px(GRAPH_HEIGHT))
        .flex_shrink_0()
        .gap(px(1.))
        .items_end()
        .rounded(theme.radius)
        .overflow_hidden()
        .bg(theme.muted)
        .children(history.iter().map(|value| {
            let value = value.clamp(0.0, 1.0);
            div()
                .flex_1()
                .h(relative(value))
                .rounded(px(1.))
                .bg(theme.chart_2)
        }))
}

/// One filesystem: a fill drawn behind the mount's own name.
fn disk_row(disk: &DiskUsage, theme: &Theme) -> impl IntoElement {
    // The fill carries the same reading as the bar's length: blue while there is room,
    // yellow near full, red when there is not. Two signals for one fact, because a bar
    // that is 92% full and one that is 96% full look alike at this width.
    let fill = if disk.percent > 0.9 {
        theme.danger
    } else if disk.percent > 0.75 {
        theme.warning
    } else {
        theme.chart_2
    };
    // One anatomy for every measured row — label, bar, figures — which is what a
    // scanner's eye needs. The old row painted the fill across the row's whole
    // background, where a bar behind text reads as a selected row.
    h_flex()
        .w_full()
        .h(px(20.))
        .flex_shrink_0()
        .gap(px(6.))
        .items_center()
        .child(
            div()
                .w(px(46.))
                .flex_shrink_0()
                .text_xs()
                .truncate()
                .child(SharedString::from(disk.path.clone())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .relative()
                .rounded(px(2.))
                .bg(theme.muted)
                .h(px(8.))
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h_full()
                        .w(relative(disk.percent.clamp(0.0, 1.0)))
                        .bg(fill),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(disk.detail.clone())),
        )
}

fn divider(theme: &Theme) -> impl IntoElement {
    div().w_full().h(px(1.)).flex_shrink_0().bg(theme.border)
}

/// The header dot's colour for a session state.
fn conn_state(state: u8) -> u8 {
    match state {
        1 => 1,
        2 => 2,
        _ => 0,
    }
}

/// What the header says about a session: the host while it is up, and the host with what
/// went wrong on either side of that.
fn connection_text(status: &TabStatus) -> SharedString {
    match status.state {
        1 => status.host.clone().into(),
        2 => format!(
            "{} {}",
            status.host,
            crate::i18n::t("已断开", "disconnected")
        )
        .into(),
        _ => format!("{} {}", crate::i18n::t("连接中", "Connecting"), status.host).into(),
    }
}

/// A used/total pair as a fraction, with a zero total meaning "nothing to report" rather
/// than a division.
fn fraction(used: u64, total: u64) -> f32 {
    if total > 0 {
        used as f32 / total as f32
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopped_or_unknown_session_never_claims_a_used_fraction() {
        assert_eq!(fraction(0, 0), 0.0, "no total is not a full disk");
        assert!((fraction(1, 4) - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn the_header_names_what_happened_to_the_session() {
        let mut status = TabStatus {
            host: "root@example.com:22".to_string(),
            state: 1,
            ..Default::default()
        };
        assert_eq!(connection_text(&status).as_ref(), "root@example.com:22");
        assert_eq!(conn_state(status.state), 1);

        // Asserted through `t` rather than against the English wording: the panel follows
        // the interface language, and a test that pinned one language would fail the day
        // someone runs the suite in the other.
        status.state = 2;
        assert!(connection_text(&status).contains(crate::i18n::t("已断开", "disconnected")));
        assert_eq!(conn_state(status.state), 2);

        status.state = 0;
        assert!(connection_text(&status).contains(crate::i18n::t("连接中", "Connecting")));
        assert_eq!(conn_state(status.state), 0, "grey while nothing is known");
    }
}
