mod access;
pub(crate) mod approval;
#[cfg(all(windows, not(feature = "desktop")))]
mod native_approval;
pub(crate) mod risk;

pub(crate) use access::Frontend;
pub(crate) use approval::{
    prune_audit_in, queue_dir, resolve_manual, resolve_timeout_in, scan_in, seconds_left,
    AuditRecord, MAX_PENDING,
};

