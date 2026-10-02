use super::*;

#[test]
fn plain_click_has_no_selection_extent() {
    let mut buffer = make_buf(2, 20, &[], &["one"], 0);
    buffer.sel_anchor = Some((0, 1));
    buffer.sel_focus = Some((0, 1));
    buffer.sel_ranges.push(((0, 1), (0, 1)));
    assert!(!buffer.selection_has_extent());

    buffer.sel_focus = Some((0, 2));
    buffer.sel_ranges[0].1 = (0, 2);
    assert!(buffer.selection_has_extent());
}

#[test]
fn double_click_selects_shell_word_and_keeps_paths_together() {
    let mut buffer = make_buf(2, 80, &[], &["ssh user@host /var/log/app.log"], 0);
    buffer.render();
    let selected = buffer.select_word_at(0, 20).expect("word under cursor");
    assert_eq!(selected, "/var/log/app.log");
    assert_eq!(buffer.extract_selection_text(), "/var/log/app.log");
}

#[test]
fn vis_to_abs_maps_live_and_scrolled_consistently() {
    // history H0..H2 (3 lines), live LIVE0/LIVE1 → combined len 5.
    let live = make_buf(5, 20, &["H0", "H1", "H2"], &["LIVE0", "LIVE1"], 0);
    assert_eq!(live.vis_to_abs(0), 3, "live row 0 is first live line");
    assert_eq!(live.vis_to_abs(1), 4);

    // Scrolled to the very top (offset = history len).
    let top = make_buf(5, 20, &["H0", "H1", "H2"], &["LIVE0", "LIVE1"], 3);
    assert_eq!(top.vis_to_abs(0), 0, "top row 0 is oldest history line");
    assert_eq!(top.vis_to_abs(2), 2);
    assert_eq!(top.vis_to_abs(3), 3, "row 3 crosses into live content");
}

#[test]
fn extract_spans_history_and_live() {
    let mut buf = make_buf(5, 20, &["HIST0", "HIST1", "HIST2"], &["LIVE0", "LIVE1"], 3);
    buf.sel_anchor = Some((0, 0)); // top of history
    buf.sel_focus = Some((4, 19)); // end of last live line
    assert_eq!(
        buf.extract_selection_text(),
        "HIST0\nHIST1\nHIST2\nLIVE0\nLIVE1"
    );
}

#[test]
fn extract_is_view_independent() {
    // The same absolute selection copies identically whether the view is
    // scrolled to the top or sitting at the live bottom — this is the whole
    // point of the fix (a top-to-bottom selection survives auto-scrolling).
    let sel = |off| {
        let mut b = make_buf(
            5,
            20,
            &["HIST0", "HIST1", "HIST2"],
            &["LIVE0", "LIVE1"],
            off,
        );
        b.sel_anchor = Some((0, 0));
        b.sel_focus = Some((4, 19));
        b.extract_selection_text()
    };
    assert_eq!(sel(3), sel(0));
    assert_eq!(sel(3), "HIST0\nHIST1\nHIST2\nLIVE0\nLIVE1");
}

#[test]
fn extract_joins_soft_wrapped_rows() {
    let mut buf = make_buf(5, 10, &[], &["x"], 0);
    buf.history = VecDeque::from([
        wrapped_hist_line("0123456789"),
        wrapped_hist_line("abcdefghij"),
        hist_line("klmnop"),
        hist_line("next"),
    ]);
    buf.sel_anchor = Some((0, 0));
    buf.sel_focus = Some((3, 9));
    assert_eq!(
        buf.extract_selection_text(),
        "0123456789abcdefghijklmnop\nnext"
    );
}

#[test]
fn highlight_clipped_to_current_view() {
    // Scrolled to the top: a history selection is on-screen and highlighted.
    let mut top = make_buf(5, 20, &["HIST0", "HIST1", "HIST2"], &["LIVE0", "LIVE1"], 3);
    top.sel_anchor = Some((0, 2));
    top.sel_focus = Some((2, 4));
    let rects = top.selection_rects_visible(20);
    assert_eq!(
        rects.len(),
        3,
        "rows 0,1,2 (the 3 history lines) highlighted"
    );
    assert_eq!(rects[0].row, 0);
    assert_eq!(rects[2].row, 2);

    // At the live bottom the same history selection is scrolled off → none.
    let mut live = make_buf(5, 20, &["HIST0", "HIST1", "HIST2"], &["LIVE0", "LIVE1"], 0);
    live.sel_anchor = Some((0, 2));
    live.sel_focus = Some((2, 4));
    assert!(live.selection_rects_visible(20).is_empty());
}

#[test]
fn extract_handles_wide_cjk_columns() {
    // Regression for #132: copying after CJK glyphs drifted right by the
    // number of wide chars before the selection (e.g. selecting "1pctl"
    // yielded "ctl…"). The history line lays out on the grid as:
    //   提(0-1) 示(2-3) :(4) space(5) 1(6) p(7) c(8) t(9) l(10)
    let mut buf = make_buf(5, 20, &["提示: 1pctl"], &["x"], 0);

    // The "1pctl" run sits at grid cols 6..=10.
    buf.sel_anchor = Some((0, 6));
    buf.sel_focus = Some((0, 10));
    assert_eq!(buf.extract_selection_text(), "1pctl");

    // Selecting from the second CJK glyph through the end.
    buf.sel_anchor = Some((0, 2));
    buf.sel_focus = Some((0, 10));
    assert_eq!(buf.extract_selection_text(), "示: 1pctl");

    // Anchoring on the *second* cell of a wide glyph still grabs the whole
    // glyph — you can't half-select a CJK char.
    buf.sel_anchor = Some((0, 3));
    buf.sel_focus = Some((0, 10));
    assert_eq!(buf.extract_selection_text(), "示: 1pctl");
}


// --- The selection policy, moved out of `src/app.rs` so both frontends share it.

#[test]
fn a_plain_drag_selects_and_a_plain_click_does_not() {
    let mut buf = make_buf(2, 20, &[], &["hello world"], 0);
    buf.render();

    // A drag: press, move, release.
    buf.begin_selection(0, 0, false, false);
    buf.extend_selection(0, 4);
    assert_eq!(buf.finish_selection().as_deref(), Some("hello"));

    // A click: press and release with no movement. The endpoints are inclusive,
    // so without the extent check this would copy the character under the
    // pointer (#319).
    buf.begin_selection(0, 2, false, false);
    assert_eq!(buf.finish_selection(), None, "a click copies nothing");
    assert!(buf.sel_anchor.is_none(), "and leaves nothing selected");
}

#[test]
fn ctrl_click_adds_a_range_and_plain_click_replaces_it() {
    let mut buf = make_buf(2, 20, &[], &["alpha beta"], 0);
    buf.render();

    buf.begin_selection(0, 0, false, false);
    buf.extend_selection(0, 4);
    assert_eq!(buf.sel_ranges.len(), 1);

    // Ctrl starts a second, independent range rather than replacing the first,
    // so scattered output can be copied in one go.
    buf.begin_selection(0, 6, true, false);
    buf.extend_selection(0, 9);
    assert_eq!(buf.sel_ranges.len(), 2);
    assert_eq!(buf.finish_selection().as_deref(), Some("alpha\nbeta"));

    // A plain click starts over.
    buf.begin_selection(0, 0, false, false);
    assert_eq!(buf.sel_ranges.len(), 1);
}

#[test]
fn shift_click_extends_the_existing_range_instead_of_starting_one() {
    let mut buf = make_buf(2, 20, &[], &["hello world"], 0);
    buf.render();

    buf.begin_selection(0, 0, false, false);
    // Shift extends from the *original anchor*, not from wherever the focus is.
    buf.begin_selection(0, 6, false, true);
    assert_eq!(buf.sel_anchor, Some((0, 0)));
    assert_eq!(buf.sel_focus, Some((0, 6)));
    assert_eq!(buf.sel_ranges.len(), 1, "still one range");
}

#[test]
fn a_move_without_a_press_does_not_start_a_selection() {
    // A stray move event — the pointer crossing the grid with no button held —
    // must not fabricate a selection out of nothing.
    let mut buf = make_buf(2, 20, &[], &["hello world"], 0);
    buf.extend_selection(0, 5);
    assert!(buf.sel_anchor.is_none());
    assert!(buf.sel_ranges.is_empty());
}

#[test]
fn dragging_past_the_top_scrolls_back_and_keeps_the_anchor_pinned() {
    // The anchor is in absolute coordinates precisely so it stays on its text
    // while the view moves under it — this is what lets a drag extend past one
    // screen.
    let mut buf = make_buf(5, 20, &["H0", "H1", "H2", "H3", "H4"], &["LIVE"], 0);
    buf.render();
    // Anchor on the prompt row, then drag upward past the top.
    buf.begin_selection(0, 0, false, false);
    let anchor = buf.sel_anchor.expect("anchored");

    assert!(buf.autoscroll_selection(-1), "the view moves");
    assert_eq!(
        buf.view_offset, 2,
        "one step of two lines toward older output"
    );
    assert_eq!(buf.sel_anchor, Some(anchor), "and the anchor did not move");
    assert_ne!(
        buf.sel_focus,
        Some(anchor),
        "but the focus followed the edge"
    );
}

#[test]
fn auto_scrolling_stops_at_both_ends_and_on_the_alternate_screen() {
    // No scrollback to give: the view is already at the live bottom.
    let mut buf = make_buf(5, 20, &["H0"], &["LIVE"], 0);
    buf.render();
    buf.begin_selection(0, 0, false, false);
    assert!(!buf.autoscroll_selection(1), "nothing below the live tail");

    // At the oldest line there is nothing further back.
    let mut top = make_buf(5, 20, &["H0", "H1"], &["LIVE"], 2);
    top.render();
    top.begin_selection(0, 0, false, false);
    assert!(!top.autoscroll_selection(-1), "already at the oldest line");

    // A full-screen program owns its own view; scrolling it would fight the
    // program for the screen.
    let mut alt = make_buf(5, 20, &["H0", "H1"], &["LIVE"], 2);
    alt.render();
    alt.begin_selection(0, 0, false, false);
    assert!(!alt.autoscroll_selection(-1));
}

#[test]
fn triple_click_selects_the_whole_row_without_trailing_blanks() {
    // Trailing blanks are excluded, so copying a line does not drag a screenful
    // of spaces into the clipboard.
    let mut buf = make_buf(2, 20, &[], &["hello   "], 0);
    buf.render();
    let text = buf.select_line_at(0).expect("a non-empty row");
    assert_eq!(text, "hello");
    assert_eq!(buf.extract_selection_text(), "hello");

    // A blank row has nothing to select, and must not produce an empty-string
    // selection that a later copy would treat as real.
    let mut blank = make_buf(2, 20, &[], &["x"], 0);
    blank.render();
    assert_eq!(blank.select_line_at(1), None);
}

// --- The find search, moved out of `crate::app` so both frontends share it.

#[test]
fn a_search_reports_grid_columns_and_skips_overlaps() {
    let rows = vec!["aaa".to_string()];
    let hits = crate::terminal::find_matches_in_rows(&rows, "aa");
    assert_eq!(
        hits.len(),
        1,
        "overlapping hits of one query read as one hit, not two rectangles on the same text"
    );
    assert_eq!(hits[0].col, 0);
    assert_eq!(hits[0].len, 2);
}

#[test]
fn a_search_is_case_insensitive_and_reports_every_row() {
    let rows = vec!["Hello".to_string(), "HELLO".to_string(), "nope".to_string()];
    let hits = crate::terminal::find_matches_in_rows(&rows, "hello");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].row, 0);
    assert_eq!(hits[1].row, 1);
    assert_eq!(hits[0].len, 5);
}

#[test]
fn a_search_measures_in_cells_past_a_wide_glyph() {
    // "提示" is two grid columns per glyph, so the match after it starts at column 4
    // rather than at character index 2. The same rule the selection follows (#132),
    // which is why both come from `cell_prefix`.
    let rows = vec!["提示ab".to_string()];
    let hits = crate::terminal::find_matches_in_rows(&rows, "ab");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].col, 4, "grid column 4, not char index 2");
    assert_eq!(hits[0].len, 2);
}

#[test]
fn an_empty_query_matches_nothing() {
    let rows = vec!["anything".to_string()];
    assert!(crate::terminal::find_matches_in_rows(&rows, "").is_empty());
}

#[test]
fn setting_a_query_pushes_it_to_the_buffer_that_searches() {
    // The view keeps what the box shows and the buffer keeps what is searched; this is
    // the one place they meet, so a mismatch cannot survive it.
    let mut buf = make_buf(2, 20, &[], &["find me"], 0);
    buf.render();
    assert!(!buf.has_find_query());

    buf.set_find_query("find");
    assert!(buf.has_find_query());
    let hits = buf.find_matches();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].len, 4);

    buf.set_find_query("");
    assert!(!buf.has_find_query());
    assert!(buf.find_matches().is_empty());
}
