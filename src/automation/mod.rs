//! Shared non-interactive automation capabilities.
//!
//! CLI and MCP are adapters over this module. Protocol framing, argument
//! presentation, and human-readable output stay in their respective adapters.

#[path = "impls/sftp.rs"]
mod sftp;
#[path = "struct/mod.rs"]
mod structs;
#[path = "impls/tools.rs"]
mod tools;

pub(crate) use structs::Frontend;
pub(crate) use structs::execution_log;

pub(crate) use tools::{access_policy, call};
