"""Round-trip and negative checks for the release artifact admission gate."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))

spec = importlib.util.spec_from_file_location('verify', Path(__file__).resolve().parents[1] / 'scripts/headless_server_verify.py')
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


class PackageAdmission(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.source = 'a' * 40
        self.tooling = 'b' * 40
        self.stem = f'xenterm-next-{self.source[:12]}-headless-linux-x86_64'
        self.archive = self.stem + '.tar.gz'
        self.manifest = dict(repository='xue-ding-e/XenTerm-Next', source_sha=self.source,
                             tooling_sha=self.tooling, source_ci_run='123', build_run='456',
                             platform='linux-x86_64-headless', runner='ubuntu-22.04',
                             features='--no-default-features --features headless', maximum_glibc='2.34',
                             version_scheme='xt-v1', runtime_version='xt-beta-aaaaaaa-T1792060000',
                             package_version='0.7.3', version_timestamp=1792060000,
                             binary_version='xenterm xt-beta-aaaaaaa-T1792060000',
                             needed=['libc.so.6'], binary_sha256=hashlib.sha256(b'binary').hexdigest())

    def write(self, extra=None, executable=True):
        raw = json.dumps(self.manifest).encode()
        (self.root / 'BUILD.json').write_bytes(raw)
        files = {'BUILD.json': raw, 'xenterm': b'binary', 'LICENSE': b'MIT',
                 'THIRD_PARTY_NOTICES.md': b'notices', 'RUNTIME.txt': b'libc'}
        with tarfile.open(self.root / self.archive, 'w:gz') as t:
            for name, content in files.items():
                item = tarfile.TarInfo(self.stem + '/' + name)
                item.size = len(content)
                item.mode = 0o755 if name == 'xenterm' and executable else 0o644
                t.addfile(item, io.BytesIO(content))
            if extra:
                item = tarfile.TarInfo(extra)
                t.addfile(item, io.BytesIO())
        checksum = hashlib.sha256((self.root / self.archive).read_bytes()).hexdigest()
        (self.root / 'SHA256SUMS').write_text(checksum + '  ' + self.archive + '\n')

    def admit(self):
        return verify.verify(self.root, self.source, self.tooling, '123', '456')

    def test_round_trip(self):
        self.write()
        self.assertEqual(self.admit(), self.manifest)

    def test_legacy_runtime_round_trip(self):
        self.manifest.update(version_scheme='cargo-legacy', runtime_version='0.7.3',
                             version_timestamp=None, binary_version='xenterm 0.7.3')
        self.write()
        self.assertEqual(self.admit(), self.manifest)

    def test_wrong_runtime_hash_or_binary_version(self):
        for key, value in [('runtime_version', 'xt-beta-bbbbbbb-T1792060000'),
                           ('binary_version', 'xenterm 0.7.3'),
                           ('version_timestamp', 1792060001),
                           ('version_scheme', 'unknown')]:
            with self.subTest(key=key):
                before = self.manifest[key]
                self.manifest[key] = value
                self.write()
                with self.assertRaises(ValueError):
                    self.admit()
                self.manifest[key] = before

    def test_wrong_source_or_run(self):
        for key in ('source_sha', 'tooling_sha', 'source_ci_run', 'build_run'):
            with self.subTest(key=key):
                before = self.manifest[key]
                self.manifest[key] = 'wrong'
                self.write()
                with self.assertRaises(ValueError):
                    self.admit()
                self.manifest[key] = before

    def test_archive_corruption(self):
        self.write()
        with (self.root / self.archive).open('ab') as f:
            f.write(b'changed')
        with self.assertRaises(ValueError):
            self.admit()

    def test_extra_file(self):
        self.write()
        (self.root / 'extra').write_text('unexpected')
        with self.assertRaises(ValueError):
            self.admit()

    def test_binary_hash_mismatch(self):
        self.manifest['binary_sha256'] = '0' * 64
        self.write()
        with self.assertRaises(ValueError):
            self.admit()

    def test_new_abi(self):
        self.manifest['maximum_glibc'] = '2.39'
        self.write()
        with self.assertRaises(ValueError):
            self.admit()

    def test_gui_dependency(self):
        self.manifest['needed'] = ['libgtk-3.so.0']
        self.write()
        with self.assertRaises(ValueError):
            self.admit()

    def test_path_escape(self):
        self.write(extra=self.stem + '/../outside')
        with self.assertRaises(ValueError):
            self.admit()

    def test_duplicate_member(self):
        self.write(extra=self.stem + '/BUILD.json')
        with self.assertRaises(ValueError):
            self.admit()

    def test_alias_members_rejected(self):
        for alias in ('./xenterm', '/xenterm', './BUILD.json'):
            with self.subTest(alias=alias):
                self.write(extra=self.stem + '/' + alias)
                with self.assertRaises(ValueError):
                    self.admit()

    def test_missing_exec_mode(self):
        self.write(executable=False)
        with self.assertRaises(ValueError):
            self.admit()


if __name__ == '__main__':
    unittest.main()
