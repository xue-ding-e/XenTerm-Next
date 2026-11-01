"""Regression tests for version metadata emitted by the real Bash resolver."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class VersionMetadataTests(unittest.TestCase):
    def resolve(self, ref_type, ref_name, override=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "scripts").mkdir()
            shutil.copyfile(Path(__file__).resolve().parents[1] / "scripts/version.sh", root / "scripts/version.sh")
            (root / "Cargo.toml").write_text('[package]\nversion = "1.2.3"\n', encoding="utf-8")
            env = {key: value for key, value in os.environ.items()
                   if not key.startswith(("XT_", "GITHUB_", "GIT_"))}
            env.update(GITHUB_REF_TYPE=ref_type, GITHUB_REF_NAME=ref_name,
                       GITHUB_SHA="abcdef0123456789012345678901234567890123",
                       GIT_CEILING_DIRECTORIES=str(root.parent))
            if override:
                env["XT_VERSION"] = override
            result = subprocess.run([os.environ.get("VERSION_TEST_BASH", "bash"), str(root / "scripts/version.sh")],
                                    env=env, cwd=root, text=True, capture_output=True, check=True)
            metadata = json.loads((root / "version.json").read_text(encoding="utf-8"))
            self.assertEqual(result.stdout.strip(), metadata["version"])
            self.assertEqual(len(metadata), 9)
            return metadata

    def test_legal_quoted_branch(self):
        branch = 'fix/quote"branch'
        subprocess.run(["git", "check-ref-format", "refs/heads/" + branch], check=True)
        self.assertEqual(self.resolve("branch", branch)["branch"], branch)

    def test_unicode_branch(self):
        branch = 'fix/中文-é-"quote'
        self.assertEqual(self.resolve("branch", branch)["branch"], branch)

    def test_control_characters_and_backslash(self):
        branch = "fix/\\" + "".join(chr(i) for i in range(1, 32)) + "tail"
        self.assertEqual(self.resolve("branch", branch)["branch"], branch)

    def test_override_preserves_quoted_tag(self):
        tag = 'snapshot/quote"tag'
        subprocess.run(["git", "check-ref-format", "refs/tags/" + tag], check=True)
        data = self.resolve("tag", tag, "xt-v1.2.3")
        self.assertEqual(data["tag"], tag)
        self.assertIsNone(data["branch"])

    def test_plain_branch_and_null_tag(self):
        data = self.resolve("branch", "main")
        self.assertEqual(data["branch"], "main")
        self.assertIsNone(data["tag"])
        self.assertEqual(data["hash"], "abcdef0")
        self.assertEqual(data["channel"], "dev")

    def test_stable_tag_keeps_null_branch(self):
        data = self.resolve("tag", "v1.2.3")
        self.assertEqual(data["version"], "xt-v1.2.3")
        self.assertEqual(data["tag"], "v1.2.3")
        self.assertIsNone(data["branch"])


if __name__ == "__main__":
    unittest.main()
