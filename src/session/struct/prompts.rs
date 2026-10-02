use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use tokio::runtime::Runtime;

use crate::config::ConfigStore;
use crate::resource::TabStatuses;
use crate::session::protocol::{
    CredentialResponder, HostKeyResponder, MfaResponder, SessionHandle, TabRoute, TabRoutes,
};

/// Shared dependencies for starting or reconnecting a session tab.
///
/// Framework-neutral, and deliberately so: the whole connect path below this —
/// `start_session_in_tab`, the four session kinds' spawners, the pump threads —
/// names nothing from a UI toolkit. A second frontend reuses all of it by
/// building one of these. The last thing that made that untrue was a weak handle
/// to the window, whose only use was reading the window's sidebar/zen state to
/// decide whether to sample host resources; that decision is now made by whoever
/// opens the connection and passed in as [`ConnectCtx::monitoring_enabled`].
///
/// What the context deliberately does *not* do is enumerate the per-tab stores.
/// `sink`, `window_id`, the term buffers, the SFTP handles, the last reported cwd
/// and the follow-cd switch are all fields of [`TabRoute`], so this carries the
/// route those pumps already re-read per batch instead of a second copy of its
/// fields. Adding a store to a tab used to mean editing this struct, `TabRoute`,
/// the UI's session state, and two function signatures that only passed them
/// along; now it means editing `TabRoute`.
pub(crate) struct ConnectCtx {
    /// Whether to sample the host's CPU/memory for this session.
    ///
    /// Off when the window is collapsed to a sidebar or in zen mode: the numbers
    /// are not on screen, and a hidden sampler is a remote process per session for
    /// nothing (#127). A UI fact, so the UI answers it at connect time rather than
    /// this struct holding a window handle to ask later.
    pub(crate) monitoring_enabled: bool,
    /// The owning window's event destination, and every store the pumps reach
    /// through it, for this tab.
    ///
    /// The caller builds it — it is the same `Arc<Mutex<TabRoute>>` a detach/merge
    /// would later rewrite to retarget the running pumps — and the pump clones it
    /// once per batch.
    pub(crate) route: Arc<Mutex<TabRoute>>,
    /// Registry id of the window this session belongs to. Used to tag
    /// connect-time prompts (host key / credentials / MFA) so their dialogs open —
    /// and abort on close — in the owning window rather than whichever window
    /// happens to resolve the global queue front first (#multi-window).
    pub(crate) window_id: u64,
    pub(crate) runtime: Arc<Runtime>,
    /// Live sessions, keyed by tab id. The connect path inserts this tab's handle
    /// here, so unlike the stores in `route` it is not merely forwarded.
    pub(crate) handles: Rc<RefCell<HashMap<String, SessionHandle>>>,
    /// Read directly by the reconnect-on-Enter path to recognise a dead session,
    /// so unlike the stores folded into `route` this one is not merely forwarded.
    pub(crate) tab_statuses: TabStatuses,
    /// The size the terminal last reported, so a resumed session's PTY is not
    /// 80x24. Narrower than the stores above, because only the connect path reads
    /// it.
    pub(crate) last_term_size: Arc<Mutex<(u32, u32)>>,
    pub(crate) store: Rc<RefCell<ConfigStore>>,
    /// Process-wide tab delivery routes. Starting a session registers its
    /// route here so a later detach/merge can retarget the running pumps
    /// at another window without respawning them (#tab-detach).
    pub(crate) tab_routes: TabRoutes,
}

pub(crate) struct PendingHostKey {
    /// Registry id of the window that owns this prompt's session(s); the
    /// dialog is shown there and the entry is aborted if that window closes.
    pub(crate) window_id: u64,
    pub(crate) host: String,
    pub(crate) port: u16,
    /// Algorithm and fingerprint of the key this prompt is *about*.
    ///
    /// Carried because `host:port` is not an answer about a key: the same address
    /// can present a different one, which is exactly what a changed-key prompt
    /// means. Both the run-scoped decision memory and the merge rule below are
    /// keyed on this pair as well as the address, so a decision made about one key
    /// is never handed to another.
    pub(crate) key_type: String,
    pub(crate) fingerprint: String,
    pub(crate) changed: bool,
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) detail: String,
    pub(crate) confirm_label: String,
    pub(crate) responders: Vec<HostKeyResponder>,
}

pub(crate) struct PendingCred {
    /// Owning window's registry id (see `PendingHostKey::window_id`).
    pub(crate) window_id: u64,
    pub(crate) session_id: String,
    pub(crate) host: String,
    pub(crate) user: String,
    pub(crate) need_user: bool,
    pub(crate) need_password: bool,
    pub(crate) responders: Vec<CredentialResponder>,
}

pub(crate) struct PendingMfa {
    /// Owning window's registry id (see `PendingHostKey::window_id`).
    pub(crate) window_id: u64,
    pub(crate) session_id: String,
    pub(crate) host: String,
    pub(crate) prompt: String,
    pub(crate) echo: bool,
    pub(crate) responders: Vec<MfaResponder>,
}
