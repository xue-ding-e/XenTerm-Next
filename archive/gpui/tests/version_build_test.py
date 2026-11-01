"""Exercise the actual build script in a Linux dependency-free Cargo fixture."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    source = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="xenterm-version-incremental-") as directory:
        root = Path(directory)
        (root / "src").mkdir()
        (root / "scripts").mkdir()
        # All watched paths must exist; a missing .git path makes Cargo rerun
        # unconditionally and would hide missing environment invalidation.
        (root / ".git/refs").mkdir(parents=True)
        (root / ".git/HEAD").write_text("ref: refs/heads/main\n")
        (root / ".git/packed-refs").write_text("")
        shutil.copy(source / "build.rs", root / "build.rs")
        shutil.copy(source / "scripts/version.sh", root / "scripts/version.sh")
        (root / "Cargo.toml").write_text(
            '[package]\nname="version-fixture"\nversion="1.2.3"\nedition="2021"\n')
        (root / "src/main.rs").write_text('fn main(){println!("{}",env!("XT_VERSION"));}')
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(("XT_", "GITHUB_", "GIT_"))}
        env.update(GITHUB_REF_TYPE="branch", GITHUB_REF_NAME="main",
                   GITHUB_SHA="abcdef0123456789012345678901234567890123",
                   GIT_CEILING_DIRECTORIES=str(root.parent))

        def run():
            return subprocess.check_output(
                ["cargo", "run", "--offline", "--quiet"], cwd=root, env=env,
                text=True, timeout=60).strip()

        first = run()
        assert first.startswith("xt-dev-abcdef0-T"), first
        env["XT_CHANNEL"] = "alpha"
        second = run()
        assert second.startswith("xt-alpha-abcdef0-T"), second
        env["GITHUB_SHA"] = "1234567890123456789012345678901234567890"
        third = run()
        assert third.startswith("xt-alpha-1234567-T"), third
        env["XT_VERSION"] = "xt-v1.2.3"
        assert run() == "xt-v1.2.3"
        del env["XT_VERSION"]
        del env["XT_CHANNEL"]
        env.update(GITHUB_REF_TYPE="tag", GITHUB_REF_NAME="v2.3.4")
        assert run() == "xt-v2.3.4"
        print("PASS: five actual incremental builds; no external dependencies")


if __name__ == "__main__":
    main()
