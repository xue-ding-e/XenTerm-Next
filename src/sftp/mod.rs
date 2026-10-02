#[path = "impls/capped_stream.rs"]
mod capped_stream;
#[path = "impls/sftp.rs"]
mod sftp;
#[path = "struct/transfer.rs"]
mod transfer;

pub(crate) use sftp::*;
pub(crate) use transfer::{DownloadConflict, SftpCommand, SftpHandles, SftpLastCwd};
// The handle struct itself is only ever built by tests; production code holds
// the map type and talks through the command channel.
#[cfg(test)]
pub(crate) use transfer::SftpHandle;
