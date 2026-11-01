//! The xenterm service core, as a library.
//!
//! The binary in `main.rs` is a thin entry (CLI / MCP / help); everything the
//! product does lives here so the Flutter frontend's bridge crate
//! (`apps/xenterm/native`, docs/flutter-bridge.md) can link the core
//! in-process. The layering rules in `arch_guards` hold for any consumer: no
//! module in this library names a UI toolkit, and the `dead_code` allows
//! below mark the bridge surface waiting for the frontend's callers — remove
//! them module by module as the bridge grows real ones.

#[allow(dead_code)]
pub mod app;

#[allow(dead_code)]
pub mod automation;
#[allow(dead_code)]
pub mod cli;

#[allow(dead_code)]
pub mod config;

/// Public façade for in-process frontends (the Flutter bridge).
pub mod frontend;

#[allow(dead_code)]
pub mod core;

#[allow(dead_code)]
pub mod i18n;

#[allow(dead_code)]
pub mod logging;

#[allow(dead_code)]
pub mod mcp;

#[allow(dead_code)]
pub mod resource;

#[allow(dead_code)]
pub mod session;

#[allow(dead_code)]
pub mod sftp;

#[allow(dead_code)]
pub mod ssh;

#[allow(dead_code)]
pub mod terminal;

#[allow(dead_code)]
pub mod tunnel;

#[allow(dead_code)]
pub mod version;

#[allow(dead_code)]
pub mod webdav;

#[allow(dead_code)]
pub mod wallpaper;

#[cfg(test)]
mod arch_guards;
