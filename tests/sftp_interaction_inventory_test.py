import unittest
from sftp_interaction_inventory import EXPECTED, verify


def log(names, status="ok"):
    return "\n".join(f"test {name} ... {status}" for name in names) + f"\ntest result: ok. {len(names)} passed; 0 failed; 0 ignored;\n"


class InventoryTests(unittest.TestCase):
    def setUp(self):
        self.names = sorted(EXPECTED)

    def test_all_required_and_existing_neighbor_pass(self):
        self.assertEqual(verify([log(self.names[:2]), log(self.names[2:] + ["existing::neighbor"])]), 43)

    def test_each_missing_test_is_rejected(self):
        for name in self.names:
            with self.subTest(name=name), self.assertRaises(ValueError):
                verify([log([n for n in self.names if n != name])])

    def test_duplicate_is_rejected(self):
        with self.assertRaises(ValueError):
            verify([log(self.names + self.names[:1])])

    def test_same_count_substitution_is_rejected(self):
        with self.assertRaises(ValueError):
            verify([log(self.names[:-1] + ["unknown::replacement"])])

    def test_ignored_or_failed_required_test_is_rejected(self):
        for status in ["ignored", "FAILED"]:
            with self.subTest(status=status), self.assertRaises(ValueError):
                verify([log(self.names[:-1]) + log(self.names[-1:], status)])

    def test_failure_summary_is_rejected_despite_individual_success(self):
        with self.assertRaises(ValueError):
            verify([log(self.names) + "test result: FAILED. 0 passed; 1 failed;\n"])

    def test_zero_tests_or_no_summary_is_rejected(self):
        for value in ["", log([]), "\n".join(f"test {n} ... ok" for n in self.names)]:
            with self.subTest(value=value[:50]), self.assertRaises(ValueError):
                verify([value])

    def test_duplicate_across_process_logs_is_rejected(self):
        with self.assertRaises(ValueError):
            verify([log(self.names), log(self.names[:1])])


if __name__ == "__main__":
    unittest.main()
