"""Reject empty, duplicated, unrelated and incomplete test enumeration."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('gate', Path(__file__).resolve().parents[1] / 'scripts/gpui_candidate_validation.py')
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class EnumerationGuard(unittest.TestCase):
    def test_expected_unique_test_names(self):
        self.assertEqual(gate.check_listing('example::a: test\nexample::b: test\n2 tests, 0 benchmarks\n', 'example::', 2), ['example::a', 'example::b'])

    def test_reject_zero_filtered_tests(self):
        with self.assertRaises(RuntimeError):
            gate.check_listing('0 tests, 0 benchmarks\n', 'example::', 2)

    def test_reject_missing_test(self):
        with self.assertRaises(RuntimeError):
            gate.check_listing('example::a: test\n', 'example::', 2)

    def test_reject_duplicate_or_foreign_test(self):
        for listing in ['example::a: test\nexample::a: test\n', 'example::a: test\nforeign::b: test\n']:
            with self.assertRaises(RuntimeError):
                gate.check_listing(listing, 'example::', 2)

    def test_expected_scope_is_27_tests(self):
        self.assertEqual(sum(count for _, count in gate.GATES), 27)
        self.assertEqual(len(gate.SOURCE), 40)


if __name__ == '__main__':
    unittest.main()
