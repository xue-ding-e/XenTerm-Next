use super::*;

/// A buffer with `count` lines of scrollback and nothing on screen.
fn scrolled(count: usize) -> TermBuffer {
    let history: Vec<String> = (0..count).map(|i| format!("line-{i}")).collect();
    let refs: Vec<&str> = history.iter().map(String::as_str).collect();
    make_buf(5, 20, &refs, &["live"], 0)
}

#[test]
fn a_whole_line_of_wheel_moves_the_view_by_one_line() {
    let mut buffer = scrolled(10);
    assert!(buffer.scroll_by_lines(1.0));
    assert_eq!(buffer.view_offset, 1);
    assert!(buffer.scroll_by_lines(-1.0));
    assert_eq!(buffer.view_offset, 0, "and back again");
}

#[test]
fn a_fraction_of_a_line_is_banked_rather_than_dropped() {
    // The whole reason the accumulator exists. A trackpad produces deltas far smaller
    // than a line, and if each event were rounded the view would never move at all for
    // a slow scroll — the behaviour this policy was written to fix. Three thirds of a
    // line are a line, and only the third event should move the view.
    let mut buffer = scrolled(10);
    assert!(!buffer.scroll_by_lines(1.0 / 3.0), "a third is not a line");
    assert!(!buffer.scroll_by_lines(1.0 / 3.0), "nor are two thirds");
    assert_eq!(buffer.view_offset, 0);
    assert!(buffer.scroll_by_lines(1.0 / 3.0), "and now it is");
    assert_eq!(buffer.view_offset, 1);
}

#[test]
fn resting_at_a_boundary_keeps_banking_the_fraction() {
    // The subtle half, and the one that was a real bug: clearing the banked remainder
    // whenever the offset merely *equalled* a boundary wiped it on every event, so slow
    // scrolling was dead until it was fast enough to cross a whole line at once. The
    // remainder is dropped only when a boundary actually clipped a step.
    let mut buffer = scrolled(10);
    // Push against the top: the offset cannot go below zero, so the step is clipped and
    // the remainder is dropped — correct, because the gesture's intent is exhausted.
    buffer.scroll_by_lines(-5.0);
    assert_eq!(buffer.view_offset, 0);
    assert_eq!(
        buffer.scroll_accum, 0.0,
        "a clipped step drops the remainder"
    );

    // Now bank a fraction while resting exactly at the top. The next event has to be
    // able to use it, or the earlier fractions are thrown away for nothing.
    buffer.scroll_by_lines(0.5);
    assert_eq!(buffer.view_offset, 0, "half a line is not a line");
    assert_eq!(buffer.scroll_accum, 0.5, "and it is kept");
    buffer.scroll_by_lines(1.0);
    assert_eq!(buffer.view_offset, 1, "a half plus a whole crosses one");
}

#[test]
fn a_huge_delta_cannot_teleport_the_view() {
    // A stray pixel delta — a driver quirk, a window resize mid-gesture — must not
    // jump the whole scrollback. Each event is clamped to 24 lines.
    let mut buffer = scrolled(1000);
    buffer.scroll_by_lines(100_000.0);
    assert_eq!(buffer.view_offset, 24, "one event moves at most 24 lines");
}

#[test]
fn scrolling_stops_at_the_scrollback_that_exists() {
    let mut buffer = scrolled(7);
    buffer.scroll_by_lines(100.0);
    assert_eq!(buffer.view_offset, 7, "not past the oldest retained line");
    assert!(!buffer.scroll_by_lines(1.0), "and it reports no movement");
}

#[test]
fn scrolling_at_a_boundary_reports_no_movement_so_the_caller_can_skip_a_repaint() {
    // The return value is not decoration: a caller repaints the whole grid on a `true`,
    // and a wheel event at the top of the scrollback has to be free.
    let mut buffer = scrolled(0);
    assert!(!buffer.scroll_by_lines(1.0), "nothing to scroll");
    assert!(!buffer.scroll_to_boundary(true));
    assert!(!buffer.scroll_to_boundary(false));
}

#[test]
fn a_scrollbar_jump_lands_where_it_was_asked_to() {
    // A drag names a position rather than a delta, so it must not go through the
    // accumulator: a banked fraction would fight the pointer.
    let mut buffer = scrolled(50);
    buffer.scroll_by_lines(0.5);
    assert!(buffer.scroll_to_offset(30));
    assert_eq!(buffer.view_offset, 30);
    assert_eq!(buffer.scroll_accum, 0.0, "the gesture's remainder is over");
    assert!(
        !buffer.scroll_to_offset(30),
        "and moving nowhere is reported"
    );
}

#[test]
fn a_scrollbar_jump_past_the_end_lands_at_the_end() {
    let mut buffer = scrolled(12);
    assert!(buffer.scroll_to_offset(999));
    assert_eq!(buffer.view_offset, 12);
}

#[test]
fn the_two_boundaries_are_the_live_bottom_and_the_oldest_line() {
    let mut buffer = scrolled(9);
    assert!(buffer.scroll_to_boundary(false));
    assert_eq!(buffer.view_offset, 9, "the oldest retained line");
    assert!(buffer.scroll_to_boundary(true));
    assert_eq!(
        buffer.view_offset, 0,
        "the live bottom, where offset 0 lives"
    );
}
