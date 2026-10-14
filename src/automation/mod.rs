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

/// Safe remote diagnostic shared by MCP gates and subsequent tool loads.
pub(crate) const MCP_CONFIG_UNAVAILABLE_MESSAGE: &str =
    "MCP configuration is unavailable; request denied. Check the selected profile and retry.";
