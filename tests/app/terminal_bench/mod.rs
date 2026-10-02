//! Benchmarks for the vt100-facing hot paths this module owns.
//!
//! They are `#[ignore]`d, so a normal `cargo test` neither runs them nor slows
//! down. Run them with:
//!
//! ```text
//! cargo +stable-x86_64-pc-windows-gnu test --release -- --ignored --nocapture bench_terminal
//! ```
//!
//! Each benchmark compares the shape this code had before the optimization with
//! the shape it has now, inside one binary — so a ratio is not a build-to-build
//! comparison. Where the old path is still reachable (the byte-at-a-time scanner
//! in `ingest`, the normal-screen `reflow`) the benchmark drives that path
//! directly; where it was deleted (a whole-buffer `windows(4).rposition`,
//! `to_lowercase` per scrollback line, `live_rows()`) the deleted snippet is
//! reproduced here verbatim and labelled as the old shape.

use std::hint::black_box;

use super::*;

/// Fastest of `iters` runs, in seconds per run.
///
/// Throughput work is quoted at its best on purpose: the slowest run of a
/// benchmark measures whatever else the machine was doing, not the code.
fn best_of(iters: u32, mut body: impl FnMut()) -> f64 {
    body();
    let mut best = f64::INFINITY;
    for _ in 0..iters {
        let start = std::time::Instant::now();
        body();
        let elapsed = start.elapsed().as_secs_f64();
        if elapsed < best {
            best = elapsed;
        }
    }
    best
}

/// The fastest of `runs` passes over a byte stream, in seconds per pass.
fn best_pass(runs: u32, bytes: &[u8], chunk: usize) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..runs {
        let mut buffer = TermBuffer::new(50, 200);
        let start = std::time::Instant::now();
        for part in bytes.chunks(chunk) {
            let _ = buffer.ingest(part);
        }
        let elapsed = start.elapsed().as_secs_f64();
        if elapsed < best {
            best = elapsed;
        }
    }
    best
}

/// Log-shaped ASCII at 100 columns per line, which is what a terminal spends its
/// life on.
fn log_output(bytes: usize) -> Vec<u8> {
    let line = b"2026-09-27 12:00:00 INFO  worker finished job 000123 in 1234ms payload=abcdefghijklmnop\r\n";
    let mut out = Vec::with_capacity(bytes + line.len());
    while out.len() < bytes {
        out.extend_from_slice(line);
    }
    out
}

fn mib_per_sec(bytes: usize, secs: f64) -> f64 {
    (bytes as f64 / (1024.0 * 1024.0)) / secs
}

#[test]
#[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_terminal"]
fn bench_terminal_ingest_plain_versus_scanner_path() {
    const CHUNK: usize = 32 * 1024;
    const TOTAL: usize = 4 * 1024 * 1024;
    let plain = log_output(TOTAL);

    // One escape per chunk puts every chunk on the byte-at-a-time scanner path —
    // which is the path all of them took before the plain-run fast path existed.
    let mut scanned = Vec::with_capacity(plain.len() + plain.len() / CHUNK * 8);
    for part in plain.chunks(CHUNK) {
        scanned.extend_from_slice(part);
        scanned.extend_from_slice(b"\x1b[0m");
    }

    let before = best_pass(3, &scanned, CHUNK);
    let after = best_pass(3, &plain, CHUNK);
    println!(
        "ingest 4 MiB in 32 KiB chunks (parser-dominated in this profile - read the isolated number below): scanner path {:.1} MiB/s, plain-run fast path {:.1} MiB/s -> {:.2}x",
        mib_per_sec(plain.len(), before),
        mib_per_sec(plain.len(), after),
        before / after
    );

    // The work the fast path actually removed, with the parser out of the way:
    // the old shape pushed every byte of the chunk into `display` through the
    // state machine, the new one only asks whether the chunk is plain.
    let probe = TermBuffer::new(50, 200);
    let chunk = &plain[..CHUNK];
    let old_scan = best_of(2_000, || {
        let mut display = Vec::with_capacity(chunk.len());
        let mut state = CsiState::Normal;
        for &byte in chunk {
            match state {
                CsiState::Normal => {
                    if byte == 0x1b {
                        state = CsiState::Esc;
                    } else {
                        display.push(byte);
                    }
                }
                _ => {
                    display.push(byte);
                    state = CsiState::Normal;
                }
            }
        }
        let _ = black_box(display);
    });
    let new_scan = best_of(2_000, || {
        let _ = black_box(probe.is_plain_run(chunk));
    });
    println!(
        "  per 32 KiB chunk, scanner only: old copy through the state machine {:.1} us, new plain-run check {:.1} us -> {:.1}x",
        old_scan * 1e6,
        new_scan * 1e6,
        old_scan / new_scan
    );
}

#[test]
#[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_terminal"]
fn bench_terminal_csi3j_scan_over_replay_buffer() {
    let mut buffer = TermBuffer::new(50, 200);
    // Filled directly rather than through `ingest`: `cap_raw` always trims back
    // below its limit (it drops to the next line boundary), so waiting for `raw`
    // to reach the limit from the ingest side would never finish.
    let filler = log_output(64 * 1024);
    while buffer.raw.len() < RAW_CAP {
        buffer.raw.extend_from_slice(&filler);
    }
    let ring = buffer.raw.len();

    // The scan alone, not the chunk's other work: this is the helper
    // `ingest_display_bytes` now calls with a three-byte carry plus the new chunk.
    // Measuring through `ingest` would have priced in `cap_raw`'s memmove of the
    // whole ring, which both the old and the new code pay.
    let tail = &buffer.raw[buffer.raw.len() - 3..];
    let chunk = b"tail";
    let after = best_of(50_000, || {
        let _ = black_box(super::term_buffer::find_last_erase_saved(tail, chunk));
    });
    // The deleted line, verbatim: the whole retained stream, rescanned per chunk.
    let before = best_of(200, || {
        let _ = black_box(
            buffer
                .raw
                .windows(4)
                .rposition(|window| window == b"\x1b[3J"),
        );
    });
    println!(
        "CSI 3 J scan over a {} KiB replay ring, per chunk: old whole-ring rescan {:.1} us, 3-byte carry + chunk {:.4} us -> {:.0}x",
        ring / 1024,
        before * 1e6,
        after * 1e6,
        before / after
    );
}

#[test]
#[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_terminal"]
fn bench_terminal_selection_coordinate_mapping() {
    let mut buffer = TermBuffer::new(50, 200);
    let _ = buffer.ingest(&log_output(50 * 199));
    let (rows, cols) = buffer.parser.screen().size();

    let after = best_of(2_000, || {
        let _ = black_box(buffer.vis_to_abs(25));
    });
    // The deleted body of `live_rows()`, verbatim: every row rebuilt, every cell
    // read, to count the ones that carry runs — per pointer move of a drag.
    let before = best_of(2_000, || {
        let screen = buffer.parser.screen();
        let live: Vec<Line> = (0..rows).map(|row| build_row(screen, row, cols)).collect();
        let _ = black_box(live.iter().rposition(|(_, runs, _)| !runs.is_empty()));
    });

    buffer.begin_selection(10, 5, false, false);
    buffer.extend_selection(30, 40);
    let rects = best_of(2_000, || {
        let _ = black_box(buffer.selection_rects_visible(cols));
    });

    println!(
        "drag mapping at {rows}x{cols}: old live_rows rebuild {:.1} us/op, O(1) vis_to_abs {:.3} us/op -> {:.0}x; selection_rects_visible {:.1} us/op",
        before * 1e6,
        after * 1e6,
        before / after,
        rects * 1e6
    );
}

#[test]
#[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_terminal"]
fn bench_terminal_find_over_full_scrollback() {
    const LINES: u32 = 100_000;
    let mut buffer = TermBuffer::new(50, 200);
    for index in 0..LINES {
        buffer.history.push_back((
            format!("2026-09-27 INFO worker {index:06} finished job 000123 in 1234ms"),
            Vec::new(),
            false,
        ));
    }

    // A query that cannot match, so both sides scan the whole scrollback.
    let query = "zzzz-not-present";
    let lowercased = query.to_lowercase();

    let shipped = best_of(5, || {
        let _ = black_box(buffer.scroll_to_first_find_match(query));
    });
    // The allocation-free fold that was tried here and rejected: it measured
    // slower than the `to_lowercase` + std substring search it replaced, so this
    // guards the shipped choice rather than claiming that allocating is free.
    let rejected = best_of(5, || {
        let needle = lowercased.as_bytes();
        let hit = buffer.history.iter().any(|line| {
            let haystack = line.0.as_bytes();
            needle.len() <= haystack.len()
                && (0..=haystack.len() - needle.len()).any(|start| {
                    haystack[start].to_ascii_lowercase() == needle[0].to_ascii_lowercase()
                        && haystack[start..start + needle.len()].eq_ignore_ascii_case(needle)
                })
        });
        let _ = black_box(hit);
    });
    println!(
        "find over {LINES} scrollback lines: shipped fold-once + std search {:.1} ms, rejected allocation-free fold {:.1} ms -> shipped is {:.1}x faster",
        shipped * 1e3,
        rejected * 1e3,
        rejected / shipped
    );
}

#[test]
#[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_terminal"]
fn bench_terminal_parser_scrollback_cost() {
    const ROWS: u16 = 50;
    const COLS: u16 = 200;
    const LINES: usize = 5000;
    let cell = std::mem::size_of::<vt100::Cell>();
    let with = best_of(20, || {
        let _ = black_box(vt100::Parser::new(ROWS, COLS, LINES));
    });
    let without = best_of(20, || {
        let _ = black_box(vt100::Parser::new(ROWS, COLS, 0));
    });
    let mib = |cols: usize| (cell * cols * LINES) as f64 / (1024.0 * 1024.0);
    println!(
        "vt100::Parser::new with {LINES} lines of scrollback {:.2} ms vs none {:.2} ms ({:.1}x); Cell = {cell} B so that ring is ~{:.0} MiB at {COLS} cols and ~{:.0} MiB at 120 cols, per tab",
        with * 1e3,
        without * 1e3,
        with / without,
        mib(usize::from(COLS)),
        mib(120)
    );
}

#[test]
#[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_terminal"]
fn bench_terminal_reflow_alt_versus_normal() {
    let stream = log_output(1024 * 1024);
    let mut normal = TermBuffer::new(50, 200);
    let _ = normal.ingest(&stream);
    let mut alt = TermBuffer::new(50, 200);
    let _ = alt.ingest(&stream);
    let _ = alt.ingest(b"\x1b[?1049h");
    assert!(alt.parser.screen().alternate_screen());

    let before = best_of(3, || normal.reflow(60, 240));
    let after = best_of(3, || alt.reflow(60, 240));
    let clone = best_of(3, || {
        let _ = black_box(normal.raw.clone());
    });
    println!(
        "resize with a {} KiB replay stream: normal screen (replay) {:.2} ms, alt screen (set_size) {:.3} ms -> {:.0}x; the 2 MB clone it no longer does {:.2} ms",
        normal.raw.len() / 1024,
        before * 1e3,
        after * 1e3,
        before / after,
        clone * 1e3
    );
}

#[test]
#[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_terminal"]
fn bench_terminal_render_frame() {
    let mut buffer = TermBuffer::new(50, 200);
    let mut coloured = Vec::new();
    for row in 0..50 {
        for cell in 0..20 {
            coloured.extend_from_slice(
                format!("\x1b[3{}mword{cell:02} ", (row + cell) % 8).as_bytes(),
            );
        }
        coloured.extend_from_slice(b"\x1b[0m\r\n");
    }
    let _ = buffer.ingest(&coloured);
    buffer.is_dark = true;

    let per_frame = best_of(20, || {
        let _ = black_box(buffer.render());
    });
    println!(
        "render() at 50x200 of coloured output: {:.2} ms/frame ({:.0} fps) — the full-screen rebuild this batch did not touch",
        per_frame * 1e3,
        1.0 / per_frame
    );
}
