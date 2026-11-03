//! Local diagnostic only. Fixed strings, native platform text system, no UI content.
//! No assertions of visual correctness: this records GPUI's public layout geometry.
use gpui::{App, LineLayout, TextRun, WindowTextSystem, font, px};
use serde_json::{Value, json};

fn snapshot(layout: &LineLayout, text: &str, system: &WindowTextSystem) -> Value {
    let boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i)
        .chain(std::iter::once(text.len())).collect();
    json!({
        "width": f32::from(layout.width),
        "ascent": f32::from(layout.ascent),
        "descent": f32::from(layout.descent),
        "utf8_carets": boundaries.iter().map(|&index| {
            let x = layout.x_for_index(index);
            json!({"byte_index": index, "x": f32::from(x),
                "index_for_x": layout.index_for_x(x),
                "closest_index_for_x": layout.closest_index_for_x(x)})
        }).collect::<Vec<_>>(),
        "runs": layout.runs.iter().map(|run| json!({
            "font_id": run.font_id.0,
            // This is GPUI's request cache, not authoritative fallback-font metadata.
            "cached_requested_family": system.get_font_for_id(run.font_id)
                .map(|f| f.family.to_string()),
            "glyphs": run.glyphs.iter().map(|g| json!({
                "id": g.id.0, "byte_index": g.index,
                "x": f32::from(g.position.x), "y": f32::from(g.position.y),
                "emoji": g.is_emoji,
            })).collect::<Vec<_>>()
        })).collect::<Vec<_>>()
    })
}

// Bounded native shaping stress only, not Input rendering or allocation profiling.
fn stress_probe(cx: &App, available: &[String]) -> Value {
    let candidates = ["Noto Sans", "DejaVu Sans", "Liberation Sans"];
    let Some(family) = candidates.iter().find(|candidate|
        available.iter().any(|name| name == **candidate)) else {
        return json!({"status": "skipped_missing_font", "candidates": candidates});
    };
    const BYTES: usize = 64 * 1024;
    const SPANS: usize = 4096;
    const MAX_RUNS: usize = 2 * SPANS + 1;
    let base = TextRun { len: 0, font: font((*family).to_owned()),
        color: gpui::black(), ..Default::default() };
    let mut runs = Vec::with_capacity(MAX_RUNS);
    let mut cursor = 0;
    for span in 0..SPANS {
        // One colored ASCII byte centered in each sixteen-byte block, with
        // ordinary foreground gaps before, between, and after the spans.
        let start = span * 16 + 8;
        runs.push(TextRun { len: start - cursor, ..base.clone() });
        runs.push(TextRun { len: 1, color: gpui::rgb(0xff0000).into(), ..base.clone() });
        cursor = start + 1;
    }
    runs.push(TextRun { len: BYTES - cursor, ..base });
    assert_eq!(runs.len(), MAX_RUNS);
    assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), BYTES);
    let mut observations = Vec::with_capacity(3);
    for iteration in 0..3 {
        let mut text = "abcdefghijklmnop".repeat(SPANS);
        // Deterministic harmless cold variation, preserving exact byte count.
        text.replace_range(0..1, ["0", "1", "2"][iteration]);
        assert_eq!(text.len(), BYTES);
        // Each iteration owns a fresh line-layout cache. Native platform/font
        // caches remain shared: do not interpret these as fully cold process runs.
        let system = WindowTextSystem::new(cx.text_system().clone());
        let started = std::time::Instant::now();
        let layout = system.layout_line(&text, px(20.0), &runs, None);
        let elapsed = started.elapsed();
        observations.push(json!({"iteration": iteration,
            "layout_elapsed_ms": elapsed.as_secs_f64() * 1000.0,
            "width": f32::from(layout.width),
            "glyph_count": layout.runs.iter().map(|run| run.glyphs.len()).sum::<usize>()}));
    }
    json!({"status": "observed", "text_bytes": BYTES, "colored_spans": SPANS,
        "input_runs": runs.len(), "max_input_runs": MAX_RUNS,
        "requested_family": family, "font_size_px": 20,
        "cache_scope": "fresh WindowTextSystem per iteration; platform/font caches shared",
        "timing_scope": "layout_line only, excludes setup and glyph counting",
        "iterations": observations})
}

fn probe(cx: &mut App) -> Value {
    let system = WindowTextSystem::new(cx.text_system().clone());
    let available = system.all_font_names();
    // Model ordinary Input: one Latin-capable primary font for all text. Do not
    // directly resolve script-only families or explicit fallback chains here:
    // this pinned native loader removes faces without ASCII 'm'. Cosmic's
    // automatic per-glyph fallback uses a different path. Listing a required
    // installed family is a coverage prerequisite, not proof it shaped glyphs.
    let primary_family = "Noto Sans";
    let cases: &[(&str, &str, &str)] = &[
        ("latin_ligature", "ffi", "Noto Sans"),
        ("latin_word", "office", "Noto Sans"),
        ("arabic", "سلام", "Noto Sans Arabic"),
        ("devanagari", "क्षि", "Noto Sans Devanagari"),
        ("combining", "e\u{301}cole", "Noto Sans"),
        ("cjk", "中文日本", "Noto Sans CJK SC"),
        ("emoji", "👩\u{200d}💻🙂", "Noto Color Emoji"),
    ];
    let mut results = Vec::new();
    for &(name, text, required_coverage_family) in cases {
        if !available.iter().any(|name| name == primary_family)
            || !available.iter().any(|name| name == required_coverage_family)
        {
            results.push(json!({"case": name, "status": "skipped_missing_font",
                "requested_family": primary_family,
                "required_coverage_family": required_coverage_family}));
            continue;
        }
        let base = TextRun { len: text.len(), font: font(primary_family),
            color: gpui::black(), ..Default::default() };
        let plain = system.layout_line(text, px(20.0), &[base.clone()], None);
        let plain_snapshot = snapshot(&plain, text, &system);
        let mut splits = Vec::new();
        for split in text.char_indices().map(|(i, _)| i).skip(1) {
            let left = TextRun { len: split, ..base.clone() };
            let right = TextRun { len: text.len() - split, ..base.clone() };
            let control = system.layout_line(text, px(20.0), &[left.clone(), right.clone()], None);
            let colored = system.layout_line(text, px(20.0), &[left,
                TextRun { color: gpui::rgb(0xff0000).into(), ..right }], None);
            let control_snapshot = snapshot(&control, text, &system);
            let colored_snapshot = snapshot(&colored, text, &system);
            // Sample identical physical x coordinates for all variants, including
            // outside either edge. This is independent of glyph/cluster boundaries.
            let extent = f32::from(plain.width).max(f32::from(colored.width));
            let hit_tests: Vec<_> = (-1..=17).map(|i| {
                let x = px(extent * (i as f32 / 16.0));
                json!({"x": f32::from(x),
                    "plain": [plain.index_for_x(x), Some(plain.closest_index_for_x(x))],
                    "control": [control.index_for_x(x), Some(control.closest_index_for_x(x))],
                    "colored": [colored.index_for_x(x), Some(colored.closest_index_for_x(x))]})
            }).collect();
            let caret_delta = text.char_indices().map(|(i, _)| i)
                .chain(std::iter::once(text.len())).map(|i|
                    f32::from(plain.x_for_index(i) - colored.x_for_index(i)).abs())
                .fold(0.0_f32, f32::max);
            splits.push(json!({"split_byte": split,
                "control_matches_plain_exactly": control_snapshot == plain_snapshot,
                "colored_matches_plain_exactly": colored_snapshot == plain_snapshot,
                "width_delta": f32::from(colored.width - plain.width),
                "max_caret_x_delta": caret_delta,
                "colored": colored_snapshot, "control": control_snapshot,
                "fixed_x_hit_tests": hit_tests}));
        }
        results.push(json!({"case": name, "status": "observed", "text": text,
            "requested_family": primary_family,
            "required_coverage_family": required_coverage_family,
            "font_selection": "primary plus native automatic fallback; no explicit fallback list",
            "plain_missing_glyph_count": plain.runs.iter().flat_map(|r| &r.glyphs)
                .filter(|g| g.id.0 == 0).count(),
            "plain_emoji_glyph_count": plain.runs.iter().flat_map(|r| &r.glyphs)
                .filter(|g| g.is_emoji).count(),
            "font_size_px": 20,
            "plain": plain_snapshot, "splits": splits}));
    }
    json!({"schema": "gpui-native-color-runs-v2", "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH, "gpui_pre_version": "0.3.8",
        "platform_entry": "gpui_platform::application() (native, non-test)",
        "note": "Observed layout only; no rendering, screenshot, Input interaction, grapheme or bidi correctness claim. Cached requested families may not identify fallback fonts. Glyph id 0 can indicate missing coverage.",
        "cases": results, "native_shaping_stress": stress_probe(cx, &available)})
}

fn main() {
    // The real Application supplies the platform text system. Do not replace this
    // with #[gpui::test], TestAppContext::default(), or NoopTextSystem.
    gpui_platform::application().run(|cx: &mut App| {
        let result = probe(cx);
        println!("{}", result);
        // Linux calls this launch callback before calloop::run, which resets
        // its stop flag. Enqueue quit for the foreground event loop instead.
        cx.spawn(async |cx| {
            cx.update(|cx| cx.quit());
        }).detach();
    });
}
