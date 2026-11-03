"""Fail closed on missing native coverage or changed geometry, not a UI verdict."""
import json
import math
import pathlib
import sys

raw = pathlib.Path(sys.argv[1]).read_bytes()
if len(raw) > 2 * 1024**2:
    raise ValueError("Native report exceeds 2 MiB")
# Platform diagnostics may precede stdout; require exactly one schema report.
reports = []
for line in raw.splitlines():
    if line.startswith(b'{"'):
        value = json.loads(line)
        if value.get("schema") == "gpui-native-color-runs-v2":
            reports.append(value)
if len(reports) != 1:
    raise ValueError("Expected exactly one native geometry report")
report = reports[0]
assert report["os"] == "linux" and report["gpui_pre_version"] == "0.3.8"
assert report["platform_entry"] == "gpui_platform::application() (native, non-test)"
families = {
    "latin_ligature": "Noto Sans", "latin_word": "Noto Sans",
    "arabic": "Noto Sans Arabic", "devanagari": "Noto Sans Devanagari",
    "combining": "Noto Sans", "cjk": "Noto Sans CJK SC", "emoji": "Noto Color Emoji",
}
texts = {
    "latin_ligature": "ffi", "latin_word": "office", "arabic": "سلام",
    "devanagari": "क्षि", "combining": "e\u0301cole", "cjk": "中文日本",
    "emoji": "👩\u200d💻🙂",
}
assert len(report["cases"]) == len(families)
assert {case["case"] for case in report["cases"]} == families.keys()

def geometry(snapshot, require_emoji=False):
    assert all(math.isfinite(snapshot[key]) for key in ("width", "ascent", "descent"))
    assert snapshot["width"] > 0
    glyphs = [(run["font_id"], glyph) for run in snapshot["runs"] for glyph in run["glyphs"]]
    assert glyphs and all(glyph["id"] != 0 for _, glyph in glyphs), "Missing glyph coverage"
    assert all(isinstance(glyph["emoji"], bool) for _, glyph in glyphs)
    if require_emoji:
        assert any(glyph["emoji"] for _, glyph in glyphs), "Missing native emoji glyph coverage"
    return (snapshot["width"], snapshot["ascent"], snapshot["descent"], snapshot["utf8_carets"], glyphs)

for case in report["cases"]:
    assert case["status"] == "observed", f"Missing coverage: {case['case']}"
    # Installed coverage families do not identify the font that actually shaped
    # a fallback glyph. All requests model one ordinary Latin-capable primary.
    assert case["requested_family"] == "Noto Sans", "Unexpected primary family"
    assert case["required_coverage_family"] == families[case["case"]]
    assert case["font_selection"] == "primary plus native automatic fallback; no explicit fallback list"
    assert case["text"] == texts[case["case"]], "Unexpected coverage text"
    assert case["font_size_px"] == 20
    boundaries = [len(case["text"][:i].encode()) for i in range(1, len(case["text"]))]
    assert [split["split_byte"] for split in case["splits"]] == boundaries
    require_emoji = case["case"] == "emoji"
    plain = geometry(case["plain"], require_emoji)
    plain_glyphs = [glyph for run in case["plain"]["runs"] for glyph in run["glyphs"]]
    assert case["plain_missing_glyph_count"] == sum(glyph["id"] == 0 for glyph in plain_glyphs) == 0
    assert case["plain_emoji_glyph_count"] == sum(glyph["emoji"] for glyph in plain_glyphs)
    for split in case["splits"]:
        assert geometry(split["control"], require_emoji) == plain, f"Control geometry changed: {case['case']}"
        assert geometry(split["colored"], require_emoji) == plain, f"Color geometry changed: {case['case']}"
        assert split["width_delta"] == 0 and split["max_caret_x_delta"] == 0
        assert len(split["fixed_x_hit_tests"]) == 19
        assert all(hit["plain"] == hit["control"] == hit["colored"] for hit in split["fixed_x_hit_tests"])
stress = report["native_shaping_stress"]
assert stress["status"] == "observed" and stress["requested_family"] == "Noto Sans"
assert stress["text_bytes"] == 65536 and stress["colored_spans"] == 4096
assert stress["input_runs"] == stress["max_input_runs"] == 8193
assert [item["iteration"] for item in stress["iterations"]] == [0, 1, 2]
for item in stress["iterations"]:
    assert math.isfinite(item["layout_elapsed_ms"]) and item["layout_elapsed_ms"] >= 0
    assert math.isfinite(item["width"]) and item["width"] > 0
    assert item["glyph_count"] > 0
# Additive bounded diagnostic; this does not change the seven-script gate above.
from statistics import median

command = report["command_size_shaping"]
assert command["status"] == "observed" and command["requested_family"] == "Noto Sans"
assert command["font_size_px"] == 20
assert command["cache_scope"] == "fresh WindowTextSystem per measurement; platform/font caches shared"
assert command["timing_scope"] == "layout_line only; excludes setup, counting and geometry comparisons"
assert command["sample_limit"] == "4 sizes x 3 iterations x 3 variants = 36 layouts"
expected_sizes = [(128, 16), (512, 64), (1024, 64), (2048, 128)]
assert [(case["text_bytes"], case["colored_spans"]) for case in command["cases"]] == expected_sizes
variants = ["plain", "same_color_control", "colored"]
summaries = []

def finite_number(value):
    return type(value) in (int, float) and math.isfinite(value)

def sample_summary(values):
    return {"samples": len(values), "median_ms": median(values), "max_ms": max(values)}

for case, (byte_count, span_count) in zip(command["cases"], expected_sizes):
    assert type(case["text_bytes"]) is int and type(case["colored_spans"]) is int
    assert type(case["split_input_runs"]) is int and case["split_input_runs"] == 2 * span_count + 1
    assert [item["iteration"] for item in case["iterations"]] == [0, 1, 2]
    samples = {variant: [] for variant in variants}
    for item in case["iterations"]:
        assert type(item["iteration"]) is int
        iteration = item["iteration"]
        assert item["measurement_order"] == variants[iteration:] + variants[:iteration]
        assert item["control_geometry_matches_plain"] is True
        assert item["colored_geometry_matches_plain"] is True
        assert [measurement["variant"] for measurement in item["measurements"]] == variants
        widths = []
        for measurement in item["measurements"]:
            variant = measurement["variant"]
            expected_runs = 1 if variant == "plain" else 2 * span_count + 1
            assert type(measurement["input_runs"]) is int and measurement["input_runs"] == expected_runs
            elapsed = measurement["layout_elapsed_ms"]
            assert finite_number(elapsed) and elapsed >= 0
            assert finite_number(measurement["width"]) and measurement["width"] > 0
            assert type(measurement["glyph_count"]) is int and measurement["glyph_count"] > 0
            assert type(measurement["missing_glyph_count"]) is int and measurement["missing_glyph_count"] == 0
            samples[variant].append(elapsed)
            widths.append(measurement["width"])
        assert widths[0] == widths[1] == widths[2]
    paired = {}
    for left, right in [("colored", "plain"), ("colored", "same_color_control"), ("same_color_control", "plain")]:
        # Preserve negative noisy differences; never clamp them to suggest zero cost.
        differences = [a - b for a, b in zip(samples[left], samples[right])]
        paired[f"{left}_minus_{right}"] = sample_summary(differences)
    summaries.append({"text_bytes": byte_count, "colored_spans": span_count,
                      "absolute_layout_time": {name: sample_summary(values) for name, values in samples.items()},
                      "paired_layout_time_differences": paired})
print(json.dumps({
    "schema": "gpui-command-shaping-summary-v1",
    "coverage_and_geometry": "qualified for these fixed native observations",
    "performance_verdict": "not assessed; no timing threshold is applied",
    "sample_scope": "three samples per variant and size; medians/maxima only, not p95 or FPS",
    "application_candidate_cap": {"text_bytes": 1024, "colored_words": 64},
    "limitations": "Synthetic one-byte ASCII spans, not application tokenization or full Input frame latency. Platform/font caches shared. Direct script/emoji font requests remain unqualified.",
    "cases": summaries,
}, allow_nan=False))
