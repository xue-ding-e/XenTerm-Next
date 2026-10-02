//! Pump-side terminal ingest and output pacing.
//!
//! None of it names a toolkit type: bytes go into the terminal buffer on the pump
//! thread, and the renderer is asked — never waited on synchronously more than the
//! ack timeout — to draw what landed.

use std::time::Duration;

use crate::session::protocol::SessionEvent;
use crate::terminal::{TermBuffer, TermBufferHandle, TermBuffers};
use crate::terminal::RenderTicket;

/// Above this, two adjacent `Output` events are left unmerged so one batch
/// cannot monopolize the UI thread (#209).
pub(crate) const OUTPUT_MERGE_BYTE_CAP: usize = 64 * 1024;
/// Output bytes delivered per render request. A checkpoint every budget worth
/// of bytes keeps a firehose from starving the renderer.
pub(crate) const INGEST_FRAME_BUDGET: usize = 64 * 1024;
pub(crate) const UI_FLUSH_ACK_TIMEOUT: Duration = Duration::from_millis(50);
pub(crate) const PACED_LOCAL_BACKLOG_LIMIT: usize = 1024 * 1024;
pub(crate) const PACED_QUEUE_EVENT_LIMIT: usize = 256;

pub(crate) fn term_buf(bufs: &TermBuffers, tab_id: &str) -> Option<TermBufferHandle> {
    bufs.lock().unwrap().get(tab_id).cloned()
}


pub(crate) fn ingest_terminal_output(bufs: &TermBuffers, tab_id: &str, chunk: &[u8]) -> Vec<u8> {
    if let Some(h) = term_buf(bufs, tab_id) {
        h.lock().unwrap().ingest(chunk)
    } else {
        Vec::new()
    }
}

/// Checkpoint tracker for the frame budget: answers whether this chunk should
/// carry a render request with it.
pub(crate) fn record_ingested_chunk(chunk_len: usize, ingested_since_checkpoint: &mut usize) -> bool {
    debug_assert!(*ingested_since_checkpoint < INGEST_FRAME_BUDGET);
    if chunk_len == 0 {
        return false;
    }

    let remaining = INGEST_FRAME_BUDGET - *ingested_since_checkpoint;
    if chunk_len < remaining {
        *ingested_since_checkpoint += chunk_len;
        false
    } else {
        *ingested_since_checkpoint = (chunk_len - remaining) % INGEST_FRAME_BUDGET;
        true
    }
}

pub(crate) fn event_requires_immediate_ui(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::Connected
            | SessionEvent::Closed(_)
            | SessionEvent::HostKeyPrompt { .. }
            | SessionEvent::CredentialPrompt { .. }
            | SessionEvent::MfaPrompt { .. }
    )
}

pub(crate) fn wait_for_ui_flush(ticket: Option<RenderTicket>) {
    if let Some(ticket) = ticket {
        let _ = ticket.wait_for_flush(UI_FLUSH_ACK_TIMEOUT);
    }
}

#[cfg(test)]
#[path = "../../tests/app/terminal_ingest/mod.rs"]
mod ingest_frame_tests;
