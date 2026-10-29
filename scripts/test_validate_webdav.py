"""Synthetic control guards only; never compiles or claims product coverage."""
import importlib.util
from pathlib import Path
import unittest
import tempfile
import sys
import os
import time
from unittest.mock import patch
spec = importlib.util.spec_from_file_location('driver',Path(__file__).with_name('validate-webdav.py'))
d = importlib.util.module_from_spec(spec)
spec.loader.exec_module(d)

class Guards(unittest.TestCase):
    def test_patch_identity(self):
        for name,digest in d.PATCHES.items(): self.assertEqual(d.b.digest(d.HERE/name),digest)
    def test_exact_inventory(self):
        for name in d.NAMES:
            d.b.parse_inventory(name+': test\n',name)
            for bad in ['',name+': test\n'+name+': test\n',name+': benchmark\n','wrong: test\n']:
                with self.assertRaises(AssertionError): d.b.parse_inventory(bad,name)
    def test_result_rejects_missing_ignored_wrong(self):
        for name in d.NAMES:
            good = 'running 1 test\ntest '+name+' ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.12s\n'
            d.b.parse_result(good,name)
            for bad in ['',good+good,good.replace('1 passed','0 passed'),good.replace(' ... ok',' ... ignored'),good.replace(name,'wrong')]:
                with self.assertRaises(AssertionError): d.b.parse_result(bad,name)
    def test_binary_identity(self):
        d.b.verify_hash('a','a')
        with self.assertRaises(AssertionError): d.b.verify_hash('a','b')
    def test_source_and_feature_boundaries(self):
        self.assertEqual(d.SHA,'79ef3363f0f600f5674ee2c245b5eb357f855961')
        self.assertEqual(len(set(d.NAMES)),2)
        source = (d.HERE/'validate-webdav.py').read_text()
        self.assertIn("baseline = desktop('original-lock')",source)
        self.assertIn("upgraded = desktop('upgraded-lock')",source)
        self.assertIn("'+1.88.0','test','--locked','--no-default-features','--features','headless'",source)
    def test_list_to_execute_hash_barrier_and_final_identity(self):
        source = (d.HERE/'validate-webdav.py').read_text()
        listing = source.index('b.parse_inventory(b.run')
        barrier = source.index('b.verify_hash(b.digest(binary),digest)',listing)
        execute = source.index('b.parse_result(b.run',listing)
        self.assertLess(barrier,execute)
        for marker in ['final-source-head','final-control-head','final-control-clean','upgraded-status-after']:
            self.assertIn(marker,source)
    def test_owner_retains_generation_until_final_reap(self):
        owner = d.b.owner
        events = []
        class Child:
            pid = 123
            def wait(self,timeout): events.append('reap'); return 0
        def members(*args):
            self.assertNotIn('reap',events)
            events.append('scan')
            return {}
        with patch.object(owner,'owned_members',members), patch.object(owner,'observe_exit',lambda child: 0):
            self.assertEqual(owner.cleanup(Child(),{}),{})
        self.assertEqual(events[-1],'reap')
        source = (d.HERE/'owned-supervisor.py').read_text()
        self.assertIn('os.WNOWAIT',source)
        self.assertIn('signal.pidfd_send_signal',source)
        self.assertNotIn('os.killpg',source)
    def test_owner_real_success_timeout_orphan(self):
        owner = d.b.owner
        cases = [('success',"print('ok')",3,'completed'),
                 ('timeout','import time; time.sleep(5)',0.15,'timeout'),
                 ('orphan','import os,time; p=os.fork(); time.sleep(5) if p == 0 else None',2,'surviving_descendants_after_parent_exit')]
        with tempfile.TemporaryDirectory() as tmp:
            owner.EVIDENCE = Path(tmp)
            owner.PHASE = 'synthetic'
            owner.RECORDS = []
            owner.TOTAL_LOG_BYTES = 0
            owner.JOB_DEADLINE = time.monotonic()+30
            owner.check_disk_budgets = lambda: None  # no product work or large files
            for label,code,seconds,reason in cases:
                if reason == 'completed':
                    self.assertEqual(owner.run(label,[sys.executable,'-c',code],seconds,tmp,os.environ.copy()),b'ok\n')
                else:
                    with self.assertRaises(RuntimeError): owner.run(label,[sys.executable,'-c',code],seconds,tmp,os.environ.copy())
                record = owner.RECORDS[-1]
                self.assertEqual(record['reason'],reason)
                self.assertFalse(record['survivors'])
                self.assertIsNone(owner.ACTIVE)
if __name__ == '__main__': unittest.main()
