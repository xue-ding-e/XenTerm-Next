#[path = "struct/state.rs"]
mod state;

#[path = "impls/charset.rs"]
mod charset;
#[path = "impls/encoding.rs"]
mod encoding;
#[path = "impls/input.rs"]
mod input;
#[path = "impls/json_output.rs"]
mod json_output;
#[path = "impls/local.rs"]
pub(crate) mod local;
#[path = "impls/output_highlight.rs"]
mod output_highlight;
#[path = "impls/presentation.rs"]
mod presentation;
#[path = "impls/render.rs"]
mod render;
#[path = "impls/render_gate.rs"]
mod render_gate;
#[path = "impls/serial.rs"]
pub(crate) mod serial;
#[path = "impls/telnet.rs"]
pub(crate) mod telnet;
#[path = "impls/term_buffer.rs"]
mod term_buffer;
#[path = "impls/zmodem.rs"]
pub(crate) mod zmodem;

pub(crate) use charset::CharsetTracker;
#[cfg(test)]
pub(crate) use term_buffer::find_matches_in_rows;
#[cfg(test)]
pub(crate) use term_buffer::OSC_CAP;
#[cfg(test)]
use std::collections::VecDeque;
#[cfg(test)]
pub(crate) use input::terminal_uses_bracketed_paste;
pub(crate) use encoding::TerminalEncoding;
#[cfg(test)]
pub(crate) use input::normalize_pasted_newlines;
pub(crate) use input::{
    encode_command_bar_input, encode_mouse_event,
    encode_pasted_text, key_to_pty_bytes, paste_requires_large_review,
};
// The review decision belongs to the view, which holds the setting and builds the
// dialog; the terminal layer only encodes what it is handed.
pub(crate) use input::paste_needs_review;
// The IME event vocabulary, named only by the view that has an IME to talk to.
pub(crate) use input::CompositionEvent;
pub(crate) use json_output::format_json_output;
pub(crate) use output_highlight::compile_output_rules;
pub(crate) use presentation::{highlight_plain_output, render_term_span};
// The terminal view paints its pane with this; the grid paints its own text over it.
pub(crate) use presentation::terminal_background;
#[cfg(test)]
pub(crate) use presentation::{log_level_marker, text_cell_width, vt_span_colors};

#[cfg(test)]
use crate::app::term_buf;
#[cfg(test)]
use crate::config::OutputHighlightRule;
#[cfg(test)]
use std::sync::{Arc, Mutex};
#[cfg(test)]
#[path = "../../tests/app/output_highlighting/mod.rs"]
mod log_highlight_tests;
#[cfg(test)]
#[path = "../../tests/app/terminal_rendering/mod.rs"]
mod selection_tests;
#[cfg(test)]
#[path = "../../tests/app/terminal_bench/mod.rs"]
mod terminal_bench;
pub(crate) use render::{
    build_row, cell_prefix, char_after_cell_end, char_at_cell_start, detect_scroll, MAX_HISTORY,
    RAW_CAP,
};
pub(crate) use state::{
    BuiltScreen, CompiledOutputRule, CsiState, EmojiImage, HistSpan, Line, OutputHighlightPreset,
    RenderGates, RenderTicket, Rgba, TabRenderGate, TermBuffer, TermBufferHandle, TermBuffers,
    TermMatch, TermSpan,
};
