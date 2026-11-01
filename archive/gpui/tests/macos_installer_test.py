"""Run: python3 -m unittest discover -s tests -p 'macos_installer_test.py'."""
import importlib.util
import pathlib
import plistlib
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parents[1]
MAC = ROOT / "packaging/macos"
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("macos_prepare", MAC / "prepare.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class MacInstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="xenterm package test ")
        self.root = pathlib.Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)
        self.app = self.root / "xenterm.app"
        binary = self.app / "Contents/MacOS/xenterm"
        binary.parent.mkdir(parents=True)
        binary.write_text("#!/bin/sh\necho fixture\n")
        binary.chmod(0o755)
        with (self.app / "Contents/Info.plist").open("wb") as stream:
            plistlib.dump({"CFBundleIdentifier": "dev.xenterm.xenterm", "CFBundleExecutable": "xenterm", "CFBundleVersion": "0.1.0"}, stream)
        self.link = self.root / "usr/local/bin/xenterm"

    def install_cli(self):
        return subprocess.run(["sh", str(MAC / "cli-scripts/postinstall"), "fixture.pkg", "/", str(self.root)], capture_output=True, text=True)

    def test_stage_preserves_binary_and_default_choices(self):
        out = self.root / "staged package"
        module.prepare(self.app, "0.1.0", out)
        self.assertTrue((out / "payload/Applications/xenterm.app/Contents/MacOS/xenterm").stat().st_mode & 0o111)
        xml = ET.parse(out / "distribution.xml")
        choices = {c.attrib["id"]: c.attrib for c in xml.findall("choice")}
        self.assertEqual(choices["app"]["enabled"], "false")
        self.assertEqual(choices["cli"]["start_selected"], "true")
        self.assertNotIn("selected", choices["cli"])
        self.assertEqual(xml.find("options").attrib["customize"], "always")
        self.assertFalse(plistlib.loads((out / "components.plist").read_bytes())[0]["BundleIsRelocatable"])

    def test_rejects_version_injection(self):
        with self.assertRaises(ValueError): module.prepare(self.app, '1"<bad', self.root / "out")

    def test_rejects_wrong_bundle(self):
        (self.app / "Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "other"}))
        with self.assertRaises(ValueError): module.prepare(self.app, "1", self.root / "out")

    def test_rejects_non_executable(self):
        (self.app / "Contents/MacOS/xenterm").chmod(0o644)
        with self.assertRaises(ValueError): module.prepare(self.app, "1", self.root / "out")

    def test_cli_install_and_repeated_upgrade(self):
        self.assertEqual(self.install_cli().returncode, 0)
        self.assertEqual(self.install_cli().returncode, 0)
        self.assertEqual(str(self.link.readlink()), "/Applications/xenterm.app/Contents/MacOS/xenterm")

    def test_cli_foreign_file_untouched(self):
        self.link.parent.mkdir(parents=True)
        self.link.write_text("not ours")
        self.assertNotEqual(self.install_cli().returncode, 0)
        self.assertEqual(self.link.read_text(), "not ours")

    def test_cli_foreign_symlink_untouched(self):
        self.link.parent.mkdir(parents=True)
        self.link.symlink_to("/foreign-command")
        self.assertNotEqual(self.install_cli().returncode, 0)
        self.assertEqual(str(self.link.readlink()), "/foreign-command")

    def test_cli_redirected_parent_rejected(self):
        foreign = self.root / "foreign"
        foreign.mkdir()
        (self.root / "usr").symlink_to(foreign)
        self.assertNotEqual(self.install_cli().returncode, 0)
        self.assertEqual(list(foreign.iterdir()), [])

    def test_existing_stage_rejected(self):
        with self.assertRaises(FileExistsError): module.prepare(self.app, "1", self.root)

    def test_no_global_shell_profile_or_desktop_writes(self):
        script = (MAC / "cli-scripts/postinstall").read_text()
        for forbidden in (".zshrc", ".bashrc", "/Desktop", "defaults write", "ln -sf"):
            self.assertNotIn(forbidden, script)

if __name__ == "__main__": unittest.main()
