//! Application-level infrastructure shared by the UI shell and the session
//! pumps.
//!
//! What lives here is what the UI shell and the non-UI layers still reach: the session
//! models, the pump-side runtime and ingest pacing, WebDAV sync, and the Windows jump
//! list. The pump delivery routes moved to `crate::session::protocol` with the rest of
//! the session vocabulary.
//!
//! `session_runtime`, `session_models` and `webdav` were written against
//! `use super::*` on the old module root, so the names they expect are still
//! in scope here.

pub(crate) mod ingest;
#[cfg(windows)]
pub(crate) mod jump_list;
pub(crate) mod session_models;
pub(crate) mod session_runtime;
pub(crate) mod webdav;

pub(crate) use ingest::{
    event_requires_immediate_ui, ingest_terminal_output, record_ingested_chunk, term_buf,
    wait_for_ui_flush, OUTPUT_MERGE_BYTE_CAP, PACED_LOCAL_BACKLOG_LIMIT,
    PACED_QUEUE_EVENT_LIMIT,
};

// The shared vocabulary the `use super::*` children expect. Mirrors what the
// old `app.rs` root imported for them; prune if a module stops needing one.
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

#[allow(unused_imports)]
use anyhow::{Context, Result};
#[allow(unused_imports)]
use crate::config::{
    is_reserved_session_group, named_display_groups, AuthMethod, ConfigStore,
    OutputHighlightRule, Secret, Session, SessionKind,
};
#[allow(unused_imports)]
use crate::core::TransferStore;
#[allow(unused_imports)]
use crate::i18n::t;
#[allow(unused_imports)]
use crate::session::ConnectCtx;
#[allow(unused_imports)]
use crate::sftp::{spawn_sftp, SftpHandles, SftpLastCwd};
#[allow(unused_imports)]
use crate::session::protocol::{SessionCommand, SessionEvent, SessionHandle, TabRoute, TabRoutes};
#[allow(unused_imports)]
use crate::ssh::spawn_session;
#[allow(unused_imports)]
use crate::terminal::{TermBuffer, TermBufferHandle, TermBuffers};
