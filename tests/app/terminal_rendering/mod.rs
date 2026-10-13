use super::*;

fn hist_line(s: &str) -> Line {
    (s.to_string(), Vec::new(), false)
}

fn wrapped_hist_line(s: &str) -> Line {
    (s.to_string(), Vec::new(), true)
}

fn make_buf(
    rows: u16,
    cols: u16,
    history: &[&str],
    live_lines: &[&str],
    view_offset: usize,
) -> TermBuffer {
    let mut buffer = TermBuffer::new(rows, cols);
    buffer.parser.process(live_lines.join("\r\n").as_bytes());
    let saved = history.iter().map(|s| hist_line(s)).collect::<Vec<_>>();
    buffer.parser.seed_history(&saved);
    buffer.view_offset = view_offset;
    buffer.output_highlight = OutputHighlightPreset::Log;
    buffer.vt100_drawing = true;
    buffer
}


#[test]
fn releasing_scrollback_drops_retained_history_and_replay_bytes() {
    let mut buffer = make_buf(5, 20, &["old-1", "old-2"], &["live"], 2);
    buffer.parser.process(b"\r\nnew output");
    buffer.displayed_text.push("visible".to_string());
    buffer.sel_anchor = Some((0, 0));
    buffer.sel_focus = Some((1, 1));
    buffer.sel_ranges.push(((0, 0), (1, 1)));

    buffer.release_scrollback();

    assert_eq!(buffer.history_len(), 0);
    assert!(!buffer.parser.screen().contents().contains("old-1"));
    assert!(!buffer.parser.screen().contents().contains("new output"));
    assert!(buffer.displayed_text.is_empty());
    assert!(buffer.sel_anchor.is_none());
    assert!(buffer.sel_focus.is_none());
    assert!(buffer.sel_ranges.is_empty());
    assert_eq!(buffer.view_offset, 0);
}

mod charset;
mod colors;
mod protocol;
mod scroll;
mod selection;
