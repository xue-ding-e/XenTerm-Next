#[path = "impls/capped_stream.rs"]
mod capped_stream;
#[path = "impls/sftp.rs"]
mod sftp;
#[path = "struct/transfer.rs"]
mod transfer;

pub(crate) use sftp::*;
pub(crate) use transfer::{DownloadConflict, SftpCommand, SftpHandles, SftpLastCwd};
// Automation owns a handle so cancellation can abort its worker.
pub(crate) use transfer::SftpHandle;
