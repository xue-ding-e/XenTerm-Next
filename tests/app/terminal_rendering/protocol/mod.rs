use super::*;

#[test]
fn paste_tracks_remote_bracketed_paste_state() {
    let bufs = TermBuffers::default();
    let mut buffer = make_buf(2, 20, &[], &[], 0);
    buffer.parser.process(b"\x1b[?2004h");
    bufs.lock()
        .unwrap()
        .insert("tab".into(), Arc::new(Mutex::new(buffer)));

    assert!(terminal_uses_bracketed_paste(&bufs, "tab"));
    assert!(!terminal_uses_bracketed_paste(&bufs, "missing"));

    let buffer = term_buf(&bufs, "tab").unwrap();
    buffer.lock().unwrap().parser.process(b"\x1b[?2004l");
    assert!(!terminal_uses_bracketed_paste(&bufs, "tab"));
}

#[test]
fn bash_readline_history_repaints_the_current_line() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b[?2004hP> echo second");
    // GNU readline replaces "second" with the shorter "first" using six
    // backspaces, DCH for the leftover cell, then the replacement suffix.
    let _ = buffer.ingest(b"\x08\x08\x08\x08\x08\x08\x1b[1Pfirst");
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "P> echo first");
    assert_eq!(buffer.parser.screen().cursor_position(), (0, 13));
}

#[test]
fn terminal_queries_reply_at_the_current_cursor_position() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);

    assert_eq!(buffer.ingest(b"abc\x1b[6n"), b"\x1b[1;4R");
    assert_eq!(buffer.ingest(b"\x1b[?6n"), b"\x1b[?1;4R");
    assert_eq!(
        buffer.ingest(b"\x1b[5n\x1b[c\x1b[0c"),
        b"\x1b[0n\x1b[?1;2c\x1b[?1;2c"
    );
    assert_eq!(buffer.raw.iter().copied().collect::<Vec<_>>(), b"abc");
}

#[test]
fn terminal_query_and_hvp_scanners_survive_split_output_chunks() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);

    assert!(buffer.ingest(b"\x1b[").is_empty());
    assert!(buffer.ingest(b"6").is_empty());
    assert_eq!(buffer.ingest(b"n"), b"\x1b[1;1R");

    assert!(buffer.ingest(b"\x1b[2;").is_empty());
    assert!(buffer.ingest(b"3fX").is_empty());
    assert_eq!(buffer.parser.screen().cursor_position(), (1, 3));
}

#[test]
fn csi_3j_clears_xenterm_scrollback_even_when_split() {
    let mut buffer = make_buf(3, 20, &["old one", "old two"], &["current"], 2);
    buffer.raw.extend(b"old one\nold two\n");
    buffer.prev.push(hist_line("old two"));
    buffer.sel_anchor = Some((0, 0));
    buffer.sel_focus = Some((1, 2));

    let _ = buffer.ingest(b"\x1b[3");
    assert_eq!(buffer.history.len(), 2);
    let _ = buffer.ingest(b"J");

    assert!(buffer.history.is_empty());
    assert_eq!(buffer.view_offset, 0);
    assert!(buffer.raw.is_empty());
    assert!(buffer.sel_anchor.is_none());
    assert!(buffer.sel_focus.is_none());
}

#[test]
fn incoming_output_keeps_a_scrolled_view_anchored() {
    let mut buffer = make_buf(3, 20, &[], &[], 0);
    let _ = buffer.ingest(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
    assert!(!buffer.history.is_empty());

    buffer.view_offset = 1;
    buffer.render();
    let before = buffer.displayed_text.clone();
    let old_offset = buffer.view_offset;

    let _ = buffer.ingest(b"\r\nseven");
    buffer.render();

    assert_eq!(buffer.view_offset, old_offset + 1);
    assert_eq!(buffer.displayed_text, before);
}

#[test]
fn long_unbroken_output_is_captured_before_it_wraps_off_screen() {
    let mut buffer = make_buf(3, 10, &[], &[], 0);
    let output = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

    let _ = buffer.ingest(output);
    buffer.render();

    assert!(
        !buffer.history.is_empty(),
        "wrapped rows must enter scrollback even without newline bytes"
    );
    let rendered = buffer
        .history
        .iter()
        .map(|line| line.0.as_str())
        .chain(buffer.displayed_text.iter().map(String::as_str))
        .collect::<String>();
    assert_eq!(rendered, String::from_utf8_lossy(output));
}

#[test]
fn enabling_mouse_reporting_is_visible_to_a_frontend() {
    // The view decides whether to forward a click by reading this flag off the
    // snapshot, so if the sequence does not set it the mouse is silently never
    // reported — the failure looks exactly like a click handler that does not run.
    let mut buf = make_buf(5, 20, &[], &["live"], 0);
    buf.render();
    assert!(!buf.mouse_tracked, "off until the program asks");

    let _ = buf.ingest(b"\x1b[?1000h");
    buf.render();
    assert!(buf.mouse_tracked, "SGR mouse reporting is on");

    let _ = buf.ingest(b"\x1b[?1000l");
    buf.render();
    assert!(!buf.mouse_tracked, "and off again when it says so");
}

#[test]
fn the_snapshot_carries_the_mouse_flag_to_the_view() {
    let mut buf = make_buf(5, 20, &[], &["live"], 0);
    let _ = buf.ingest(b"\x1b[?1000h");
    let built = buf.render();
    assert!(
        built.mouse_tracked,
        "BuiltScreen must carry it, or the view has no way to ask"
    );
}

#[test]
fn an_oversized_osc_is_dropped_rather_than_forwarded() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    let mut bytes = b"\x1b]2;".to_vec();
    bytes.extend(std::iter::repeat(b'x').take(OSC_CAP + 64));
    bytes.push(0x07);
    bytes.extend_from_slice(b"after");

    let _ = buffer.ingest(&bytes);
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "after");
    // Not even the replay ring keeps it: a resize would feed a truncated OSC back
    // through the parser, which is the failure this avoids.
    assert_eq!(buffer.raw.iter().copied().collect::<Vec<_>>(), b"after");
}

#[test]
fn a_short_osc_still_reaches_the_parser_untouched() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b]2;title\x07kept");
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "kept");
    assert_eq!(
        buffer.raw.iter().copied().collect::<Vec<_>>(),
        b"\x1b]2;title\x07kept"
    );
}

#[test]
fn can_terminates_an_oversized_osc_and_frees_the_output_stream() {
    // The parser treats CAN/SUB as an exit from any sequence state; the local
    // scanner must agree, or a server ending an oversized OSC with CAN leaves
    // every later byte swallowed and the terminal looks frozen.
    for abort in [0x18u8, 0x1a] {
        let mut buffer = make_buf(4, 40, &[], &[], 0);
        let mut bytes = b"\x1b]2;".to_vec();
        bytes.extend(std::iter::repeat(b'x').take(OSC_CAP + 64));
        bytes.push(abort);
        bytes.extend_from_slice(b"after");
        let _ = buffer.ingest(&bytes);
        buffer.render();
        assert_eq!(buffer.displayed_text[0], "after", "CAN=0x{abort:02x}");
    }
}

#[test]
fn can_also_terminates_a_short_osc_and_a_partial_csi() {
    // The buffered OSC state forwards the aborted sequence and returns to
    // normal text, and a CAN in the middle of a CSI must not hold back the
    // text that follows it.
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b]2;short\x18after");
    buffer.render();
    assert_eq!(buffer.displayed_text[0], "after");

    let mut buffer = make_buf(4, 40, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b[31;4\x18text");
    buffer.render();
    assert_eq!(buffer.displayed_text[0], "text");
}

#[test]
fn a_resize_on_the_alternate_screen_keeps_the_scrollback_it_had() {
    let mut buffer = make_buf(4, 20, &[], &[], 0);
    let _ = buffer.ingest(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");
    let history_before = buffer.history.len();
    assert!(history_before > 0, "the normal screen scrolled rows into history");

    let _ = buffer.ingest(b"\x1b[?1049halt-screen");
    assert!(buffer.parser.screen().alternate_screen());

    buffer.reflow(6, 30);

    assert_eq!(buffer.parser.screen().size(), (6, 30));
    assert!(
        buffer.parser.screen().alternate_screen(),
        "the resize must not drop the program off its own screen"
    );
    assert_eq!(
        buffer.history.len(),
        history_before,
        "a resize while vim is open must not throw the scrollback away"
    );

    // Leaving the alt screen must land on a normal grid that was resized too.
    let _ = buffer.ingest(b"\x1b[?1049l");
    assert!(!buffer.parser.screen().alternate_screen());
    assert_eq!(buffer.parser.screen().size(), (6, 30));
}
