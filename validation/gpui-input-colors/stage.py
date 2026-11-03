"""Stage only into a new disposable directory; never touch product dependencies."""
import hashlib
import io
import json
import pathlib
import subprocess
import sys
import tarfile
import tomllib
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
PINS = {
    "gpui-base-0.7.1.crate": "3cf698677b253118459f552fe15513858c669d79a25837a94e480db2ae330050",
    "input-text-colors.patch": "7f6e8a5c88741e05455ad934f0ebd7f24149d1d40583e1f04fe6f885479868c4",
    "native_text_probe.rs": "98d7f69c09537d7ca52a158c2d7ad5c3b623d9cf11504979180f05ba709df6c3",
    "example-manifest-stanza.toml": "787218d89ae02aed371d84a99bf10573aa13660e83ea25a453b93817ef6e2cc3",
    # Exact workspace fixture from the source commit recorded in .cargo_vcs_info.json:
    # https://github.com/longbridge/gpui-kit/blob/87d10ae5e1299d1be18670a37570c5402f952e34/README.md
    "upstream-readme.fixture": "4c535f781dd2ac0c6687e50b9a55cf7acbb049086d09b2ef64f1538224518d89",
}

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def stage(destination):
    for name, expected in PINS.items():
        if name.endswith(".crate"):
            continue
        if sha(HERE / name) != expected:
            raise ValueError(f"Input hash mismatch: {name}")
    # Official immutable registry archive only, hard-capped before extraction.
    url = "https://static.crates.io/crates/gpui-base/gpui-base-0.7.1.crate"
    with urllib.request.urlopen(url, timeout=30) as response:
        if response.url != url:
            raise ValueError("Unexpected source archive redirect")
        blob = response.read(2 * 1024**2 + 1)
    if len(blob) > 2 * 1024**2 or hashlib.sha256(blob).hexdigest() != PINS["gpui-base-0.7.1.crate"]:
        raise ValueError("Source archive size or SHA256 mismatch")
    destination = pathlib.Path(destination).absolute()
    destination.mkdir(parents=False, exist_ok=False)
    # Restore the upstream relative layout inside this owned disposable stage.
    # A packaged lib test includes ../../../../README.md from src/text/.
    workspace = destination / "workspace"
    root = workspace / "crates" / "base"
    # Reject links, devices, duplicate names and traversal before writing anything.
    with tarfile.open(fileobj=io.BytesIO(blob), mode="r:gz") as archive:
        members = archive.getmembers()
        seen = set()
        if len(members) > 10000 or sum(m.size for m in members) > 64 * 1024**2:
            raise ValueError("Archive bounds exceeded")
        for member in members:
            path = pathlib.PurePosixPath(member.name)
            if (path.is_absolute() or ".." in path.parts or "\\" in member.name
                    or not path.parts or path.parts[0] != "gpui-base-0.7.1"
                    or member.name in seen or not (member.isfile() or member.isdir())):
                raise ValueError(f"Unsafe archive member: {member.name}")
            seen.add(member.name)
        for member in members:
            target = root.joinpath(*pathlib.PurePosixPath(member.name).parts[1:])
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with archive.extractfile(member) as source, target.open("xb") as sink:
                    sink.write(source.read())
    source = json.loads((root / ".cargo_vcs_info.json").read_text())
    if (source["git"]["sha1"] != "87d10ae5e1299d1be18670a37570c5402f952e34"
            or source["path_in_vcs"] != "crates/base"):
        raise ValueError("Upstream workspace fixture source mismatch")
    with (workspace / "README.md").open("xb") as fixture:
        fixture.write((HERE / "upstream-readme.fixture").read_bytes())
    manifest = root / "Cargo.toml"
    original = manifest.read_bytes()
    parsed = tomllib.loads(original.decode())
    dep = parsed["dev-dependencies"]["gpui_platform"]
    if dep["version"] != "=0.3.8" or dep["package"] != "gpui-pre-platform":
        raise ValueError("Native platform dependency changed")
    lock = sha(root / "Cargo.lock")
    # git apply checks all context exactly and has no patch(1) fuzz fallback.
    patch = HERE / "input-text-colors.patch"
    subprocess.run(["git", "apply", "--check", str(patch)], cwd=root, check=True, timeout=20)
    subprocess.run(["git", "apply", str(patch)], cwd=root, check=True, timeout=20)
    examples = root / "examples"
    examples.mkdir(exist_ok=True)
    with (examples / "native_text_probe.rs").open("xb") as out:
        out.write((HERE / "native_text_probe.rs").read_bytes())
    manifest.write_bytes(original + b"\n" + (HERE / "example-manifest-stanza.toml").read_bytes())
    if sha(root / "Cargo.lock") != lock:
        raise ValueError("Staging changed Cargo.lock")
    print(f"source_crate_sha256={PINS['gpui-base-0.7.1.crate']}")
    print(f"patch_sha256={PINS['input-text-colors.patch']}")
    print(f"workspace_fixture_sha256={PINS['upstream-readme.fixture']}")
    print(f"lock_sha256={lock}")

if __name__ == "__main__":
    stage(sys.argv[1])
