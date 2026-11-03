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
print("Native Noto Sans primary with automatic fallback geometry qualification passed for seven cases; installed coverage families are prerequisites, not identified fallback faces. Bounded stress observed without performance threshold verdict. Does not qualify direct script/emoji font requests, pixels, actual Input editing/scrolling, bidi/grapheme correctness, other fonts or platforms.")
