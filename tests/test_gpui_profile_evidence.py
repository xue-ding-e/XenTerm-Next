"""Synthetic profile evidence contracts; does not run Windows or MSI."""
from contextlib import closing, contextmanager
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('profile_evidence', ROOT / 'scripts/gpui_msi_profile_evidence.py')
evidence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evidence)


class ProfileEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / 'profile'
        self.root.mkdir()
        self.baseline = Path(self.temp.name) / 'before.json'
        (self.root / 'other-setting').write_bytes(b'original')

    def test_unchanged_profile(self):
        evidence.capture(self.root, self.baseline)
        evidence.verify(self.root, self.baseline)

    def test_capture_cannot_overwrite_original(self):
        evidence.capture(self.root, self.baseline)
        before = self.baseline.read_bytes()
        (self.root / 'other-setting').write_bytes(b'corrupted')
        with self.assertRaises(FileExistsError):
            evidence.capture(self.root, self.baseline)
        self.assertEqual(self.baseline.read_bytes(), before)
        with self.assertRaises(ValueError):
            evidence.verify(self.root, self.baseline)

    def test_detects_non_sentinel_change_deletion_and_addition(self):
        evidence.capture(self.root, self.baseline)
        path = self.root / 'other-setting'
        path.write_bytes(b'wrong')
        with self.assertRaises(ValueError):
            evidence.verify(self.root, self.baseline)
        path.unlink()
        with self.assertRaises(ValueError):
            evidence.verify(self.root, self.baseline)
        path.write_bytes(b'original')
        (self.root / 'unexpected').write_bytes(b'new')
        with self.assertRaises(ValueError):
            evidence.verify(self.root, self.baseline)

    def test_rejects_empty_and_different_root(self):
        evidence.capture(self.root, self.baseline)
        other = Path(self.temp.name) / 'other'
        other.mkdir()
        with self.assertRaises(ValueError):
            evidence.capture(other, Path(self.temp.name) / 'empty.json')
        (other / 'other-setting').write_bytes(b'original')
        with self.assertRaises(ValueError):
            evidence.verify(other, self.baseline)

    @unittest.skipUnless(hasattr(os, 'symlink'), 'symlinks unavailable')
    def test_rejects_link_in_profile(self):
        (self.root / 'link').symlink_to(self.root / 'other-setting')
        with self.assertRaises(ValueError):
            evidence.capture(self.root, self.baseline)

    @contextmanager
    def fixture_database(self):
        # SQLite's transaction context commits/rolls back, but does not close.
        # Explicit ownership releases the Windows file handle before cleanup.
        with closing(sqlite3.connect(self.root / 'sessions.db')) as db, db:
            yield db

    def test_fixture_database_closes_on_success_and_error(self):
        for fail in (False, True):
            with self.subTest(fail=fail):
                try:
                    with self.fixture_database() as db:
                        db.execute('SELECT 1')
                        if fail:
                            raise ValueError('synthetic transaction failure')
                except ValueError:
                    self.assertTrue(fail)
                with self.assertRaises(sqlite3.ProgrammingError):
                    db.execute('SELECT 1')

    def make_database(self):
        with self.fixture_database() as db:
            db.executescript('CREATE TABLE sessions(ordinal INTEGER, id TEXT PRIMARY KEY, data TEXT);'
                             'CREATE TABLE command_history(seq INTEGER PRIMARY KEY, command TEXT);'
                             'CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);')
            db.execute('INSERT INTO meta VALUES(?,?)', ('settings', json.dumps({'theme': 'system'})))

    def test_synthetic_seed_and_semantic_retention(self):
        self.make_database()
        evidence.seed(self.root, 'fixture')
        value = evidence.semantic(self.root, 'fixture')
        self.assertEqual(value['session']['host'], 'installer-test.invalid')
        self.assertEqual(value['history'], ['echo synthetic-installer-fixture'])
        self.assertEqual(set(value['policy'].values()), {False})
        evidence.write_new(self.baseline, value)
        evidence.verify_semantic(self.root, self.baseline, 'fixture', Path(self.temp.name) / 'after.json')
        with self.assertRaises(ValueError):
            evidence.seed(self.root, 'second')

    def test_semantic_corruption_is_not_hidden_by_fresh_snapshot(self):
        self.make_database()
        evidence.seed(self.root, 'fixture')
        evidence.write_new(self.baseline, evidence.semantic(self.root, 'fixture'))
        with self.fixture_database() as db:
            db.execute('DELETE FROM command_history')
        with self.assertRaises(ValueError):
            evidence.verify_semantic(self.root, self.baseline, 'fixture', Path(self.temp.name) / 'after.json')

    def test_missing_database_does_not_get_created(self):
        with self.assertRaises(sqlite3.OperationalError):
            evidence.semantic(self.root, 'fixture')
        with self.assertRaises(sqlite3.OperationalError):
            evidence.seed(self.root, 'fixture')
        self.assertFalse((self.root / 'sessions.db').exists())

    def test_native_initialization_is_inspection_not_schema_creation(self):
        evidence.assert_native_profile(self.root, False)
        with self.assertRaises(ValueError):
            evidence.assert_native_profile(self.root, True)
        self.assertFalse((self.root / 'sessions.db').exists())
        # Synthetic schema here tests only the inspector; it is not native acceptance.
        self.make_database()
        before = (self.root / 'sessions.db').read_bytes()
        with self.assertRaises(ValueError):
            evidence.assert_native_profile(self.root, True)
        self.assertEqual(before, (self.root / 'sessions.db').read_bytes())
        with self.fixture_database() as db:
            db.execute("UPDATE meta SET value=? WHERE key='settings'", (json.dumps({'mcp_access_mode': 'restricted'}),))
        evidence.assert_native_profile(self.root, True)
        evidence.seed(self.root, 'fixture')
        for required in (False, True):
            with self.assertRaises(ValueError):
                evidence.assert_native_profile(self.root, required)

    def test_native_schema_must_be_complete(self):
        self.make_database()
        with self.fixture_database() as db:
            db.execute("UPDATE meta SET value=? WHERE key='settings'", (json.dumps({'mcp_access_mode': 'restricted'}),))
            db.execute('DROP TABLE command_history')
        with self.assertRaises(sqlite3.OperationalError):
            evidence.assert_native_profile(self.root, True)

    def test_msi_phase_order_and_scope(self):
        script = (ROOT / 'scripts/test_next_gpui_msi_e2e.ps1').read_text()
        self.assertNotIn('$script:snapshot', script)
        self.assertEqual(script.count("@('capture','--profile',$profile,'--output',$upgradeBaseline)"), 1)
        self.assertEqual(script.count("@('verify','--profile',$profile,'--baseline',$upgradeBaseline)"), 5)
        initial = script.index('Initialize-GpuiTestProfile -Exe')
        self.assertLess(script.index('Assert-Installed $directory $oldCode $true'), initial)
        self.assertLess(script.index("@('assert-empty-or-missing'"), initial)
        self.assertLess(initial, script.index("@('assert-native-initialized'"))
        self.assertLess(initial, script.index("@('seed'"))
        freeze = script.index('$upgradeBaseline =')
        launch = script.index('Assert-Installed $directory $newCode $true')
        self.assertGreater(launch, script.index('"repair-current-$label"'))
        self.assertGreater(script.index('$uninstallBaseline ='), launch)
        self.assertNotIn('$oldCode $true', script[freeze:launch])
        self.assertIn('Assert-ProtectedProductSentinels $Label', script)
        sentinel = (ROOT / 'scripts/gpui_msi_coexistence_sentinels.ps1').read_text()
        for product in ('upstream', 'portable', 'flutter'):
            self.assertIn("name='" + product + "'", sentinel)
        self.assertIn('real_other_product_gui_tested=$false', sentinel)
        self.assertIn('MainWindowHandle', sentinel)
        self.assertNotIn('Stop-Process -Name', sentinel)


if __name__ == '__main__':
    unittest.main()
