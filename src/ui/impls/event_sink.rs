//! The destination session pumps deliver to, and the render-gate protocol.
//!
//! A pump thread calls [`EventSink::deliver`] and [`EventSink::request_render`] and
//! never learns which implementation is on the other end.
//!
//! What the sink shares with the pumps is more than a trait. The render gate
//! (`crate::terminal::TabRenderGate`) is framework-agnostic and this uses the same
//! one with the same protocol: a pump that has filled the parser asks for a render,
//! gets a ticket, and blocks on it until the frame it asked for exists. That pacing
//! is what stops a firehose from becoming one UI task per chunk, and a second
//! implementation would be a second place for it to be wrong. Only the marshalling
//! differs: the pump leaves its events on a channel that a task on the view drains.
//!
//! Why a channel rather than `WeakEntity::update`: `deliver` runs on a plain
//! `std::thread` that owns no `App` and cannot obtain one, and GPUI's
//! `ForegroundExecutor` is deliberately not `Send`. A channel is what the local
//! session's pump in [`super::view`] already uses for the same reason, and putting
//! both kinds of work through one queue also preserves their order — a render
//! request must not overtake the events it is meant to show.

use std::sync::Arc;

use crate::core::EventSink;
use crate::session::protocol::SessionEvent;
use crate::terminal::{RenderGates, RenderTicket};

/// Work handed from a pump thread to the window's own thread.
pub(crate) enum UiMessage {
    /// Events to apply to one tab.
    Events {
        tab_id: String,
        events: Vec<SessionEvent>,
    },
    /// A tab's screen needs repainting, and its gate is waiting to be settled.
    ///
    /// Carries the gate rather than looking it up again on the far side, because
    /// the gate is what the producer is blocked on: a flush that re-resolved it
    /// could settle a different one and leave this sender waiting for a frame that
    /// has already been drawn. The gate is also the only thing the flush needs —
    /// which tab it belongs to is already settled by which map it came out of.
    Render {
        gate: Arc<crate::terminal::TabRenderGate>,
    },
}

/// A session's delivery route into the window's view.
///
/// Holds the gates as well as the queue because [`EventSink::request_render`] has to
/// answer synchronously with a ticket, and the ticket has to come from the same gate
/// the view will settle.
#[derive(Clone)]
pub(crate) struct GpuiEventSink {
    /// Where work goes. Unbounded, so `deliver` never blocks a pump; a pump's own
    /// pacing comes from the render ticket, not from this queue.
    ui: tokio::sync::mpsc::UnboundedSender<UiMessage>,
    /// The per-tab gates, shared with the view that settles them.
    gates: RenderGates,
}

impl GpuiEventSink {
    pub(crate) fn new(
        ui: tokio::sync::mpsc::UnboundedSender<UiMessage>,
        gates: RenderGates,
    ) -> Self {
        Self { ui, gates }
    }
}

impl EventSink for GpuiEventSink {
    fn deliver(&self, tab_id: &str, events: Vec<SessionEvent>) {
        if events.is_empty() {
            return;
        }
        // A closed channel means the window is gone, which is the normal end of a
        // sink's life rather than a failure: the pumps are stopped by the same
        // window closing. `request_render` is where that becomes visible to a
        // waiting producer, because that is where a ticket exists to wake.
        let _ = self.ui.send(UiMessage::Events {
            tab_id: tab_id.to_string(),
            events,
        });
    }

    fn request_render(&self, tab_id: &str) -> Option<RenderTicket> {
        let gate = self.gates.lock().ok()?.get(tab_id).cloned()?;
        let (generation, should_schedule) = gate.request()?;
        let ticket = RenderTicket::new(gate.clone(), generation);

        // `should_schedule` false means a flush is already in flight and this
        // request has been coalesced into it — the ticket still has to come back,
        // because the caller's next move is to wait for that frame.
        if should_schedule
            && self
                .ui
                .send(UiMessage::Render { gate: gate.clone() })
                .is_err()
        {
            // Nobody will ever flush. Close the gate so this ticket and every later
            // one resolve as `Closed`, instead of blocking a pump thread forever on
            // a frame that cannot come.
            gate.close();
        }
        Some(ticket)
    }
}
