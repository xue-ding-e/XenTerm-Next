"""Runner contract tests; no Rust toolchain or network required."""
import importlib.util
from pathlib import Path
import subprocess
import unittest

SPEC = importlib.util.spec_from_file_location(
    "russh_mac_runner", Path(__file__).with_name("run_russh_mac_tests.py"))
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
ROOT = Path(__file__).resolve().parents[2]


class MacRunnerTests(unittest.TestCase):
    def simulate(self, output=None, code=0, mutate=False):
        original = (ROOT / "vendor/russh-0.61.2/Cargo.lock").read_bytes()
        def fake(command, **kwargs):
            self.assertIn("--locked", command)
            self.assertEqual(command[command.index("--features") + 1], "ring,rsa,flate2")
            self.assertIn("--no-default-features", command)
            self.assertNotIn("-p", command)
            crate = kwargs["cwd"]
            self.assertNotEqual(crate, ROOT / "vendor/russh-0.61.2")
            self.assertIn("[workspace]", (crate / "Cargo.toml").read_text())
            self.assertEqual((crate / "Cargo.lock").read_bytes(), original)
            if mutate:
                (crate / "Cargo.lock").write_text("changed")
            return subprocess.CompletedProcess(command, code, output if output is not None else
                "\n".join(f"test {name} ... ok" for name in gate.TESTS) +
                "\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out")
        try:
            gate.run_tests(ROOT, fake)
        finally:
            self.assertEqual((ROOT / "vendor/russh-0.61.2/Cargo.lock").read_bytes(), original)

    def test_isolated_standalone_locked_graph(self):
        self.simulate()

    def test_zero_or_ignored_tests_rejected(self):
        for output in ("running 0 tests", "\n".join(f"test {name} ... ignored" for name in gate.TESTS)):
            with self.subTest(output=output), self.assertRaises(RuntimeError):
                self.simulate(output=output)

    def test_missing_test_rejected(self):
        with self.assertRaises(RuntimeError):
            self.simulate(output=f"test {gate.TESTS[0]} ... ok")

    def test_cargo_failure_rejected(self):
        with self.assertRaises(RuntimeError):
            self.simulate(code=1)

    def test_lock_mutation_rejected(self):
        with self.assertRaises(RuntimeError):
            self.simulate(mutate=True)


if __name__ == "__main__":
    unittest.main()
