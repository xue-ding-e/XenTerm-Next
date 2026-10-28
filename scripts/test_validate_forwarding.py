import importlib.util
from pathlib import Path
import unittest
import tempfile
import json
import os
spec = importlib.util.spec_from_file_location('driver', Path(__file__).with_name('validate-forwarding.py'))
d = importlib.util.module_from_spec(spec)
spec.loader.exec_module(d)

class Gates(unittest.TestCase):
    def test_exact_inventory(self):
        for name in d.EXPECTED:
            d.parse_inventory(name + ': test\n', name)
            for text in ['', name+': test\n'+name+': test\n', name+': benchmark\n', 'other: test\n']:
                with self.assertRaises(AssertionError): d.parse_inventory(text, name)
    def test_exact_results(self):
        for name in d.EXPECTED:
            good = 'running 1 test\ntest '+name+' ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.12s\n'
            d.parse_result(good, name)
            for bad in ['',good.replace('1 passed','0 passed'),good.replace(' ... ok',' ... ignored'),good+good,good.replace(name,'wrong')]:
                with self.assertRaises(AssertionError): d.parse_result(bad,name)
    def test_binary_hash(self):
        d.verify_hash('a','a')
        with self.assertRaises(AssertionError): d.verify_hash('a','b')
    def test_patch_hash(self):
        self.assertEqual(d.digest(Path(__file__).with_name('forwarding.patch')),d.PATCH_HASH)
    def test_version_output_guards(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = d.version_preflight(root)
            data = dict(version='xt-dev-'+d.SHA[:7]+'-T1793220000', channel='dev', semver=None,
                        hash=d.SHA[:7], timestamp=1793220000, source='branch', branch='validation/forwarding-slots-20261028',tag=None,generated_at='2026-10-28T21:00:00Z')
            path.write_text(json.dumps(data))
            self.assertEqual(d.version_receipt(path)['content']['hash'],d.SHA[:7])
            with self.assertRaises(AssertionError): d.version_preflight(root)
            data['hash']='e9232fd'
            path.write_text(json.dumps(data))
            with self.assertRaises(AssertionError): d.version_receipt(path)
            path.write_text('{')
            with self.assertRaises(json.JSONDecodeError): d.version_receipt(path)
            path.unlink()
            path.symlink_to(root/'missing')
            with self.assertRaises(AssertionError): d.version_preflight(root)
            with self.assertRaises(AssertionError): d.version_receipt(path)
            path.unlink()
            link = root/'linked'
            link.symlink_to(root, target_is_directory=True)
            with self.assertRaises(AssertionError): d.version_preflight(link)
if __name__ == '__main__': unittest.main()
