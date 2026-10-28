import importlib.util
from pathlib import Path
import unittest
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
if __name__ == '__main__': unittest.main()
