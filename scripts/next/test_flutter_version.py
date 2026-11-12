"""Exercise the real Bash version entry point for repeatable Flutter builds."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class FlutterVersionTests(unittest.TestCase):
    def resolve(self, root, **overrides):
        script = root / 'scripts/version.sh'
        script.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(Path(__file__).resolve().parents[1] / 'version.sh', script)
        cargo = root / 'crates' / 'xenterm' / 'Cargo.toml'
        cargo.parent.mkdir(parents=True, exist_ok=True)
        cargo.write_text('[package]\nname = "xenterm"\nversion = "0.7.3"\n', encoding='utf-8')
        pubspec = root / 'apps/xenterm/pubspec.yaml'
        pubspec.parent.mkdir(parents=True, exist_ok=True)
        pubspec.write_text('version: 0.7.3+1\n', encoding='utf-8')
        env = {k: v for k, v in os.environ.items()
               if k not in ('XT_VERSION', 'XT_VERSION_JSON', 'XT_CHANNEL', 'GITHUB_REF_TYPE',
                            'GITHUB_REF_NAME', 'GITHUB_SHA', 'SOURCE_DATE_EPOCH')}
        env.update(XT_CHANNEL='beta', GITHUB_REF_TYPE='branch', GITHUB_REF_NAME='flutter-next',
                   GITHUB_SHA='1234567' + 'a' * 33, SOURCE_DATE_EPOCH='1794480000')
        env.update(overrides)
        if os.name == 'nt':
            git = Path(shutil.which('git'))
            bash = env.get('XT_BASH') or str(git.parent.parent / 'bin/bash.exe')
        else:
            bash = 'bash'
        result = subprocess.run([bash, str(script)], cwd=root, env=env,
                                capture_output=True, text=True, encoding='utf-8', timeout=20)
        return result

    def test_same_source_and_epoch_resolve_once_for_all_platforms_and_retries(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            first = self.resolve(root)
            self.assertEqual(first.returncode, 0, first.stderr)
            self.assertEqual(first.stdout.strip(), 'xt-beta-1234567-T1794480000')
            metadata = json.loads((root / 'version.json').read_text())
            self.assertEqual(metadata['timestamp'], 1794480000)
            self.assertEqual(metadata['hash'], '1234567')
            self.assertEqual(self.resolve(root).stdout, first.stdout)

    def test_flutter_rolling_tag_is_a_beta_channel_and_invalid_epochs_fail(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            tagged = self.resolve(root, GITHUB_REF_TYPE='tag', GITHUB_REF_NAME='flutter-beta')
            self.assertEqual(tagged.returncode, 0, tagged.stderr)
            self.assertEqual(tagged.stdout.strip(), 'xt-beta-1234567-T1794480000')
            for epoch in ('1794480000x', '-1', '123'):
                with self.subTest(epoch=epoch):
                    result = self.resolve(root, SOURCE_DATE_EPOCH=epoch)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(result.stdout.strip(), '')


if __name__ == '__main__':
    unittest.main()
