#!/usr/bin/env python3
"""Isolated installer fixtures; run with python3 tests/install_linux_test.py.

Only a fake binary is installed, and every install destination is checked to be
inside TemporaryDirectory. sudo and cache tools are stubs; no real sudo, app,
cache update, or system installation is performed. Root behavior is simulated
by stubbing id, so the suite itself needs no elevated privileges.
"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest


REPO = Path(__file__).resolve().parents[1]
BASH = shutil.which("bash")
INSTALL = shutil.which("install")
VALIDATOR = shutil.which("desktop-file-validate")

# Captured verbatim from the installer's pre-prefix-support desktop heredoc.
LEGACY_DESKTOP = """[Desktop Entry]
Type=Application
Name=xenterm
GenericName=SSH Client
Comment=Lightweight Rust + GPUI SSH/SFTP client
Comment[zh_CN]=轻量级 Rust + GPUI SSH/SFTP 客户端
Exec=xenterm
Icon=xenterm
Terminal=false
Categories=Network;TerminalEmulator;
Keywords=ssh;sftp;terminal;shell;
StartupNotify=true
StartupWMClass=xenterm
Actions=new-window;

[Desktop Action new-window]
Name=New Window
Name[zh_CN]=新建窗口
Exec=xenterm --new-window
"""


class InstallerTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="xenterm-installer-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.package = self.root / "release package"
        self.package.mkdir()
        self.script = self.package / "install-linux.sh"
        shutil.copyfile(REPO / "assets/install-linux.sh", self.script)
        self.binary = self.package / "xenterm"
        self.binary.write_text("#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\" > \"$XENTERM_FIXTURE_LAUNCH_LOG\"\n")
        # Source permissions must remain untouched, even for an unpacked binary.
        self.binary.chmod(0o644)
        (self.package / "icon@512.png").write_bytes(b"fixture icon")
        self.home = self.root / "home"
        self.home.mkdir()
        self.prefix = self.root / "fresh prefix" / "nested"
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.log = self.root / "commands.jsonl"
        for command in ("cat", "dirname", "readlink", "mktemp", "rm", "grep", "cmp"):
            (self.tools / command).symlink_to(shutil.which(command))
        self.make_tool("id", "print(os.environ.get('FIXTURE_UID', '1000'))")
        self.make_tool("install", f"""
args = sys.argv[1:]
paths = args[args.index('--') + 1:]
outputs = paths if '-d' in args else paths[-1:]
root = Path(os.environ['FIXTURE_ROOT']).resolve()
for output in outputs:
    assert Path(output).resolve().is_relative_to(root), output
with open(os.environ['FIXTURE_LOG'], 'a') as log:
    log.write(json.dumps(['install'] + args) + chr(10))
os.execv({INSTALL!r}, [{INSTALL!r}] + args)
""")
        self.make_tool("sudo", """
with open(os.environ['FIXTURE_LOG'], 'a') as log:
    log.write(json.dumps(['sudo'] + sys.argv[1:]) + chr(10))
if os.environ.get('FIXTURE_SUDO') != 'allow':
    sys.exit('unexpected sudo invocation')
# Simulate privilege acquisition solely by unlocking this fixture directory.
if os.environ.get('FIXTURE_UNLOCK'):
    path = Path(os.environ['FIXTURE_UNLOCK'])
    assert path.resolve().is_relative_to(Path(os.environ['FIXTURE_ROOT']))
    path.chmod(0o755)
if sys.argv[1:] != ['-v']:
    os.execvp(sys.argv[1], sys.argv[1:])
""")
        for command in ("update-desktop-database", "gtk-update-icon-cache"):
            self.make_tool(command, f"""
with open(os.environ['FIXTURE_LOG'], 'a') as log:
    log.write(json.dumps([{command!r}] + sys.argv[1:]) + chr(10))
sys.exit(int(os.environ.get('FIXTURE_CACHE_EXIT', '0')))
""")
        self.env = dict(os.environ)
        for key in ("PREFIX", "XDG_DATA_HOME", "BASH_ENV", "ENV"):
            self.env.pop(key, None)
        self.env.update(
            HOME=str(self.home), PATH=str(self.tools), TMPDIR=str(self.root),
            FIXTURE_ROOT=str(self.root), FIXTURE_LOG=str(self.log),
            FIXTURE_UID="1000", FIXTURE_SUDO="deny",
        )

    def make_tool(self, name, code):
        path = self.tools / name
        path.write_text(
            f"#!{sys.executable}\nimport json, os, sys\nfrom pathlib import Path\n"
            + textwrap.dedent(code).lstrip()
        )
        path.chmod(0o755)

    def run_installer(self, *args, success=True, env=None):
        result = subprocess.run(
            [BASH, str(self.script), *map(str, args)], cwd=self.root,
            env=self.env | (env or {}), text=True, capture_output=True,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def commands(self, name):
        rows = [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []
        return [row for row in rows if row[0] == name]

    def assert_installed(self, prefix):
        executable = prefix / "bin/xenterm"
        self.assertEqual(executable.read_bytes(), self.binary.read_bytes())
        self.assertEqual(executable.stat().st_mode & 0o777, 0o755)
        desktop = prefix / "share/applications/xenterm.desktop"
        self.assertEqual(desktop.stat().st_mode & 0o777, 0o644)
        self.assertTrue((prefix / "share/icons/hicolor/512x512/apps/xenterm.png").is_file())
        if VALIDATOR:
            result = subprocess.run([VALIDATOR, str(desktop)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return desktop

    def test_fresh_writable_prefix_needs_no_sudo(self):
        self.run_installer("--prefix", self.prefix)
        self.assert_installed(self.prefix)
        self.assertEqual(self.commands("sudo"), [])
        self.assertEqual(self.binary.stat().st_mode & 0o777, 0o644)

    def test_user_default_and_repeat_install(self):
        self.run_installer("--user")
        self.run_installer("--user")
        self.assert_installed(self.home / ".local")
        self.assertEqual(self.commands("sudo"), [])

    def test_environment_prefix_and_legacy_positional_binary(self):
        source = self.root / "alternate binary"
        source.write_bytes(self.binary.read_bytes())
        self.binary.unlink()
        self.run_installer(source, env={"PREFIX": str(self.prefix)})
        self.binary.write_bytes(source.read_bytes())
        self.assert_installed(self.prefix)

    def test_cli_prefix_wins_regardless_of_user_option_order(self):
        for args in (("--user", "--prefix", self.prefix), (f"--prefix={self.prefix}", "--user")):
            self.run_installer(*args, env={"PREFIX": str(self.root / "ignored")})
        self.assert_installed(self.prefix)
        self.assertFalse((self.root / "ignored").exists())

    def test_environment_prefix_overrides_user_default(self):
        self.run_installer("--user", env={"PREFIX": str(self.prefix)})
        self.assert_installed(self.prefix)

    def test_relative_prefix_and_dash_binary(self):
        source = self.root / "-binary"
        source.write_bytes(self.binary.read_bytes())
        self.run_installer("--prefix", "relative/nested", "--", "-binary")
        self.assert_installed(self.root / "relative/nested")

    def test_source_tree_fallback(self):
        self.binary.unlink()
        source = self.root / "target/release/xenterm"
        source.parent.mkdir(parents=True)
        source.write_text("fake source-tree binary")
        self.run_installer("--prefix", self.prefix)
        self.assertEqual((self.prefix / "bin/xenterm").read_text(), source.read_text())

    def test_root_does_not_require_sudo(self):
        (self.tools / "sudo").unlink()
        self.run_installer("--prefix", self.prefix, env={"FIXTURE_UID": "0"})
        self.assert_installed(self.prefix)

    def test_writable_prefix_does_not_require_sudo_installed(self):
        (self.tools / "sudo").unlink()
        self.run_installer("--prefix", self.prefix)
        self.assert_installed(self.prefix)

    def locked_prefix(self):
        if os.geteuid() == 0:
            self.skipTest("permission checks require a non-root fixture runner")
        self.prefix.mkdir(parents=True)
        self.prefix.chmod(0o555)
        self.addCleanup(self.prefix.chmod, 0o755)

    def test_unwritable_prefix_uses_stub_sudo(self):
        self.locked_prefix()
        self.run_installer("--prefix", self.prefix, env={
            "FIXTURE_SUDO": "allow", "FIXTURE_UNLOCK": str(self.prefix),
        })
        self.assert_installed(self.prefix)
        self.assertEqual(self.commands("sudo")[0], ["sudo", "-v"])
        self.assertTrue(any(row[1] == "install" for row in self.commands("sudo")))

    def test_user_mode_never_escalates(self):
        self.locked_prefix()
        result = self.run_installer("--user", "--prefix", self.prefix, success=False)
        self.assertIn("--user never uses sudo", result.stderr)
        self.assertEqual(self.commands("sudo"), [])
        self.assertEqual(self.commands("install"), [])

    def test_unwritable_prefix_without_sudo_has_actionable_error(self):
        self.locked_prefix()
        (self.tools / "sudo").unlink()
        result = self.run_installer("--prefix", self.prefix, success=False)
        self.assertIn("writable --prefix", result.stderr)
        self.assertEqual(self.commands("install"), [])

    def test_sudo_failure_does_not_install(self):
        self.locked_prefix()
        self.run_installer("--prefix", self.prefix, success=False)
        self.assertEqual(self.commands("install"), [])

    def test_custom_user_launcher_is_preserved(self):
        desktop = self.home / ".local/share/applications/xenterm.desktop"
        desktop.parent.mkdir(parents=True)
        content = "[Desktop Entry]\nExec=/custom/xenterm --custom\n"
        desktop.write_text(content)
        result = self.run_installer("--prefix", self.prefix)
        self.assertEqual(desktop.read_text(), content)
        self.assertIn("preserved user launcher", result.stderr)

    def test_xdg_custom_user_launcher_is_preserved(self):
        data = self.root / "xdg data"
        desktop = data / "applications/xenterm.desktop"
        desktop.parent.mkdir(parents=True)
        desktop.write_text("[Desktop Entry]\nExec=xenterm --custom\n")
        result = self.run_installer("--prefix", self.prefix, env={"XDG_DATA_HOME": str(data)})
        self.assertTrue(desktop.is_file())
        self.assertIn(str(desktop), result.stderr)

    def test_unmanaged_destination_launcher_is_not_overwritten(self):
        desktop = self.home / ".local/share/applications/xenterm.desktop"
        desktop.parent.mkdir(parents=True)
        content = "[Desktop Entry]\nExec=xenterm --custom\n"
        desktop.write_text(content)
        self.run_installer("--user", success=False)
        self.assertEqual(desktop.read_text(), content)
        self.assertEqual(self.commands("install"), [])

    def test_exact_legacy_launcher_is_upgraded_and_marked(self):
        desktop = self.prefix / "share/applications/xenterm.desktop"
        desktop.parent.mkdir(parents=True)
        desktop.write_text(LEGACY_DESKTOP)
        self.run_installer("--prefix", self.prefix)
        self.assert_installed(self.prefix)
        self.assertIn("X-XenTerm-Installer=true\n", desktop.read_text())
        self.assertIn(str(self.prefix / "bin/xenterm"), desktop.read_text())

    def test_modified_legacy_launcher_is_not_overwritten(self):
        desktop = self.prefix / "share/applications/xenterm.desktop"
        desktop.parent.mkdir(parents=True)
        variants = (
            LEGACY_DESKTOP + "X-Custom-Setting=true\n",
            LEGACY_DESKTOP.replace("Exec=xenterm\n", "Exec=xenterm --custom\n"),
            LEGACY_DESKTOP.replace("Exec=xenterm --new-window", "Exec=/custom/xenterm"),
            LEGACY_DESKTOP + "# User customization\n",
        )
        for content in variants:
            with self.subTest(content=content):
                desktop.write_text(content)
                self.run_installer("--prefix", self.prefix, success=False)
                self.assertEqual(desktop.read_text(), content)
                self.assertEqual(self.commands("install"), [])

    def test_missing_icon_and_missing_or_failing_cache_tools(self):
        (self.package / "icon@512.png").unlink()
        (self.tools / "update-desktop-database").unlink()
        result = self.run_installer("--prefix", self.prefix, env={"FIXTURE_CACHE_EXIT": "1"})
        self.assertIn("warning: icon not found", result.stderr)
        self.assertIn("Icon=xenterm\n", (self.prefix / "share/applications/xenterm.desktop").read_text())

    def test_bad_arguments_and_missing_binary_do_not_install(self):
        for args in (("--prefix",), ("--prefix=",), ("--unknown",), ("a", "b"),
                     ("--prefix", self.prefix, self.root / "missing/binary")):
            with self.subTest(args=args):
                self.run_installer(*args, success=False)
        self.assertEqual(self.commands("install"), [])

    def test_invalid_desktop_path_rejected_before_writes(self):
        for name in ("has=equals", "newline\ninjection", "tab\tpath", "return\rpath"):
            with self.subTest(name=name):
                self.run_installer("--prefix", self.root / name, success=False)
        self.assertEqual(self.commands("install"), [])

    def test_help_requires_no_binary_or_sudo(self):
        self.binary.unlink()
        (self.tools / "sudo").unlink()
        result = self.run_installer("--help")
        self.assertIn("--prefix", result.stdout)
        self.assertEqual(self.commands("install"), [])

    def test_exec_special_characters_round_trip(self):
        prefix = self.root / 'spaces "quotes" \\slash $cash `tick` %f 100% \'single\' &;()#<>*?~|'
        self.run_installer("--prefix", prefix)
        desktop = self.assert_installed(prefix)
        values = [line[5:] for line in desktop.read_text().splitlines() if line.startswith("Exec=")]
        self.assertEqual(len(values), 2)
        # Undo the desktop string layer, then parse Exec's quoted argument.
        import shlex
        for value, tail in zip(values, ([], ["--new-window"])):
            decoded = value.replace("\\\\", "\\")
            argv = shlex.split(decoded)
            self.assertEqual(argv[:2], ["/usr/bin/env", "--"])
            argv = argv[2:]
            # shlex intentionally preserves \$ and \` inside double quotes.
            argv[0] = argv[0].replace("\\$", "$").replace("\\`", "`").replace("%%", "%")
            self.assertEqual(argv, [str(prefix / "bin/xenterm"), *tail])

    def test_gio_launches_only_fixture_binary_with_exact_path_and_arguments(self):
        # Optional real freedesktop consumer check. The fake shell binary only
        # records argv in this fixture; no XenTerm process is ever launched.
        python = None
        for candidate in dict.fromkeys((sys.executable, "/usr/bin/python3")):
            if Path(candidate).exists():
                probe = subprocess.run(
                    [candidate, "-c", "from gi.repository import Gio"],
                    capture_output=True,
                )
                if probe.returncode == 0:
                    python = candidate
                    break
        if python is None:
            self.skipTest("GIO Python bindings are not installed")
        for name in ('plain space', 'special " \\ $ ` %f 100% \' &;()#<>*?~|'):
            with self.subTest(name=name):
                prefix = self.root / name
                self.run_installer("--prefix", prefix)
                desktop = self.assert_installed(prefix)
                code = """
import sys, time
from pathlib import Path
from gi.repository import Gio
app = Gio.DesktopAppInfo.new_from_filename(sys.argv[1])
assert app
log = Path(sys.argv[2])
context = Gio.AppLaunchContext()
context.setenv('XENTERM_FIXTURE_LAUNCH_LOG', str(log))
for action in ('main', 'new-window'):
    if log.exists():
        log.unlink()
    if action == 'main':
        app.launch([], context)
    else:
        app.launch_action(action, context)
    expected = [sys.argv[3]] + ([] if action == 'main' else ['--new-window'])
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if log.exists() and log.read_text().splitlines() == expected:
            break
        time.sleep(0.02)
    assert log.exists() and log.read_text().splitlines() == expected
"""
                result = subprocess.run(
                    [python, "-c", code, str(desktop), str(self.root / "launch.log"),
                     str(prefix / "bin/xenterm")],
                    capture_output=True, text=True, timeout=15,
                    env=self.env | {
                        "GIO_USE_VFS": "local",
                        "DBUS_SESSION_BUS_ADDRESS": "unix:path=" + str(self.root / "no-session-bus"),
                    },
                )
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
