"""Run vendored MAC tests with their own dev-dependency lock, not the product lock."""
import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

TESTS = (
    "mac::md5_compat_tests::rfc_2202_full_length_vectors",
    "mac::md5_compat_tests::packet_mac_binds_sequence_payload_and_full_tag",
    "mac::md5_compat_tests::md5_registered_but_not_default",
)


def validate_source(source):
    manifest = tomllib.loads((source / "Cargo.toml").read_text(encoding="utf-8"))
    if "workspace" in manifest or "patch" in manifest:
        raise ValueError("expected normalized standalone vendor manifest")
    # Published normalized dependencies must not reach outside the copy.
    def check(value):
        if isinstance(value, dict):
            if "path" in value or "workspace" in value:
                raise ValueError("non-standalone dependency")
            for nested in value.values():
                check(nested)
        elif isinstance(value, list):
            for nested in value:
                check(nested)
    for section, value in manifest.items():
        if section in ("dependencies", "dev-dependencies", "build-dependencies", "target"):
            check(value)
    packages = tomllib.loads((source / "Cargo.lock").read_text(encoding="utf-8"))["package"]
    for package in packages:
        for dependency in package.get("dependencies", []):
            name, *rest = dependency.split()
            candidates = [item for item in packages if item["name"] == name
                          and (not rest or item["version"] == rest[0])]
            if len(candidates) != 1:
                raise ValueError("unresolved or ambiguous locked dependency: " + dependency)
    russh = next(item for item in packages if item["name"] == "russh")
    if "md-5" not in russh["dependencies"]:
        raise ValueError("missing direct MD5 dependency")
    md5 = next(item for item in packages if item["name"] == "md-5")
    if (md5["version"], md5["checksum"]) != (
        "0.11.0", "69b6441f590336821bb897fb28fc622898ccceb1d6cea3fde5ea86b090c4de98"
    ):
        raise ValueError("unexpected MD5 lock identity")


def run_tests(root, runner=subprocess.run):
    source = root / "vendor/russh-0.61.2"
    validate_source(source)
    # A workspace marker also protects against TMP/TEMP pointing into a checkout.
    with tempfile.TemporaryDirectory(prefix="russh-mac-tests-") as temporary:
        crate = Path(temporary) / "russh"
        shutil.copytree(source, crate)
        with (crate / "Cargo.toml").open("a", encoding="utf-8") as stream:
            stream.write("\n[workspace]\n")
        lock_before = (crate / "Cargo.lock").read_bytes()
        environment = os.environ.copy()
        # An independent cache avoids confusing standalone and product artifacts.
        environment["CARGO_TARGET_DIR"] = str(root / "target/russh-mac-tests")
        command = ["cargo", "test", "--locked", "--no-default-features",
                   "--features", "ring,rsa,flate2", "--lib", "md5_compat_tests",
                   "--", "--test-threads=1"]
        result = runner(command, cwd=crate, env=environment, text=True,
                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=1200)
        print(result.stdout)
        if result.returncode:
            raise RuntimeError("vendored MAC tests failed")
        if (crate / "Cargo.lock").read_bytes() != lock_before:
            raise RuntimeError("locked vendor graph changed")
        passed = set(re.findall(r"^test (\S+) \.\.\. ok$", result.stdout, re.MULTILINE))
        if passed != set(TESTS) or not re.search(
            r"test result: ok\. 3 passed; 0 failed; 0 ignored;", result.stdout
        ):
            raise RuntimeError("expected all three MAC regressions, not zero/ignored tests")
        print("VENDORED_MAC_TESTS_PASS count=3")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    run_tests(parser.parse_args().root.resolve())
