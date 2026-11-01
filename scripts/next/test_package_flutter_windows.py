import hashlib
import json
import importlib.util
from pathlib import Path
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location('pack', Path(__file__).with_name('package_flutter_windows.py'))
pack = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pack)


class PackageTests(unittest.TestCase):
    def test_complete_bundle_excludes_profiles_and_preserves_dependencies(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bundle = root / 'bundle'
            for name in (*pack.REQUIRED, 'plugin.dll', 'config/sessions.db',
                         'config-flutter-next/secret.key', 'log/log', 'debug.pdb'):
                path = bundle / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b'fixture')
            info = {'core_version': 'fixture-version', 'source_sha': 'a' * 40,
                    'minimum_vc_runtime': '14.44.35211.0', 'runtime_mode': 'external-prerequisite',
                    'clean_windows_tested': False,
                    'binary_sha256': {n: hashlib.sha256((bundle / n).read_bytes()).hexdigest()
                                      for n in ('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll')}}
            (bundle / 'WINDOWS-RUNTIME.json').write_text(json.dumps(info))
            for name in ('scripts/next/Start-Flutter-Preview.ps1', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'assets/fonts/LICENSE-test'):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b'fixture license')
            archive = pack.package(root, bundle, root / 'dist', 'a' * 40, 'fixture-version')
            with zipfile.ZipFile(archive) as z:
                names = z.namelist()
                self.assertTrue(any(n.endswith('/plugin.dll') for n in names))
                self.assertTrue(any(n.endswith('/licenses/LICENSE') for n in names))
                self.assertFalse(any('/config/' in n or '/config-flutter-next/' in n or '/log/' in n or n.endswith('.pdb') for n in names))
            with self.assertRaisesRegex(ValueError, 'source/version'):
                pack.package(root, bundle, root / 'bad', 'a' * 40, 'wrong')
            (bundle / 'xenterm-cli.exe').write_bytes(b'tampered')
            with self.assertRaisesRegex(ValueError, 'provenance'):
                pack.package(root, bundle, root / 'bad', 'a' * 40, 'fixture-version')
            (bundle / 'xenterm-cli.exe').write_bytes(b'fixture')
            with self.assertRaises(FileExistsError):
                pack.package(root, bundle, root / 'dist', 'a' * 40, 'fixture-version')

    def test_missing_runtime_is_not_a_successful_package(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ValueError, 'runtime file'):
                pack.validate_bundle(Path(tmp))

    def test_invalid_source_identity_rejected(self):
        with self.assertRaisesRegex(ValueError, 'source commit'):
            pack.package(Path('.'), Path('.'), Path('.'), 'main', 'version')


if __name__ == '__main__':
    unittest.main()
