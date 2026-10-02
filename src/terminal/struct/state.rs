use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

use crate::terminal::charset::CharsetTracker;

/// Per-terminal state used by normal and alternate-screen rendering.
pub(crate) struct TermBuffer {
    pub(crate) parser: vt100::Parser,
    pub(crate) find_query: String,
    pub(crate) is_dark: bool,
    pub(crate) output_highlight: OutputHighlightPreset,
    pub(crate) custom_highlight_rules: Vec<CompiledOutputRule>,
    pub(crate) json_format_output: bool,
    /// Honor VT100 line-drawing (DEC Special Graphics) via SCS designators and
    /// SO/SI even when the session encoding is UTF-8 (#376, PuTTY's option).
    pub(crate) vt100_drawing: bool,
    pub(crate) charset: CharsetTracker,
    pub(crate) interactive_echo_until: std::time::Instant,
    pub(crate) sel_anchor: Option<(usize, u16)>,
    pub(crate) sel_focus: Option<(usize, u16)>,
    pub(crate) sel_ranges: Vec<((usize, u16), (usize, u16))>,
    /// Whether the remote application is tracking the mouse (vt100
    /// `mouse_protocol_mode() != None`). When true, clicks and drags are
    /// forwarded to the PTY instead of starting a local drag-selection, so
    /// btop/htop/mc can be operated with the mouse (#terminal-mouse).
    pub(crate) mouse_tracked: bool,
    pub(crate) history: VecDeque<Line>,
    pub(crate) prev: Vec<Line>,
    pub(crate) view_offset: usize,
    /// Fractional scrollback rows not yet applied. Wheel deltas arrive as
    /// pixel fractions of a row (trackpad + macOS momentum decay); keeping
    /// the remainder here lets the momentum tail glide to a stop instead of
    /// stepping a fixed amount per event.
    pub(crate) scroll_accum: f32,
    pub(crate) displayed_text: Vec<String>,
    pub(crate) csi_state: CsiState,
    pub(crate) csi_pending: Vec<u8>,
    /// Retained post-rewrite byte stream, capped at RAW_CAP, replayed through a
    /// fresh parser to reflow already-printed output on resize (#169).
    ///
    /// A `Vec` rather than a `VecDeque`: appending is the hot operation, and a
    /// deque needs `make_contiguous()` (a memmove) before the stream can be
    /// scanned. Dropping from the front only happens once the cap is crossed.
    pub(crate) raw: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum CsiState {
    Normal,
    Esc,
    Csi,
    /// Buffering an SCS designator (`ESC ( X`, `ESC ) X`, …); payload holds the
    /// designator intro byte. Bytes still pass through to the display verbatim.
    Designate(u8),
    /// Buffering an OSC string (`ESC ] … BEL/ST`) so its payload is never
    /// charset-translated. Bytes still pass through to the display verbatim.
    Osc,
    OscEsc,
    /// An OSC longer than `OSC_CAP`: dropped unparsed until its terminator.
    ///
    /// Handing the buffered prefix to the parser instead would not preserve it —
    /// vte caps its own OSC buffer at 1024 bytes — and the replay ring would then
    /// hold a truncated sequence that a resize feeds back through the parser.
    /// Nothing is retained while dropping.
    OscDiscard,
    /// The `ESC` of a possible `ST` while dropping an oversized OSC.
    OscDiscardEsc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputHighlightPreset {
    Off,
    Log,
    DevOps,
}

/// A rule after compilation: its matcher, and the colour it paints with.
///
/// Not `PartialEq`, and that is not an omission: `regex::Regex` is not comparable, and
/// comparing compiled rules is the wrong question anyway — two sets of rules are the same
/// when the *source* rules are the same, which is what `OutputHighlightRule` compares.
#[derive(Clone)]
pub(crate) struct CompiledOutputRule {
    pub(crate) matcher: regex::Regex,
    pub(crate) whole_line: bool,
    pub(crate) ansi_index: u8,
}

pub(crate) type TermBufferHandle = Arc<Mutex<TermBuffer>>;
pub(crate) type TermBuffers = Arc<Mutex<HashMap<String, TermBufferHandle>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderWaitResult {
    Settled,
    Closed,
    TimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RenderGatePhase {
    Idle,
    Scheduled,
    Flushing,
}

pub(super) struct RenderGateState {
    pub(super) requested: u64,
    pub(super) settled: u64,
    pub(super) phase: RenderGatePhase,
    pub(super) closed: bool,
    pub(super) last_visible_flush: std::time::Instant,
}

/// Coalesces and acknowledges UI snapshot flushes for one terminal tab.
pub(crate) struct TabRenderGate {
    pub(super) state: Mutex<RenderGateState>,
    pub(super) settled_cv: Condvar,
}

pub(crate) type RenderGates = Arc<Mutex<HashMap<String, Arc<TabRenderGate>>>>;

/// A handle onto one scheduled flush of a tab's render gate.
///
/// Handed back by a render request so a producer running ahead of the renderer
/// can block until the frame it asked for has actually been flushed, instead of
/// pouring more bytes into a terminal buffer nobody has painted yet. Lives here
/// rather than in the UI layer because `crate::core::EventSink::request_render`
/// returns one, and core may not name a type from the toolkit glue.
pub(crate) struct RenderTicket {
    gate: Arc<TabRenderGate>,
    generation: u64,
}

impl RenderTicket {
    pub(crate) fn new(gate: Arc<TabRenderGate>, generation: u64) -> Self {
        Self { gate, generation }
    }

    /// Block until this ticket's flush settles, `timeout` elapses, or the gate
    /// closes underneath (the tab went away).
    pub(crate) fn wait_for_flush(self, timeout: std::time::Duration) -> RenderWaitResult {
        self.gate.wait_for(self.generation, timeout)
    }
}

/// A coloured, cursor-annotated snapshot ready for a terminal grid.
///
/// Spans are [`TermSpan`], the terminal layer's own type, so a snapshot is
/// something any frontend can draw rather than a type owned by one of them.
pub(crate) struct BuiltScreen {
    pub(crate) spans: Vec<TermSpan>,
    pub(crate) cursor_row: i32,
    pub(crate) cursor_col: i32,
    pub(crate) rows_used: i32,
    pub(crate) is_alt: bool,
    pub(crate) mouse_tracked: bool,
    pub(crate) scroll_max: i32,
    pub(crate) scroll_offset: i32,
}

/// One coloured run within a terminal line.
#[derive(Clone)]
pub(crate) struct HistSpan {
    pub(crate) text: String,
    pub(crate) fg: vt100::Color,
    pub(crate) bg: vt100::Color,
    pub(crate) bold: bool,
    pub(crate) inverse: bool,
    pub(crate) col: i32,
    pub(crate) cells: i32,
}

pub(crate) type Line = (String, Vec<HistSpan>, bool);

/// A colour the terminal layer can produce without knowing what a colour is.
///
/// A toolkit colour type used to be the currency here, which is what made this
/// layer depend on the toolkit. Alpha is kept because the terminal needs it: the
/// default background is transparent so a span does not paint a fill over the
/// theme's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rgba {
    pub(crate) r: u8,
    pub(crate) g: u8,
    pub(crate) b: u8,
    pub(crate) a: u8,
}

impl Rgba {
    pub(crate) const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 0xff }
    }

    pub(crate) const fn transparent() -> Self {
        Self {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        }
    }
}

/// A decoded emoji bitmap, sized in pixels for whichever frontend draws it.
///
/// The bytes are the `Arc` because a span is cloned per visible cell and the image
/// is the same for every clone; copying a few KB of RGBA per cell would make
/// scrolling allocate in proportion to the screen rather than to the emoji.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EmojiImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Arc<Vec<u8>>,
}

/// One highlighted rectangle on the terminal grid, framework-neutral.
///
/// This is the same story as [`TermSpan`] one layer up: it used to be a type the
/// toolkit generated, so [`TermBuffer::selection_rects_visible`][crate::terminal::TermBuffer]
/// — a method on the layer this refactor exists to keep toolkit-independent —
/// returned a value whose *type* came from the toolkit being decoupled from; a
/// view outside that toolkit could not call it at all.
///
/// Positioning is in grid cells, in the visible window: `row` is a visible row
/// (0 = the top row of the grid as drawn), `col` is a column, and `len` is a width
/// in cells rather than in characters, so a run containing CJK covers the two
/// columns each of its glyphs occupies. Every frontend converts this to its own
/// rectangle type at its own boundary, in one place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TermMatch {
    pub(crate) row: i32,
    pub(crate) col: i32,
    pub(crate) len: i32,
}

impl TermMatch {
    /// A rectangle at `row`/`col` spanning `len` cells.
    ///
    /// No clamping: callers that compute a width already guarantee at least one
    /// cell, and a constructor that silently widened a zero would hide the
    /// arithmetic mistake instead of surfacing it as an invisible highlight.
    pub(crate) const fn new(row: i32, col: i32, len: i32) -> Self {
        Self { row, col, len }
    }
}

/// One coloured run of text on the terminal grid, framework-neutral.
///
/// This is the terminal layer's own type, and it exists because the previous one
/// was not: it was generated by the toolkit and carried the toolkit's colour and
/// image types, so `crate::terminal` — the layer this whole refactor exists to
/// keep toolkit-independent — named a type from the toolkit it is being decoupled
/// from. A second frontend could not consume that pipeline at all.
///
/// The view converts these at its own boundary, in `crate::ui`, building the
/// toolkit's image from the same bytes. The conversion is one-way and one-place.
/// Which module may name a toolkit is now asserted rather than remembered, in
/// `crate::arch_guards`.
///
/// `emoji_image` is an `Option` rather than a default-constructed image because "no
/// image" is a fact worth spelling: the old default image was a sentinel that every
/// consumer had to remember to test, and nothing stopped a real zero-sized image
/// from being confused with "this span is only text".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TermSpan {
    pub(crate) text: String,
    pub(crate) fg: Rgba,
    pub(crate) bg: Rgba,
    pub(crate) bold: bool,
    pub(crate) row: i32,
    pub(crate) col: i32,
    /// Width in terminal cells, which is not the character count: CJK is two and an
    /// emoji is two, and the grid is laid out in cells.
    pub(crate) cells: i32,
    /// Whether `text` contains a CJK character, so a frontend can pick a font that
    /// has the glyphs (#54). A property of the run rather than a per-cell lookup,
    /// because that is how the runs are built.
    pub(crate) cjk: bool,
    pub(crate) emoji: bool,
    /// `Some` exactly when this span draws an image instead of glyphs.
    pub(crate) emoji_image: Option<EmojiImage>,
}
