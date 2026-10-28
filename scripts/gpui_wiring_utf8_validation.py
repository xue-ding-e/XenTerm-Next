"""Supplemental Python validation, not a fresh Rust matrix or native Dock test."""
import contextlib
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import urllib.request

PRODUCT = "43769a2d014ea15e5e52e7e11830c143bc873b1a"
BASE = "0205fe53a74fc286d075a3f7a9b56556be3e99f4"
OLD_CONTROL = "0372b4e3b094854e1f9a5128219e310eee794e07"
REPO = "xue-ding-e/XenTerm-Next"
RUN = 40605028307
PATH = "tests/macos_reopen_wiring_test.py"
# These counts were verified in each completed job's log. The supplemental run
# rechecks job identities/outcomes, but does not claim to rerun those Rust tests.
RUST_EVIDENCE = {
    122542284318: ("success", "ubuntu-22.04", [8, 11, 8]),
    122542284317: ("success", "macos-14", [8, 11, 8]),
    122542284021: ("failure", "windows-latest", [8, 11, 8]),
}


def git(root, *args):
    return subprocess.run(["git", "-C", str(root), *args], check=True,
                          capture_output=True, timeout=30).stdout


def entries(raw):
    result = {}
    for line in raw.split(b"\0"):
        if line:
            mode_type_oid, name = line.split(b"\t", 1)
            result[name] = mode_type_oid
    return result


def verify_delta(root):
    assert git(root, "rev-parse", "HEAD").decode().strip() == PRODUCT
    parents = git(root, "show", "-s", "--format=%P", PRODUCT).decode().split()
    assert parents == [BASE], parents
    old = entries(git(root, "ls-tree", "-rz", BASE))
    new = entries(git(root, "ls-tree", "-rz", PRODUCT))
    assert old.keys() == new.keys()
    assert [p for p in old if old[p] != new[p]] == [PATH.encode()]
    assert old[PATH.encode()].split()[:2] == new[PATH.encode()].split()[:2]
    before = git(root, "show", f"{BASE}:{PATH}")
    after = git(root, "show", f"{PRODUCT}:{PATH}")
    needle = b'(ROOT / "src/ui/impls/shell.rs").read_text()'
    replacement = b'(ROOT / "src/ui/impls/shell.rs").read_text(encoding="utf-8")'
    assert before.count(needle) == 1
    assert before.replace(needle, replacement, 1) == after
    print("Exact one-line delta verified; every other Git tree entry/mode is identical.")
    return before, after, git(root, "show", f"{PRODUCT}:src/ui/impls/shell.rs")


def get_api(suffix):
    request = urllib.request.Request(
        f"https://api.github.com/repos/{REPO}/{suffix}",
        headers={"Accept": "application/vnd.github+json", "User-Agent": "XenTerm-Next-validation"},
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        data = response.read(1024 * 1024 + 1)
    assert len(data) <= 1024 * 1024
    return json.loads(data)


def verify_prior_run():
    run = get_api(f"actions/runs/{RUN}")
    assert run["id"] == RUN and run["run_attempt"] == 1
    assert run["head_sha"] == OLD_CONTROL
    assert run["repository"]["full_name"] == REPO
    assert run["status"] == "completed" and run["conclusion"] == "failure"
    jobs = get_api(f"actions/runs/{RUN}/attempts/1/jobs?per_page=100")
    assert jobs["total_count"] == 3 and len(jobs["jobs"]) == 3
    assert {j["id"] for j in jobs["jobs"]} == set(RUST_EVIDENCE)
    for job in jobs["jobs"]:
        conclusion, label, counts = RUST_EVIDENCE[job["id"]]
        assert job["head_sha"] == OLD_CONTROL and job["run_attempt"] == 1
        assert job["status"] == "completed" and job["conclusion"] == conclusion
        assert label in job["labels"]
        print(f"Existing Rust log evidence: job {job['id']}, counts {counts}, 0 ignored; job outcome {conclusion}.")
    print(f"Original run https://github.com/{REPO}/actions/runs/{RUN} remains FAILURE.")
    print("Rust tests are NOT rerun by this supplemental Python workflow.")


ORIGINAL_OPEN = Path.open


def cp1252_default(self, mode="r", buffering=-1, encoding=None, errors=None, newline=None):
    if "b" not in mode and encoding in (None, "locale"):
        encoding = "cp1252"
    return ORIGINAL_OPEN(self, mode, buffering, encoding, errors, newline)


def load_tests(script, root, simulate_cp1252):
    saved_argv = sys.argv
    sys.argv = [str(script), str(root)]
    try:
        with patch.object(Path, "open", cp1252_default) if simulate_cp1252 else contextlib.nullcontext():
            module = runpy.run_path(str(script), run_name="encoding_fixture_check")
    finally:
        sys.argv = saved_argv
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(module["MacosReopenWiring"])
    assert suite.countTestCases() == 5
    result = unittest.TextTestRunner(verbosity=1).run(suite)
    assert result.testsRun == 5 and result.wasSuccessful() and not result.skipped


def verify_encoding(before, after, source):
    # Only temporary synthetic copies are mutated; the checkout stays untouched.
    with tempfile.TemporaryDirectory(prefix="xenterm-wiring-utf8-") as temp:
        root = Path(temp)
        src = root / "src/ui/impls/shell.rs"
        src.parent.mkdir(parents=True)
        old_script, new_script = root / "old.py", root / "new.py"
        old_script.write_bytes(before)
        new_script.write_bytes(after)
        lf = source.replace(b"\r\n", b"\n")
        for newline, contents, offset in (("LF", lf, 6126), ("CRLF", lf.replace(b"\n", b"\r\n"), 6245)):
            src.write_bytes(contents)
            try:
                load_tests(old_script, root, True)
            except UnicodeDecodeError as exc:
                assert exc.encoding == "charmap" and exc.start == offset
                assert exc.object[exc.start] == 0x8F
                print(f"RED {newline}: old fixture fails cp1252 at byte {offset}.")
            else:
                raise AssertionError("Old fixture unexpectedly decoded with cp1252")
            load_tests(new_script, root, True)
            print(f"GREEN {newline}: explicit UTF-8, all five original wiring assertions passed.")
            load_tests(new_script, root, False)
            print(f"GREEN {newline}: native platform default, all five assertions passed.")


def main():
    root = Path(sys.argv[1]).resolve()
    before, after, source = verify_delta(root)
    verify_prior_run()
    load_tests(root / PATH, root, False)
    verify_encoding(before, after, source)
    assert not git(root, "status", "--porcelain", "--untracked-files=no").strip()
    print("Python supplement passed; native macOS Dock event delivery remains unverified.")


if __name__ == "__main__":
    main()
