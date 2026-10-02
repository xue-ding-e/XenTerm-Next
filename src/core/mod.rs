// Framework-agnostic application core.
//
// Nothing in here may name a UI toolkit or any of its types. The UI shell
// (`crate::ui`) projects this state into whatever model that toolkit wants, and
// the out-of-process plugin host reads and commands the same surface.
//
// This module exists because state ownership had split. The per-tab terminal and
// transfer rows the interface observed were not views onto Rust state — they *were*
// the authoritative stores for per-tab status text, scroll position, SFTP
// listings, and transfer bookkeeping. Code read them back out of the toolkit's own
// model objects, downcast them, and mutated rows in place. That made the UI
// framework the source of truth for application state, which is why swapping
// frameworks meant rewriting logic and not just widgets.
//
// Each type reified here moves one of those stores back into Rust. The toolkit
// model then becomes a derived projection that can be rebuilt from scratch at
// any time, and the same projection step is what a plugin capability or a view
// consumes.
//
// Each type reified here moves one of those stores back into Rust. The toolkit
// model then becomes a derived projection that can be rebuilt from scratch at
// any time, and the same projection step is what a plugin capability or a view
// consumes.

#[path = "struct/transfer.rs"]
mod transfer;

#[path = "struct/sftp.rs"]
mod sftp;

#[path = "struct/font_zoom.rs"]
mod font_zoom;

#[path = "struct/event_sink.rs"]
mod event_sink;

#[path = "struct/tab.rs"]
mod tab;

#[path = "struct/session_row.rs"]
mod session_row;
// The session editor's working state. This was once gated because the editor's form
// had its own generated draft type; that type is gone, the editor is the only form
// left, and the type is named in every build.
#[path = "struct/session_draft.rs"]
mod session_draft;

#[path = "impls/transfer_store.rs"]
mod transfer_store;

#[path = "impls/batch.rs"]
pub mod batch;

#[path = "impls/fonts.rs"]
pub mod fonts;

#[path = "impls/history.rs"]
pub mod history;

#[path = "impls/highlight.rs"]
pub mod highlight;

#[path = "impls/quick.rs"]
pub mod quick;

#[path = "impls/ssh_import.rs"]
pub mod ssh_import;

#[path = "impls/tunnel.rs"]
pub mod tunnel;

#[path = "impls/sftp_listing.rs"]
mod sftp_listing;

// Only what consumers name. Types reachable through a method call on these
// (`BaseZoom`, `SftpSortDir`, `TransferPhase`) stay unexported until one is.
pub use event_sink::EventSink;
pub use font_zoom::FontZoom;
pub use session_draft::{PortForwardDraft, SessionDraft, TriggerDraft};
pub use session_row::SessionRow;
pub use sftp::{parent_path, SftpColumn};
// Named by the SFTP panel, which draws one row per file. Listed explicitly rather
// than left to `SftpListing`'s methods so the panel's row type is part of this module's
// surface on purpose: the listing's accessors return it.
pub use sftp::SftpFile;
pub use sftp_listing::SftpListing;
pub use tab::{TabKind, TabMeta};
// Named by the transfer manager, which colours a row by what happened to it;
// `Transfer` reports the same fact through its own methods.
pub use transfer::Transfer;
pub use transfer::TransferPhase;
pub use transfer_store::TransferStore;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Shared handle to one window's transfer store.
///
/// `Arc<Mutex<_>>` rather than `Rc<RefCell<_>>` because the session pump threads
/// produce transfer progress off the UI thread while the reducer consumes it on
/// it — the same reason `TermBuffers` and `TabStatuses` are mutex-guarded.
pub type TransferRecords = Arc<Mutex<TransferStore>>;

/// One window's SFTP panel listings, keyed by tab id.
///
/// Same shape as `TermBuffers`, `TabStatuses` and `SftpHandles`, which matters
/// beyond consistency: `tab_transfer::move_locked_entry` relocates a tab's entry
/// from one such map to another when a tab is dragged out into its own window or
/// merged into an existing one, so a listing keyed the same way moves with its
/// tab for free.
pub type SftpListings = Arc<Mutex<HashMap<String, SftpListing>>>;

