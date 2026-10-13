use super::*;
use std::hint::black_box;
use std::time::Instant;

fn visible(buffer: &mut TermBuffer) -> Vec<String> {
    buffer.render();
    buffer.displayed_text.clone()
}

#[test]
fn fragmented_ansi_and_utf8_keep_text_and_cursor() {
    let mut b = TermBuffer::new(4, 40);
    for part in [
        b"ab\x1b[".as_slice(),
        b"31m",
        "中".as_bytes().get(..1).unwrap(),
        "中".as_bytes().get(1..).unwrap(),
        b"\x1b[0m",
    ] {
        assert!(b.ingest(part).is_empty());
    }
    assert_eq!(visible(&mut b)[0], "ab中");
    assert_eq!(b.parser.screen().cursor_position(), (0, 4));
}

#[test]
fn rgb_background_and_inverse_survive_render() {
    let mut b = TermBuffer::new(3, 40);
    b.is_dark = true;
    b.ingest(b"\x1b[38;2;10;20;30;48;2;40;50;60mRGB\x1b[7mINV\x1b[0m");
    let screen = b.render();
    let rgb = screen
        .spans
        .iter()
        .find(|s| s.text.contains("RGB"))
        .unwrap();
    let inv = screen
        .spans
        .iter()
        .find(|s| s.text.contains("INV"))
        .unwrap();
    assert_eq!(rgb.fg, Rgba::rgb(10, 20, 30));
    assert_eq!(rgb.bg, Rgba::rgb(40, 50, 60));
    assert_eq!(inv.fg, rgb.bg);
    assert_eq!(inv.bg, rgb.fg);
}
#[test]
fn unique_and_repeated_scrollback_rows_keep_order() {
    let mut unique = TermBuffer::new(3, 30);
    for n in 0..12 {
        unique.ingest(format!("line-{n:02}\r\n").as_bytes());
    }
    unique.render();
    let all = unique
        .history_lines()
        .map(|l| l.0.trim_end().to_string())
        .chain(
            unique
                .displayed_text
                .iter()
                .map(|s| s.trim_end().to_string()),
        )
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        all,
        (0..12).map(|n| format!("line-{n:02}")).collect::<Vec<_>>()
    );

    let mut repeated = TermBuffer::new(3, 30);
    for _ in 0..12 {
        repeated.ingest(b"repeat\r\n");
    }
    repeated.render();
    let count = repeated
        .history_lines()
        .filter(|l| l.0.trim_end() == "repeat")
        .count()
        + repeated
            .displayed_text
            .iter()
            .filter(|s| s.as_str() == "repeat")
            .count();
    assert_eq!(count, 12);
}

#[test]
fn alternate_screen_resize_restores_normal_grid_and_history() {
    let mut b = TermBuffer::new(3, 12);
    b.ingest(b"one\r\ntwo\r\nthree\r\nfour\r\n");
    let history = b.history_len();
    assert!(history > 0);
    b.ingest(b"\x1b[?1049halt");
    assert!(b.parser.screen().alternate_screen());
    b.reflow(5, 20);
    assert_eq!(b.parser.screen().size(), (5, 20));
    assert_eq!(b.history_len(), history);
    b.ingest(b"\x1b[?1049l");
    assert!(!b.parser.screen().alternate_screen());
    assert_eq!(b.parser.screen().size(), (5, 20));
}
#[test]
fn cjk_wrap_and_selection_copy_preserve_cells() {
    let mut b = TermBuffer::new(3, 6);
    b.ingest("你好世界".as_bytes());
    let rows = visible(&mut b);
    assert_eq!(rows[0], "你好世");
    assert_eq!(rows[1], "界");
    b.sel_anchor = Some((0, 0));
    b.sel_focus = Some((1, 2));
    assert_eq!(b.extract_selection_text(), "你好世界");
}

#[test]
fn modes_queries_osc_guard_and_dec_lines() {
    let mut b = TermBuffer::new(4, 40);
    assert_eq!(b.ingest(b"abc\x1b[6n"), b"\x1b[1;4R");
    assert_eq!(b.ingest(b"\x1b[?6n"), b"\x1b[?1;4R");
    assert_eq!(b.ingest(b"\x1b[5n\x1b[c"), b"\x1b[0n\x1b[?1;2c");
    b.ingest(b"\x1b[?1000h");
    assert!(b.render().mouse_tracked);
    b.ingest(b"\x1b[?1000l");
    assert!(!b.render().mouse_tracked);
    let mut osc = b"\x1b]2;".to_vec();
    osc.extend(std::iter::repeat(b'x').take(term_buffer::OSC_CAP + 64));
    osc.extend_from_slice(b"\x07kept");
    b.ingest(&osc);
    assert_eq!(visible(&mut b)[0], "abckept");
    assert!(!b.displayed_text.iter().any(|line| line.contains('x')));
    assert!(b.csi_pending.len() <= term_buffer::OSC_CAP);
    let mut lines = TermBuffer::new(3, 20);
    lines.vt100_drawing = true;
    lines.ingest(b"\x1b(0lqqk\x1b(B");
    assert_eq!(visible(&mut lines)[0], "┌──┐");
}
fn pipeline_stream(kind: &str) -> Vec<u8> {
    let pattern: &[u8] = match kind {
        "plain" => b"2026-10-13 INFO worker=7 task finished id=1234567890\r\n",
        "mixed" => b"INFO ready\r\n\x1b[33mWARN slow\x1b[0m\r\nERROR retry\r\n",
        "color" => b"\x1b[38;2;12;34;56;48;2;70;80;90mcolor-heavy\x1b[0m\r\n",
        "unicode" => "终端日志：任务完成 ✅ café résumé\r\n".as_bytes(),
        "tui" => b"\x1b[H\x1b[2J\x1b[1;1HStatus\x1b[2;3H\x1b[32mRunning\x1b[0m",
        _ => unreachable!(),
    };
    let mut data = Vec::with_capacity(256 * 1024 + pattern.len() + 32);
    if kind == "tui" {
        data.extend_from_slice(b"\x1b[?1049h");
    }
    while data.len() < 256 * 1024 {
        data.extend_from_slice(pattern);
    }
    data.extend_from_slice(b"\r\nEND-MARKER");
    data
}

fn one_pipeline_pass(data: &[u8], rows: u16, cols: u16, chunk: usize) -> (f64, usize) {
    let mut b = TermBuffer::new(rows, cols);
    let start = Instant::now();
    for part in data.chunks(chunk) {
        let reply = b.ingest(black_box(part));
        assert!(reply.is_empty());
        black_box(b.render());
    }
    let secs = start.elapsed().as_secs_f64();
    assert!(b
        .displayed_text
        .iter()
        .any(|line| line.contains("END-MARKER")));
    assert!(b.parser.screen().contents().contains("END-MARKER"));
    if !data.starts_with(b"\x1b[?1049h") {
        let logical_lines = data.iter().filter(|byte| **byte == b'\n').count() + 1;
        assert!(
            b.history_len() + rows as usize >= logical_lines,
            "pipeline lost rows: retained={} expected={logical_lines}",
            b.history_len() + rows as usize
        );
    }
    (secs, b.history_len())
}
#[test]
#[ignore = "benchmark: real TermBuffer ingest plus render"]
fn bench_terminal_pipeline_profiles() {
    for kind in ["plain", "mixed", "color", "unicode", "tui"] {
        let data = pipeline_stream(kind);
        for (rows, cols) in [(24, 80), (50, 200)] {
            for chunk in [4 * 1024, 32 * 1024] {
                let mut samples = Vec::with_capacity(9);
                let mut history = 0;
                for _ in 0..9 {
                    let (secs, count) = one_pipeline_pass(&data, rows, cols, chunk);
                    samples.push(secs);
                    history = count;
                }
                samples.sort_by(f64::total_cmp);
                println!(
                    "pipeline kind={kind} size={}x{} chunk={} bytes={} median9={:.3}s throughput={:.2}MiB/s history={history}",
                    cols, rows, chunk, data.len(), samples[4],
                    data.len() as f64 / 1_048_576.0 / samples[4],
                );
            }
        }
    }
}
#[test]
#[ignore = "benchmark: retained-output resize plus render"]
fn bench_terminal_pipeline_resize() {
    let mut data = Vec::with_capacity(1024 * 1024 + 128);
    let line = b"2026-10-13 INFO retained history row 0123456789 abcdefghijklmnop\r\n";
    while data.len() < 1024 * 1024 {
        data.extend_from_slice(line);
    }
    data.extend_from_slice(b"RESIZE-MARKER");
    let mut b = TermBuffer::new(24, 80);
    for part in data.chunks(32 * 1024) {
        b.ingest(part);
    }
    b.render();
    assert!(b.displayed_text.iter().any(|s| s.contains("RESIZE-MARKER")));
    let initial_history = b.history_len();
    let start = Instant::now();
    for n in 0..10 {
        let cols = if n % 2 == 0 { 200 } else { 80 };
        b.reflow(24, cols);
        black_box(b.render());
        assert_eq!(b.parser.screen().size(), (24, cols));
    }
    let elapsed = start.elapsed().as_secs_f64();
    assert!(b.displayed_text.iter().any(|s| s.contains("RESIZE-MARKER")));
    println!(
        "resize retained={} bytes 10 alternations={elapsed:.3}s history={}→{}",
        data.len(),
        initial_history,
        b.history_len()
    );
}
#[test]
#[ignore = "benchmark: 100k retained rows and process memory"]
fn bench_terminal_pipeline_history_memory() {
    let pid = sysinfo::Pid::from_u32(std::process::id());
    let before = sysinfo::System::new_all().process(pid).unwrap().memory();
    let mut data = Vec::with_capacity(8 * 1024 * 1024);
    for n in 0..100_000 {
        data.extend_from_slice(format!("{n:06} {:<72}\r\n", "stable log row").as_bytes());
    }
    let mut b = TermBuffer::new(24, 80);
    let start = Instant::now();
    for part in data.chunks(32 * 1024) {
        b.ingest(part);
    }
    black_box(b.render());
    let elapsed = start.elapsed().as_secs_f64();
    assert_eq!(b.history_len(), 100_000 - 24 + 1);
    assert_eq!(b.history_lines().next().unwrap().0.get(..6), Some("000000"));
    assert!(b.displayed_text.iter().any(|s| s.starts_with("099999")));
    let after = sysinfo::System::new_all().process(pid).unwrap().memory();
    println!("history lines=100000 bytes={} history={} elapsed={elapsed:.3}s rss_before={} rss_after={} delta={}",
        data.len(), b.history_len(), before, after, after.saturating_sub(before));
}
#[test]
fn emoji_cluster_survives_byte_chunks_and_copy_find() {
    let mut b = TermBuffer::new(3, 30);
    for byte in "a🧑‍💻b".as_bytes().chunks(1) {
        b.ingest(byte);
    }
    assert_eq!(visible(&mut b)[0], "a🧑‍💻b");
    b.sel_anchor = Some((0, 1));
    b.sel_focus = Some((0, 2));
    assert_eq!(b.extract_selection_text(), "🧑‍💻");
    b.set_find_query("🧑‍💻");
    let hits = b.find_matches();
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].col, hits[0].len), (1, 2));
}

#[test]
fn native_history_ring_eviction_keeps_consecutive_rows_and_view_anchor() {
    let mut b = TermBuffer::new(3, 20);
    let mut bytes = Vec::with_capacity(800_000);
    for n in 0..100_050 {
        bytes.extend_from_slice(format!("{n:06}\r\n").as_bytes());
    }
    for part in bytes.chunks(32 * 1024) {
        b.ingest(part);
    }
    assert_eq!(b.history_len(), MAX_HISTORY);
    let mut first = None;
    for (index, line) in b.history_lines().enumerate() {
        let number = line.0[..6].parse::<usize>().unwrap();
        if first.is_none() {
            first = Some(number);
        }
        assert_eq!(number, first.unwrap() + index);
    }
    assert!(first.unwrap() > 0);
    b.view_offset = 2;
    let before = visible(&mut b);
    let old_offset = b.view_offset;
    b.ingest(b"100050\r\n100051\r\n");
    assert_eq!(visible(&mut b), before);
    assert_eq!(b.view_offset, old_offset + 2);
}

#[test]
fn resize_after_more_than_raw_cap_preserves_all_native_rows() {
    let mut b = TermBuffer::new(24, 80);
    let mut bytes = Vec::with_capacity(3_200_000);
    for n in 0..50_000 {
        bytes.extend_from_slice(format!("{n:05} {:<55}\r\n", "retained row").as_bytes());
    }
    assert!(bytes.len() > 2 * 1024 * 1024);
    for part in bytes.chunks(32 * 1024) {
        b.ingest(part);
    }
    b.reflow(24, 100);
    b.render();
    let ids = b
        .history_lines()
        .map(|line| line.0)
        .chain(b.displayed_text.iter().cloned())
        .filter(|line| !line.trim().is_empty())
        .map(|line| line[..5].parse::<usize>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 50_000);
    for (expected, actual) in ids.into_iter().enumerate() {
        assert_eq!(actual, expected, "row lost or duplicated at {expected}");
    }
}

#[test]
fn an_open_synchronized_update_does_not_hide_final_output() {
    let mut b = TermBuffer::new(4, 30);
    b.ingest(b"before\x1b[?2026h");
    b.ingest(b"-tail");
    assert_eq!(visible(&mut b)[0], "before-tail");
    assert_eq!(b.ingest(b"\x1b[6n"), b"\x1b[1;12R");
}

#[test]
fn retained_selection_survives_native_history_eviction() {
    let mut b = TermBuffer::new(3, 20);
    b.parser = TerminalEngine::new(3, 20, 3);
    b.ingest(b"zero\r\none\r\ntwo\r\nthree\r\nfour\r\nfive");
    visible(&mut b);
    let row = b.history_len();
    b.sel_anchor = Some((row, 0));
    b.sel_focus = Some((row, 4));
    let selected = b.extract_selection_text();
    assert_eq!(selected, "three");
    b.ingest(b"\r\nsix");
    assert!(b.selection_has_extent());
    assert_eq!(b.extract_selection_text(), selected);
}

#[test]
fn interrupted_charset_designator_cannot_swallow_following_text() {
    for drawing in [false, true] {
        let mut b = TermBuffer::new(3, 20);
        b.vt100_drawing = drawing;
        for byte in b"\x1b(\x1b(Bq" {
            b.ingest(&[*byte]);
        }
        assert_eq!(visible(&mut b)[0], "q");
    }
}
