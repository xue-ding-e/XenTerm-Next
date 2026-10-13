//! Focused native-grid probes. End-to-end ingest/render/resize and long-history
//! memory comparisons live in tests/terminal_core so they also run headlessly.
//! These probes are ignored by normal cargo test runs.
use super::*;
use std::hint::black_box;

#[test]
#[ignore = "benchmark: native history search"]
fn bench_terminal_find_over_full_scrollback() {
    let mut buffer = TermBuffer::new(50, 200);
    let mut data = Vec::new();
    for index in 0..100_000 {
        data.extend_from_slice(format!("INFO worker {index:06} finished job\r\n").as_bytes());
    }
    buffer.ingest(&data);
    let start = std::time::Instant::now();
    for _ in 0..5 {
        black_box(buffer.scroll_to_first_find_match("zzzz-not-present"));
    }
    println!(
        "find over {} native history rows: {:.1} ms/op",
        buffer.history_len(),
        start.elapsed().as_secs_f64() * 200.0
    );
}

#[test]
#[ignore = "benchmark: native visible frame"]
fn bench_terminal_render_frame() {
    let mut buffer = TermBuffer::new(50, 200);
    for row in 0..50 {
        buffer.ingest(format!("\x1b[3{}mstyled terminal row {row}\x1b[0m\r\n", row % 8).as_bytes());
    }
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        black_box(buffer.render());
    }
    println!(
        "render 200x50 native frame: {:.1} us/frame",
        start.elapsed().as_secs_f64() * 1000.0
    );
}
