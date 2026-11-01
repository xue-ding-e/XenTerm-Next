use std::fs::File;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// One log file capped at `cap` bytes (truncate-and-restart when full).
pub struct CappedFile {
    pub(super) path: PathBuf,
    pub(super) file: File,
    pub(super) written: u64,
    pub(super) cap: u64,
}

/// Shared writer adapter used by the tracing formatter.
#[derive(Clone)]
pub struct CappedWriter(pub(super) Arc<Mutex<CappedFile>>);

pub struct Guard<'a>(pub(super) std::sync::MutexGuard<'a, CappedFile>);
