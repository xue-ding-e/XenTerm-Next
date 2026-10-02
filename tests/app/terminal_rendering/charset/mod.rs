use super::*;

#[test]
fn esc_paren_zero_draws_box_lines_even_in_utf8_mode() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b(0lqqk\x1b(B");
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "┌──┐");
    // The reflow replay ring stores the already-translated UTF-8, so a resize
    // re-renders the same glyphs without charset state.
    assert_eq!(
        buffer.raw.iter().copied().collect::<Vec<_>>(),
        "\x1b(0┌──┐\x1b(B".as_bytes()
    );
}

#[test]
fn so_si_shifts_output_to_g1_and_back() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b)0\x0eq\x0f q");
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "─ q");
}

#[test]
fn charset_designator_survives_split_output_chunks() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b");
    let _ = buffer.ingest(b"(0");
    let _ = buffer.ingest(b"lqk");
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "┌─┐");
}

#[test]
fn esc_paren_b_restores_plain_ascii() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b(0q\x1b(Bq");
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "─q");
}

#[test]
fn disabled_vt100_drawing_passes_letters_through() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    buffer.vt100_drawing = false;
    let _ = buffer.ingest(b"\x1b(0lqqk\x1b(B");
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "lqqk");
}

#[test]
fn osc_payload_is_never_charset_translated() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    // Window-title OSC emitted while DEC Special Graphics is active: its
    // payload must reach the parser verbatim; only the trailing q maps.
    let _ = buffer.ingest(b"\x1b(0\x1b]2;top - user\x07q");
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "─");
}

#[test]
fn resize_reflow_replay_keeps_translated_box_lines() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b(0lqqk\x1b(B\r\nsecond line");
    buffer.reflow(4, 20);
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "┌──┐");
    assert_eq!(buffer.displayed_text[1], "second line");
}
