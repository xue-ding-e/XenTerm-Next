use crate::session::protocol::SessionEvent;
use crate::terminal::RenderTicket;

/// Where one tab's session events go.
///
/// The session pump threads speak only to this. They produce events off the UI
/// thread and have always needed two things from whoever consumes them: apply
/// this batch, and repaint this tab. Both used to be spelled out at each of the
/// three pump delivery sites as calls into the window's own toolkit — marshalling
/// the batch onto the UI thread plus an upgrade of a weak window handle plus an
/// eleven-argument call — and that is what welded the pumps to one frontend.
///
/// Three destinations are planned and the trait is shaped for all of them:
///
/// - the UI window, the only implementation today;
/// - a second frontend, were one ever added — a different executor to marshal
///   onto, the same two operations;
/// - an out-of-process plugin host (`crate::plugin`), which serializes the batch
///   to a pipe instead of applying it, and answers [`EventSink::request_render`]
///   with `None` because it has no screen to pace a producer against.
///
/// Plugin fan-out composes rather than extends this: a multicast sink holding
/// the window sink plus the registered plugin hosts implements the same trait,
/// so the pumps never learn how many listeners exist.
///
/// # Contract
///
/// Implementations are called from pump threads, so they must be `Send + Sync`
/// and must not block on the caller's behalf beyond [`EventSink::request_render`]'s
/// ticket. Delivery is fire-and-forget: a destination that has gone away drops
/// the batch rather than reporting it, because a pump mid-firehose has nothing
/// useful to do with the answer.
pub trait EventSink: Send + Sync {
    /// Apply a batch of events belonging to `tab_id`.
    ///
    /// Takes the batch by value so an implementation that has to move it across
    /// a thread or a process boundary can do so without copying it.
    fn deliver(&self, tab_id: &str, events: Vec<SessionEvent>);

    /// Schedule a repaint of `tab_id`, returning a ticket the caller can block
    /// on to pace itself against the renderer.
    ///
    /// `None` means there is nothing to wait for — either the render was
    /// coalesced into one already in flight, or this destination does not
    /// render at all.
    fn request_render(&self, tab_id: &str) -> Option<RenderTicket>;
}
