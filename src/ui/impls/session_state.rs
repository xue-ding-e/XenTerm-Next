//! The window's shared session state, and the one place a session is started.
//!
//! It holds only the part that names no toolkit, so a session started from this
//! shell runs against the same stores, routes and gates any other entry point
//! would use: the route itself is `crate::session::protocol::TabRoute`, and the
//! session vocabulary is that module's too.
//!
//! Being `Clone` is why the maps are `Arc<Mutex<_>>`: a session's pump threads outlive
//! the call that starts them and reach into these from their own threads, while the
//! store stays an `Rc<RefCell<_>>` because only this thread ever touches it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::config::{ConfigStore, Session};
use crate::core::{EventSink, SftpListing, SftpListings};
use crate::resource::TabStatuses;
use crate::session::protocol::{RuntimeTunnelInfo, SessionHandle, TabRoute, TabRoutes};
use crate::sftp::{SftpHandles, SftpLastCwd};
use crate::terminal::{RenderGates, TermBuffers};

use super::event_sink::{GpuiEventSink, UiMessage};

/// The live tunnels of each tab, keyed by tab id.
pub(crate) type TabTunnels = Arc<Mutex<HashMap<String, Vec<RuntimeTunnelInfo>>>>;

/// The directory tree of each tab, keyed by tab id.
///
/// Flattened with a depth by the session rather than nested here: the panel draws one row per
/// node, and a tree the view had to walk would be a second copy of the session's own answer.
pub(crate) type TabTrees = Arc<Mutex<HashMap<String, Vec<crate::session::protocol::RemoteTreeNode>>>>;

/// The window id prompts are tagged with.
///
/// A constant while this shell has one window. The prompt queues are shared and
/// the outstanding design that would multiplex several windows over them tags
/// entries with the window that owns them (#multi-window); one window does not
/// need the distinction, but the tag has to be *some* value — and a constant is
/// honest about there being one window rather than pretending to a registry this
/// shell does not have.
pub(crate) const WINDOW_ID: u64 = 1;

/// Everything a session needs that outlives the call which starts it.
#[derive(Clone)]
pub(crate) struct SessionState {
    /// The shell's tokio runtime.
    ///
    /// Shared with every `ConnectCtx` rather than created per session, because the SFTP
    /// bootstrap task and the pump threads run on it and outlive the connect call:
    /// they have to be tasks of the runtime the shell shuts down, or a window closing
    /// would leave workers running on a runtime nobody is waiting for.
    runtime: Arc<tokio::runtime::Runtime>,
    /// Live sessions, keyed by tab id.
    pub(crate) handles: Rc<RefCell<HashMap<String, SessionHandle>>>,
    /// Parsed terminal screens.
    pub(crate) bufs: TermBuffers,
    /// Per-tab render gates, shared with the sink so a producer's ticket and the
    /// view's flush are the same gate.
    pub(crate) gates: RenderGates,
    /// Per-tab status, which the sampler and any status line read.
    pub(crate) statuses: TabStatuses,
    /// SFTP command channels, keyed by tab id.
    pub(crate) sftp_handles: SftpHandles,
    /// The last directory the shell reported per tab.
    pub(crate) sftp_last_cwd: SftpLastCwd,
    /// Authoritative SFTP listings, keyed by tab id.
    ///
    /// Unread in this shell so far, and it has to exist anyway: `ConnectCtx` is handed a
    /// clone, so this is the map the session's SFTP events are written into, and it is
    /// the one the SFTP panel will project from when that view lands (#18). Removing it
    /// now would mean the panel reading a different map from the events.
    #[allow(dead_code)]
    pub(crate) sftp_listings: SftpListings,
    /// The directory tree of each tab, as the session last reported it.
    ///
    /// Written by the tab's own view — `SessionEvent::SftpTreeUpdate` arrives with the rest of
    /// a session's events — and read by the file panel, which is drawn by the shell and so
    /// cannot receive them itself. One map rather than a slot per tab, because every tab that
    /// has an SFTP session has a tree.
    pub(crate) sftp_trees: TabTrees,
    /// The live tunnels of each tab, as the SSH layer last reported them.
    ///
    /// Written by the tab's own view — `SessionEvent::TunnelUpdate` arrives with the rest
    /// of a session's events — and read by the tunnel panel, which describes a session
    /// rather than drawing it and so cannot receive them itself.
    pub(crate) tunnels: TabTunnels,
    /// The text of the last remote file a session opened for the built-in viewer.
    ///
    /// Written by the tab's own view, like the listing and the tunnels, and read by the
    /// shell, which is the only place with a window to build a view in. One slot rather than
    /// a map: two files open at once would be two overlays, and the panel that asked for the
    /// second is behind the first.
    pub(crate) opened_file: Arc<Mutex<Option<OpenedFile>>>,
    /// Every transfer this window has seen, live and finished.
    ///
    /// The shell keeps this store rather than the view drawing it, because the row
    /// identity, the ordering and the in-progress count are bookkeeping, not drawing, and the
    /// transfer manager is a projection of it. It lives on the session state because the
    /// session's events are what fill it.
    pub(crate) transfers: crate::core::TransferRecords,
    /// Whether SFTP follows the shell's directory.
    pub(crate) sftp_follow_cd: Arc<AtomicBool>,
    /// The size the terminal last reported, so a new session's first PTY is not 80x24.
    pub(crate) last_term_size: Arc<Mutex<(u32, u32)>>,
    /// Process-wide tab delivery routes.
    pub(crate) tab_routes: TabRoutes,
    /// The saved sessions.
    pub(crate) store: Rc<RefCell<ConfigStore>>,
}

/// A remote file whose text has arrived, waiting for the shell to open it.
///
/// The session reports the text; the shell builds the view, because a view needs a window
/// and a session's event handler has only the view it is drawing.
pub(crate) struct OpenedFile {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) content: String,
    pub(crate) editable: bool,
    pub(crate) error: String,
}

impl SessionState {
    /// Assemble the state for one window.
    ///
    /// `runtime` is the shell's, not a new one — `ConnectCtx` hands it to the SFTP
    /// bootstrap task, which outlives the connect call, so the two have to agree about
    /// which runtime is shutting down when the window closes.
    ///
    /// No sink is built here any more: a sink belongs to a tab, because the channel
    /// behind it is what delivers that tab's events to the view drawing it. Use
    /// [`SessionState::sink_for`] when opening one.
    pub(crate) fn new(
        runtime: Arc<tokio::runtime::Runtime>,
        gates: RenderGates,
        store: Rc<RefCell<ConfigStore>>,
    ) -> Self {
        let bufs: TermBuffers = Arc::new(Mutex::new(HashMap::new()));
        let statuses: TabStatuses = Arc::new(Mutex::new(HashMap::new()));
        let sftp_listings: SftpListings = Arc::new(Mutex::new(HashMap::new()));

        Self {
            runtime,
            handles: Rc::new(RefCell::new(HashMap::new())),
            bufs,
            gates,
            statuses,
            sftp_handles: Arc::new(Mutex::new(HashMap::new())),
            sftp_last_cwd: Arc::new(Mutex::new(HashMap::new())),
            sftp_listings,
            sftp_trees: Arc::new(Mutex::new(HashMap::new())),
            tunnels: Arc::new(Mutex::new(HashMap::new())),
            opened_file: Arc::new(Mutex::new(None)),
            transfers: Arc::new(Mutex::new(crate::core::TransferStore::new())),
            sftp_follow_cd: Arc::new(AtomicBool::new(true)),
            last_term_size: Arc::new(Mutex::new((80, 24))),
            tab_routes: Arc::new(Mutex::new(HashMap::new())),
            store,
        }
    }

    /// Start `session` in a new tab, using the shared connect path.
    ///
    /// Four session kinds, the pump threads, the SFTP bootstrap and the route
    /// registration all live there, and this only assembles the `ConnectCtx` it
    /// wants. That is what the `ConnectCtx` decoupling bought.
    ///
    /// `sink` is the destination for *this tab's* events, which is the channel owned
    /// by the view that draws it. One queue for the whole window would deliver every
    /// tab's events to every view.
    ///
    /// `monitoring` asks the session for remote resource samples, and is the caller's
    /// decision rather than this one's: a window that has the resource panel folded away
    /// should not be running a remote command per session for numbers nobody can see
    /// (#127). The shell passes what its panel reports.
    pub(crate) fn connect(
        &self,
        tab_id: &str,
        session: Session,
        sink: Arc<dyn EventSink>,
        monitoring: bool,
    ) {
        // A gate per tab, in the map the sink shares, so `request_render` and the
        // view's flush are talking about one gate rather than two similar objects.
        if let Ok(mut gates) = self.gates.lock() {
            gates.insert(
                tab_id.to_string(),
                Arc::new(crate::terminal::TabRenderGate::new(RENDER_MIN_INTERVAL)),
            );
        }

        // Seed the tab's status entry, because everything that later *updates* it looks
        // it up rather than inserting: `SessionEvent::Connected` sets `state = 1` on an
        // existing entry, and a missing one would leave the tab's status dot reading
        // "never connected" for the whole life of a live session. The connect path
        // deliberately only updates, so the entry that starts the session is made here.
        if let Ok(mut statuses) = self.statuses.lock() {
            statuses.insert(
                tab_id.to_string(),
                crate::resource::TabStatus {
                    host: session.host.clone(),
                    user: session.user.clone(),
                    session_id: session.id.clone(),
                    state: 0,
                    monitor_generation: {
                        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    },
                    monitor_state: if session.kind != crate::config::SessionKind::Ssh || session.disable_shell_integration {
                        crate::session::protocol::ResourceMonitorState::Unsupported
                    } else if !monitoring {
                        crate::session::protocol::ResourceMonitorState::Paused
                    } else {
                        crate::session::protocol::ResourceMonitorState::Waiting
                    },
                    monitor_started_at: Some(std::time::Instant::now()),
                    is_local: session.kind == crate::config::SessionKind::Local,
                    ..Default::default()
                },
            );
        }

        // This tab's delivery route, built here rather than inside the connect path:
        // the stores it names are this state's, and the route is the one thing both
        // the pump threads and a later detach/merge hold. Passing it in is what
        // stopped `ConnectCtx` carrying a second copy of every field.
        let route = Arc::new(Mutex::new(TabRoute {
            sink,
            window_id: WINDOW_ID,
            bufs: self.bufs.clone(),
            sftp_handles: self.sftp_handles.clone(),
            sftp_last_cwd: self.sftp_last_cwd.clone(),
            follow_cd: self.sftp_follow_cd.clone(),
        }));

        let ctx = crate::session::ConnectCtx {
            monitoring_enabled: monitoring,
            route,
            window_id: WINDOW_ID,
            runtime: self.runtime.clone(),
            handles: self.handles.clone(),
            tab_statuses: self.statuses.clone(),
            last_term_size: self.last_term_size.clone(),
            store: self.store.clone(),
            tab_routes: self.tab_routes.clone(),
        };

        crate::app::session_runtime::start_session_in_tab(tab_id, session, &ctx);
    }

    /// This window's event destination for a new tab.
    ///
    /// Per tab rather than one shared sink, because the channel is what carries a
    /// tab's events to the view that draws it. The gates *are* shared: they are keyed
    /// by tab id and both sides look theirs up, so a per-tab sink still settles the
    /// gate its own producer is waiting on.
    pub(crate) fn sink_for(
        &self,
        ui: tokio::sync::mpsc::UnboundedSender<UiMessage>,
    ) -> Arc<dyn EventSink> {
        Arc::new(GpuiEventSink::new(ui, self.gates.clone()))
    }

    /// The listing for `tab_id`, for the panel to project.
    ///
    /// The store is written by the terminal view as the session's listing events
    /// arrive — it is the view that the sink delivers them to — and read from here.
    /// One writer, one reader, and the panel draws whatever the last event left.
    pub(crate) fn listing(&self, tab_id: &str) -> SftpListing {
        self.sftp_listings
            .lock()
            .ok()
            .and_then(|map| map.get(tab_id).cloned())
            .unwrap_or_default()
    }

    /// The named tab's listing generation, without cloning the listing. This is
    /// what the shell's per-frame drain reads — copying every file row per
    /// frame is not a price the comparison should pay.
    pub(crate) fn listing_generation(&self, tab_id: &str) -> u64 {
        self.sftp_listings
            .lock()
            .ok()
            .and_then(|map| map.get(tab_id).map(|listing| listing.generation()))
            .unwrap_or(0)
    }

    /// Retire a tab: drop every per-tab entry this state holds.
    ///
    /// These maps are keyed by tab id and read by panels that follow the
    /// *active* tab — the resource sidebar, the process and system-info
    /// windows, the tunnel panel — so a closed tab's entry is a dead session
    /// still answering those reads. The resource sidebar kept describing a
    /// closed connection after a switch or a close because its projection
    /// found the stale entry under the tab it was still pointed at. One
    /// method rather than a scatter of removes at the call site: a new
    /// per-tab store added to this state gets retired here, where the
    /// compiler's own read of `self` makes it hard to forget.
    pub(crate) fn end_tab(&self, tab_id: &str) {
        self.handles.borrow_mut().remove(tab_id);
        if let Ok(mut gates) = self.gates.lock() {
            gates.remove(tab_id);
        }
        if let Ok(mut statuses) = self.statuses.lock() {
            statuses.remove(tab_id);
        }
        if let Ok(mut bufs) = self.bufs.lock() {
            bufs.remove(tab_id);
        }
        if let Ok(mut sftp) = self.sftp_handles.lock() {
            sftp.remove(tab_id);
        }
        if let Ok(mut cwd) = self.sftp_last_cwd.lock() {
            cwd.remove(tab_id);
        }
        if let Ok(mut listings) = self.sftp_listings.lock() {
            listings.remove(tab_id);
        }
        if let Ok(mut trees) = self.sftp_trees.lock() {
            trees.remove(tab_id);
        }
        if let Ok(mut tunnels) = self.tunnels.lock() {
            tunnels.remove(tab_id);
        }
    }
}

/// The same 30 Hz ceiling `crate::app` names `RENDER_MIN_INTERVAL`, so both shells
/// pace a tab identically under load.
const RENDER_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

#[cfg(test)]
mod tests {
    use super::*;

    /// A state with one tab seeded into every per-tab store, the shape a live
    /// session leaves behind.
    fn state_with_seeded_tab(runtime: &Arc<tokio::runtime::Runtime>) -> SessionState {
        let gates: RenderGates = Arc::new(Mutex::new(HashMap::new()));
        let store = Rc::new(RefCell::new(ConfigStore {
            path: std::env::temp_dir().join(format!("ms-end-tab-{}.db", uuid::Uuid::new_v4())),
            backup_dir: None,
            cache: crate::config::ConfigFile::default(),
            key: [7u8; 32],
            keyring_enabled: false,
            saved_state: Mutex::new(crate::config::SavedState::default()).into(),
        }));
        let state = SessionState::new(runtime.clone(), gates, store);

        let (commands, _rx) = tokio::sync::mpsc::unbounded_channel();
        state.handles.borrow_mut().insert(
            "tab-1".into(),
            SessionHandle {
                tab_id: "tab-1".into(),
                commands,
                join: runtime.spawn(async {}),
            },
        );
        state
            .gates
            .lock()
            .unwrap()
            .insert("tab-1".into(), Arc::new(crate::terminal::TabRenderGate::new(RENDER_MIN_INTERVAL)));
        state
            .statuses
            .lock()
            .unwrap()
            .insert("tab-1".into(), crate::resource::TabStatus {
                host: "old@example.com:22".into(),
                state: 1,
                ..Default::default()
            });
        state
            .bufs
            .lock()
            .unwrap()
            .insert("tab-1".into(), Arc::new(Mutex::new(crate::terminal::TermBuffer::new(24, 80))));
        let (commands, _rx) = tokio::sync::mpsc::unbounded_channel();
        state.sftp_handles.lock().unwrap().insert(
            "tab-1".into(),
            crate::sftp::SftpHandle {
                commands,
                join: runtime.spawn(async {}),
            },
        );
        state
            .sftp_last_cwd
            .lock()
            .unwrap()
            .insert("tab-1".into(), "/srv".into());
        state
            .sftp_listings
            .lock()
            .unwrap()
            .insert("tab-1".into(), SftpListing::default());
        state
            .sftp_trees
            .lock()
            .unwrap()
            .insert("tab-1".into(), Vec::new());
        state.tunnels.lock().unwrap().insert("tab-1".into(), Vec::new());
        state
    }

    /// Retiring a tab must leave no entry behind: the resource sidebar and the
    /// detached windows read these maps by the active tab's id, and a closed
    /// tab's residue is what made the sidebar keep describing a connection the
    /// user had already closed.
    #[test]
    fn end_tab_retires_every_per_tab_entry() {
        let runtime = Arc::new(tokio::runtime::Runtime::new().expect("runtime"));
        let state = state_with_seeded_tab(&runtime);

        state.end_tab("tab-1");

        assert!(
            state.handles.borrow().get("tab-1").is_none(),
            "the session handle must go with the tab"
        );
        assert!(state.gates.lock().unwrap().get("tab-1").is_none());
        assert!(
            state.statuses.lock().unwrap().get("tab-1").is_none(),
            "the sidebar reads this — a stale entry is a dead connection still answering"
        );
        assert!(state.bufs.lock().unwrap().get("tab-1").is_none());
        assert!(state.sftp_handles.lock().unwrap().get("tab-1").is_none());
        assert!(state.sftp_last_cwd.lock().unwrap().get("tab-1").is_none());
        assert!(state.sftp_listings.lock().unwrap().get("tab-1").is_none());
        assert!(state.sftp_trees.lock().unwrap().get("tab-1").is_none());
        assert!(state.tunnels.lock().unwrap().get("tab-1").is_none());
    }

    /// Retiring one tab must not disturb another's entries.
    #[test]
    fn end_tab_leaves_other_tabs_alone() {
        let runtime = Arc::new(tokio::runtime::Runtime::new().expect("runtime"));
        let state = state_with_seeded_tab(&runtime);
        state
            .statuses
            .lock()
            .unwrap()
            .insert("tab-2".into(), crate::resource::TabStatus {
                host: "other@example.com:22".into(),
                state: 1,
                ..Default::default()
            });

        state.end_tab("tab-1");

        assert!(
            state.statuses.lock().unwrap().contains_key("tab-2"),
            "a sibling tab's status is not the closed tab's to take"
        );
    }
    #[test]
    fn resource_monitor_reconnect_gets_a_fresh_generation_and_empty_sample() {
        use crate::session::protocol::ResourceMonitorState as State;
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let state = state_with_seeded_tab(&runtime);
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let sink = state.sink_for(sender);
        let mut session = crate::config::Session::new_empty();
        session.kind = crate::config::SessionKind::Ssh;
        session.jump_session_ids = vec!["missing-resource-test-hop".into()];
        // Invalid explicit route resolution prevents any transport/network start.
        state.connect("tab-1", session.clone(), sink.clone(), true);
        let first_generation = state.statuses.lock().unwrap()["tab-1"].monitor_generation;
        {
            let mut statuses = state.statuses.lock().unwrap();
            let status = statuses.get_mut("tab-1").unwrap();
            status.monitor_state = State::Available;
            status.sampled_at = Some(std::time::Instant::now());
            status.cpu_sampled = true;
            status.mem_total_kib = 4096;
        }
        state.connect("tab-1", session.clone(), sink.clone(), true);
        {
            let statuses = state.statuses.lock().unwrap();
            let status = &statuses["tab-1"];
            assert_ne!(first_generation, status.monitor_generation);
            assert_eq!(status.monitor_state, State::Waiting);
            assert!(status.sampled_at.is_none());
            assert!(!status.cpu_sampled);
            assert_eq!(status.mem_total_kib, 0);
        }
        state.connect("tab-1", session.clone(), sink.clone(), false);
        assert_eq!(state.statuses.lock().unwrap()["tab-1"].monitor_state, State::Paused);
        session.disable_shell_integration = true;
        state.connect("tab-1", session, sink, true);
        assert_eq!(state.statuses.lock().unwrap()["tab-1"].monitor_state, State::Unsupported);
    }

}
