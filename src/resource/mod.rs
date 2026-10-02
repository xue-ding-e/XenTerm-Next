#[path = "impls/system.rs"]
pub(crate) mod system;
#[path = "struct/system.rs"]
mod system_types;

pub(crate) use system_types::{
    LocalSnap, NetHist, SystemSampler, SystemSnapshot, TabStatus,
    TabStatuses,
};
// Named by the system-information window, which projects these rows.
pub(crate) use system_types::InfoRow;
// Named by the same window: the panel builds its own `DiskInfo` rows from the derived
// lists, and these two are the types it names on the way in.
pub(crate) use system_types::{DiskUsage, ProcRow};

// The derived views a resource panel draws. They live beside the sampler rather than
// inside a frontend because the arithmetic — a used fraction, an auto-scaled
// history, which NIC is on top — is the same question whichever toolkit asks it, and
// two answers to it are two panels that disagree.
pub(crate) use system::{
    chunked_rows, connection_host, cpu_usage_rows, disk_usage, normalized_history, overview_rows,
    proc_rows, push_ring, selected_iface, single_row, tuple5_rows,
    NET_HISTORY_LEN,
};
