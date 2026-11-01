#!/usr/bin/env python3
"""Isolated tests for assets/install-linux.sh (the Linux desktop installer).

Run with `python3 tests/install_linux_test.py` on Linux. The suite creates a
temporary HOME, prefix and fake release bundle, and uses stub tools for sudo,
id, xdg-user-dir, install(1) and the desktop caches: no real system
installation, application, cache update or sudo call happens. Every install
destination is validated to stay inside the fixture root.

This is primarily a POSIX suite. Permission-bit, symlink and sudo tests are
skipped on other hosts, where the installer is still exercised for layout,
manifest, persistence and uninstall behaviour.
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
SCRIPT = REPO / "assets/install-linux.sh"
POSIX = os.name == "posix"
SKIP_POSIX = "requires POSIX permissions/symlinks/sudo"
SKIP_ROOT = "requires a non-root POSIX fixture runner"

# Commands the installer runs that the fixture passes through to the host.
WRAPPER_COMMANDS = (
    "readlink",
    "mktemp",
    "rm",
    "rmdir",
    "grep",
    "cmp",
    "sha256sum",
    "awk",
    "cat",
    "dirname",
)


def find_bash():
    """Locate a bash that can run the installer.

    On Windows, CreateProcess searches System32 before PATH and would find the
    WSL launcher there; prefer a real Git Bash installation instead. A Python
    started from Git Bash also sees a POSIX-style PATH, which shutil.which
    cannot split, so the well-known Git installation paths are checked first.
    """
    if os.name == "nt":
        fallbacks = (
            Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "Git/bin/bash.exe",
            Path(os.environ.get("LocalAppData", "")) / "Programs/Git/bin/bash.exe",
            Path(r"C:\Program Files\Git\bin\bash.exe"),
        )
        for fallback in fallbacks:
            if fallback.is_file():
                return str(fallback)
    candidate = shutil.which("bash")
    if os.name == "nt" and candidate and "system32" in candidate.lower():
        candidate = None
    if candidate is None:
        raise RuntimeError("bash is required to run the installer tests")
    return candidate


BASH = find_bash()


class InstallerTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="xenterm-installer-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.prefix = self.root / "fresh prefix" / "nested"
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.log = self.root / "commands.jsonl"
        self.real = self.resolve_tools()
        if POSIX and "install" not in self.real:
            raise RuntimeError("install(1) is required to run the installer tests")

        self.bundle = self.root / "release package"
        (self.bundle / "lib").mkdir(parents=True)
        (self.bundle / "data/flutter_assets").mkdir(parents=True)
        self.script = self.bundle / "install-linux.sh"
        shutil.copyfile(SCRIPT, self.script)
        self.exe = self.bundle / "xenterm"
        self.exe.write_text(
            "#!/bin/sh\n"
            'printf "%s\\n" "$0" "$@" > "$XENTERM_FIXTURE_LAUNCH_LOG"\n'
        )
        self.exe.chmod(0o755)
        (self.bundle / "lib/libxenterm_bridge.so").write_bytes(b"fixture bridge")
        (self.bundle / "data/flutter_assets/AssetManifest.bin").write_bytes(b"fixture assets")
        (self.bundle / "icon@512.png").write_bytes(b"fixture icon")
        # Runtime profile data from a portable launch must never be installed.
        (self.bundle / "config").mkdir()
        (self.bundle / "config/sessions.db").write_bytes(b"fixture secrets")
        (self.bundle / "log").mkdir()
        (self.bundle / "log/error.log").write_text("fixture log\n")

        for name in WRAPPER_COMMANDS:
            self.make_passthrough(name)
        self.make_install_stub()
        self.make_stub("id", "print(os.environ.get('FIXTURE_UID', '1000'))")
        self.make_stub("sudo", textwrap.dedent(f"""
            with open(os.environ['FIXTURE_LOG'], 'a') as log:
                log.write(json.dumps(['sudo'] + sys.argv[1:]) + chr(10))
            if os.environ.get('FIXTURE_SUDO') != 'allow':
                sys.exit('unexpected sudo invocation')
            if os.environ.get('FIXTURE_UNLOCK_NATIVE'):
                path = Path(os.environ['FIXTURE_UNLOCK_NATIVE'])
                assert path.is_relative_to(Path(os.environ['FIXTURE_ROOT_NATIVE']))
                path.chmod(0o755)
            if sys.argv[1:] != ['-v']:
                os.execvp(sys.argv[1], sys.argv[1:])
        """))
        for name in ("update-desktop-database", "gtk-update-icon-cache"):
            self.make_stub(name, textwrap.dedent(f"""
                with open(os.environ['FIXTURE_LOG'], 'a') as log:
                    log.write(json.dumps([{name!r}] + sys.argv[1:]) + chr(10))
                sys.exit(0)
            """))

        base = dict(os.environ)
        for key in ("PREFIX", "XDG_DATA_HOME", "BASH_ENV", "ENV", "ZDOTDIR"):
            base.pop(key, None)
        base.update(
            HOME=self.to_posix(self.home),
            TMPDIR=self.to_posix(self.root),
            PATH=self.to_posix(self.tools),
            LC_ALL="C",
            LANG="C",
            FIXTURE_ROOT_NATIVE=str(self.root),
            FIXTURE_ROOT_POSIX=self.to_posix(self.root),
            FIXTURE_LOG=str(self.log),
            FIXTURE_UID="1000",
            FIXTURE_SUDO="deny",
            FIXTURE_REAL_INSTALL=self.real.get("install", "") if POSIX else "",
        )
        self.env = base

    # ── fixture helpers ──────────────────────────────────────────────────────

    def to_posix(self, value):
        """Express a fixture path the way the bash script sees it."""
        path = str(value)
        if os.name != "nt":
            return path
        result = subprocess.run(
            [BASH, "-c", 'cygpath -u "$1"', "-", path],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=30,
        )
        if result.returncode != 0:
            raise RuntimeError(f"cygpath failed for {path}: {result.stderr}")
        return result.stdout.strip()

    def resolve_tools(self):
        names = (*WRAPPER_COMMANDS, "install")
        script = "for name in " + " ".join(names) + "; do " \
            'printf "%s=%s\\n" "$name" "$(command -v "$name" || true)"; done'
        result = subprocess.run([BASH, "-c", script], capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=30)
        if result.returncode != 0:
            raise RuntimeError(f"cannot resolve fixture tools: {result.stderr}")
        tools = {}
        for line in result.stdout.splitlines():
            name, _, path = line.partition("=")
            if path:
                tools[name] = path
        missing = [name for name in WRAPPER_COMMANDS if name not in tools]
        if missing:
            raise RuntimeError(f"missing shell tools required by the installer: {missing}")
        return tools

    def make_passthrough(self, name):
        # A shell wrapper resolves cleanly from both POSIX and MSYS bash, unlike
        # a symlink (which Windows may forbid).
        path = self.tools / name
        target = self.real[name].replace("'", "'\\''")
        path.write_text(f"#!/bin/sh\nexec '{target}' \"$@\"\n")
        path.chmod(0o755)

    def make_stub(self, name, code):
        source = self.tools / f"_{name}.py"
        source.write_text(
            "import json, os, re, shutil, sys\n"
            "from pathlib import Path\n"
            + textwrap.dedent(code).lstrip()
        )
        python = sys.executable if POSIX else self.to_posix(sys.executable)
        (self.tools / name).write_text(
            f"#!/bin/sh\nexec '{python}' '{self.to_posix(source)}' \"$@\"\n"
        )
        (self.tools / name).chmod(0o755)

    def make_install_stub(self):
        self.make_stub("install", """
            args = sys.argv[1:]
            dash = args.index('--') if '--' in args else len(args)
            flags = args[:dash]
            paths = args[dash + 1:]
            root = os.path.abspath(os.environ['FIXTURE_ROOT_NATIVE'])
            root_posix = os.environ.get('FIXTURE_ROOT_POSIX', '')

            def native(value):
                value = str(value)
                if root_posix and value.startswith(root_posix):
                    value = root + value[len(root_posix):]
                if os.name == 'nt':
                    match = re.match(r'^/([A-Za-z])/(.*)$', value)
                    if match:
                        sep = chr(92)
                        value = match.group(1).upper() + ':' + sep + match.group(2).replace('/', sep)
                return os.path.normpath(value)

            mode = 0o755
            for index, flag in enumerate(flags):
                if flag == '-m' and index + 1 < len(flags):
                    mode = int(flags[index + 1], 8)
                elif flag.startswith('-m') and len(flag) > 2:
                    mode = int(flag[2:], 8)
            directories = '-d' in flags
            outputs = paths if directories else paths[-1:]
            for output in outputs:
                resolved = os.path.abspath(native(output))
                assert resolved == root or resolved.startswith(root + os.sep), (
                    'install destination outside the fixture: ' + str(output)
                )
            with open(os.environ['FIXTURE_LOG'], 'a') as log:
                log.write(json.dumps(['install'] + args) + chr(10))
            real = os.environ.get('FIXTURE_REAL_INSTALL', '')
            if real:
                os.execv(real, [real] + args)
            if directories:
                for path in paths:
                    os.makedirs(native(path), exist_ok=True)
            else:
                source = paths[-2]
                target = paths[-1]
                parent = os.path.dirname(native(target))
                if parent:
                    os.makedirs(parent, exist_ok=True)
                shutil.copyfile(native(source), native(target))
                if os.name == 'posix':
                    os.chmod(native(target), mode)
        """)

    def add_xdg_user_dir(self, desktop):
        self.make_stub("xdg-user-dir", f"print({self.to_posix(desktop)!r})")

    def run_installer(self, *args, success=True, env=None, cwd=None, script=None):
        command = [BASH, self.to_posix(script or self.script)]
        for arg in args:
            command.append(self.to_posix(arg) if isinstance(arg, Path) else str(arg))
        result = subprocess.run(
            command,
            cwd=str(cwd or self.bundle),
            env=self.env | (env or {}),
            text=True, encoding="utf-8", errors="replace",
            capture_output=True,
            timeout=120,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def uninstall(self, *args, **kwargs):
        return self.run_installer("--uninstall", *args, **kwargs)

    def commands(self, name):
        if not self.log.exists():
            return []
        rows = [json.loads(line) for line in self.log.read_text(encoding="utf-8", errors="surrogateescape").splitlines()]
        return [row for row in rows if row[0] == name]

    def assert_no_install(self):
        self.assertEqual(self.commands("install"), [])

    def assert_installed(self, prefix, menu=True, path=True, desktop_shortcut=None):
        app_dir = prefix / "lib/xenterm/app"
        self.assertEqual((app_dir / "xenterm").read_bytes(), self.exe.read_bytes())
        self.assertEqual(
            (app_dir / "lib/libxenterm_bridge.so").read_bytes(), b"fixture bridge"
        )
        self.assertTrue((app_dir / "data/flutter_assets/AssetManifest.bin").is_file())
        self.assertFalse((app_dir / "config").exists(), "portable profile leaked into the install")
        self.assertFalse((app_dir / "log").exists(), "portable log leaked into the install")
        wrapper = prefix / "bin/xenterm"
        content = wrapper.read_text(encoding="utf-8", errors="surrogateescape")
        self.assertTrue(content.startswith("#!/bin/sh\n"))
        self.assertIn(self.to_posix(app_dir / "xenterm"), content)
        self.assertTrue((prefix / "share/icons/hicolor/512x512/apps/xenterm.png").is_file())
        desktop = prefix / "share/applications/xenterm.desktop"
        if menu:
            text = desktop.read_text(encoding="utf-8", errors="surrogateescape")
            self.assertIn("Exec=", text)
            self.assertIn("X-XenTerm-Installer=true\n", text)
            exec_line = next(line for line in text.splitlines() if line.startswith("Exec="))
            # Undo the desktop-string escaping of literal percent signs.
            self.assertIn(self.to_posix(wrapper), exec_line.replace("%%", "%"))
            self.assertIn("Icon=", text)
        if desktop_shortcut is not None:
            shortcut = desktop_shortcut / "xenterm.desktop"
            self.assertTrue(shortcut.is_file())
            self.assertIn("X-XenTerm-Installer=true\n", shortcut.read_text(encoding="utf-8", errors="surrogateescape"))
        if path:
            self.assertTrue((prefix / "share/xenterm/installer/path.sh").is_file())
            options = (prefix / "share/xenterm/installer/options").read_text(encoding="utf-8", errors="surrogateescape")
            self.assertIn("path=1", options)
        if POSIX:
            self.assertEqual((app_dir / "xenterm").stat().st_mode & 0o777, 0o755)
            self.assertEqual(wrapper.stat().st_mode & 0o777, 0o755)
            if menu:
                self.assertEqual(desktop.stat().st_mode & 0o777, 0o644)
        return desktop

    def manifest_paths(self, prefix):
        manifest = prefix / "share/xenterm/installer/files.sha256"
        rows = []
        for line in manifest.read_text(encoding="utf-8", errors="surrogateescape").splitlines():
            digest, _, path = line.partition("\t")
            self.assertEqual(len(digest), 64)
            rows.append((digest, path))
        return rows

    def assert_marker_count(self, rc, expected):
        text = rc.read_text(encoding="utf-8", errors="surrogateescape") if rc.exists() else ""
        self.assertEqual(text.count("# >>> XenTerm installer PATH:"), expected, text)

    # ── argument handling and bundle discovery ─────────────────────────────

    def test_bash_syntax(self):
        result = subprocess.run([BASH, "-n", self.to_posix(SCRIPT)], capture_output=True, text=True, encoding="utf-8", errors="replace")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_help_and_bad_arguments(self):
        result = self.run_installer("--help")
        self.assertIn("--bundle", result.stdout)
        self.assertIn("--uninstall", result.stdout)
        self.assert_no_install()
        for args in (
            ("--unknown",),
            ("--prefix",),
            ("--prefix=",),
            ("--bundle",),
            ("positional",),
        ):
            with self.subTest(args=args):
                self.run_installer(*args, success=False)
        self.assert_no_install()

    def test_missing_or_incomplete_bundle_fails_before_writes(self):
        lone = self.root / "lone"
        lone.mkdir()
        lone_script = lone / "install-linux.sh"
        shutil.copyfile(SCRIPT, lone_script)
        self.run_installer(
            "--prefix", self.prefix, success=False, cwd=lone, script=lone_script
        )
        empty = self.root / "empty bundle"
        empty.mkdir()
        self.run_installer("--bundle", empty, "--prefix", self.prefix, success=False)
        incomplete = self.root / "incomplete bundle"
        incomplete.mkdir()
        (incomplete / "xenterm").write_text("x")
        self.run_installer("--bundle", incomplete, "--prefix", self.prefix, success=False)
        self.assert_no_install()
        self.assertFalse(self.prefix.exists())

    def test_explicit_bundle_argument(self):
        moved = self.root / "moved bundle"
        shutil.copytree(self.bundle, moved)
        installer = self.root / "installer-copy.sh"
        shutil.copyfile(SCRIPT, installer)
        result = subprocess.run(
            [
                BASH,
                self.to_posix(installer),
                "--user",
                "--prefix",
                self.to_posix(self.prefix),
                "--bundle",
                self.to_posix(moved),
            ],
            cwd=str(self.root),
            env=self.env,
            text=True, encoding="utf-8", errors="replace",
            capture_output=True,
            timeout=120,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_installed(self.prefix)

    # ── default and custom prefixes ────────────────────────────────────────

    def test_user_default_prefix(self):
        self.run_installer("--user")
        prefix = self.home / ".local"
        self.assert_installed(prefix)
        self.assertEqual(self.commands("sudo"), [])
        self.assertIn("XenTerm installer PATH", (self.home / ".bashrc").read_text(encoding="utf-8", errors="surrogateescape"))
        self.uninstall("--user")
        self.assertFalse((prefix / "bin/xenterm").exists())

    def test_custom_prefix_forms(self):
        self.run_installer("--user", "--prefix", self.prefix)
        self.assert_installed(self.prefix)
        self.uninstall("--user", "--prefix", self.prefix)

        self.run_installer("--user", f"--prefix={self.to_posix(self.prefix)}")
        self.assert_installed(self.prefix)
        self.uninstall("--user", "--prefix", self.prefix)

        env = {"PREFIX": self.to_posix(self.prefix)}
        self.run_installer("--user", env=env)
        self.assert_installed(self.prefix)
        self.uninstall("--user", env=env)

    def test_cli_prefix_wins_over_environment_and_user_default(self):
        self.run_installer(
            "--user",
            "--prefix",
            self.prefix,
            env={"PREFIX": self.to_posix(self.root / "ignored")},
        )
        self.assert_installed(self.prefix)
        self.assertFalse((self.root / "ignored").exists())
        self.uninstall("--user", "--prefix", self.prefix)

    def test_prefix_with_spaces_and_non_ascii(self):
        prefix = self.root / "安装 目录 avec espaces"
        self.run_installer("--user", "--prefix", prefix)
        desktop = self.assert_installed(prefix)
        self.assertIn(self.to_posix(prefix / "bin/xenterm"), desktop.read_text(encoding="utf-8", errors="surrogateescape"))
        self.uninstall("--user", "--prefix", prefix)
        self.assertFalse((prefix / "bin/xenterm").exists())

    @unittest.skipUnless(POSIX, "Windows cannot represent a colon in a directory name")
    def test_colon_prefix_requires_path_opt_out(self):
        prefix = self.root / "colon:prefix"
        self.run_installer("--user", "--prefix", prefix, success=False)
        self.assert_no_install()
        self.run_installer("--user", "--prefix", prefix, "--no-add-to-path")
        self.assert_installed(prefix, path=False)
        self.uninstall("--user", "--prefix", prefix, "--no-add-to-path")

    def test_invalid_prefix_characters_are_rejected_before_writes(self):
        names = ["has=equals", "tab\tpath"]
        if POSIX:
            # MSYS bash normalises newlines in command-line arguments, so the
            # control-character cases are only meaningful on POSIX.
            names += ["newline\ninjection", "return\rpath"]
        for name in names:
            with self.subTest(name=name):
                self.run_installer("--user", "--prefix", self.root / name, success=False)
        self.assert_no_install()

    # ── bundle contents, wrapper and desktop entry ─────────────────────────

    def test_wrapper_forwards_arguments(self):
        self.run_installer("--user", "--prefix", self.prefix)
        wrapper = self.prefix / "bin/xenterm"
        launch_log = self.root / "launch.log"
        result = subprocess.run(
            [BASH, self.to_posix(wrapper), "one", "two words"],
            env=self.env | {"XENTERM_FIXTURE_LAUNCH_LOG": self.to_posix(launch_log)},
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            timeout=60,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        lines = launch_log.read_text(encoding="utf-8", errors="surrogateescape").splitlines()
        self.assertEqual(lines[0], self.to_posix(self.prefix / "lib/xenterm/app/xenterm"))
        self.assertEqual(lines[1:], ["one", "two words"])
        self.uninstall("--user", "--prefix", self.prefix)

    def test_icon_and_desktop_contents(self):
        self.run_installer("--user", "--prefix", self.prefix)
        desktop = self.assert_installed(self.prefix)
        text = desktop.read_text(encoding="utf-8", errors="surrogateescape")
        icon_line = next(line for line in text.splitlines() if line.startswith("Icon="))
        self.assertIn(
            self.to_posix(self.prefix / "share/icons/hicolor/512x512/apps/xenterm.png"),
            icon_line,
        )
        self.assertIn("StartupWMClass=xenterm\n", text)
        self.uninstall("--user", "--prefix", self.prefix)

    def test_percent_prefix_uses_env_exec_form(self):
        prefix = self.root / "100% prefix"
        self.run_installer("--user", "--prefix", prefix)
        desktop = self.assert_installed(prefix)
        exec_line = next(
            line for line in desktop.read_text(encoding="utf-8", errors="surrogateescape").splitlines() if line.startswith("Exec=")
        )
        self.assertTrue(exec_line.startswith("Exec=/usr/bin/env -- "), exec_line)
        self.uninstall("--user", "--prefix", prefix)

    # ── integration defaults, persistence and cleanup ──────────────────────

    def test_opt_out_flags_disable_everything(self):
        self.run_installer(
            "--user",
            "--no-add-to-path",
            "--no-desktop-shortcut",
            "--no-start-menu",
        )
        prefix = self.home / ".local"
        self.assertTrue((prefix / "bin/xenterm").is_file())
        self.assertFalse((self.home / ".bashrc").exists())
        self.assertFalse((prefix / "share/applications/xenterm.desktop").exists())
        self.assertFalse((prefix / "share/xenterm/installer/path.sh").exists())
        options = (prefix / "share/xenterm/installer/options").read_text(encoding="utf-8", errors="surrogateescape")
        self.assertIn("path=0", options)
        self.assertIn("desktop=0", options)
        self.assertIn("menu=0", options)
        self.uninstall("--user")

    def test_choices_persist_and_explicit_flags_override(self):
        self.run_installer(
            "--user", "--no-add-to-path", "--no-desktop-shortcut", "--no-start-menu"
        )
        self.run_installer("--user")
        self.assertFalse((self.home / ".bashrc").exists())
        self.assertFalse((self.home / ".local/share/applications/xenterm.desktop").exists())
        self.run_installer("--user", "--add-to-path", "--start-menu")
        self.assertTrue((self.home / ".bashrc").exists())
        self.assertTrue((self.home / ".local/share/applications/xenterm.desktop").is_file())
        self.uninstall("--user")

    def test_xdg_desktop_shortcut_created_and_removed(self):
        desktop = self.home / "桌面"
        desktop.mkdir()
        self.add_xdg_user_dir(desktop)
        self.run_installer("--user")
        self.assert_installed(self.home / ".local", desktop_shortcut=desktop)
        self.uninstall("--user")
        self.assertFalse((desktop / "xenterm.desktop").exists())

    def test_missing_xdg_user_dir_skips_shortcut_with_warning(self):
        result = self.run_installer("--user")
        self.assertIn("desktop shortcut skipped", result.stderr)
        self.assert_installed(self.home / ".local")
        self.uninstall("--user")

    def test_path_block_idempotent_and_removed(self):
        rc = self.home / ".bashrc"
        rc.write_text("export KEEP_ME=1\n")
        self.run_installer("--user")
        self.run_installer("--user")
        self.assert_marker_count(rc, 1)
        self.assertIn("export KEEP_ME=1\n", rc.read_text(encoding="utf-8", errors="surrogateescape"))
        self.uninstall("--user")
        self.assertEqual(rc.read_text(encoding="utf-8", errors="surrogateescape"), "export KEEP_ME=1\n")

    def test_login_and_zsh_shell_files_are_updated(self):
        profile = self.home / ".bash_profile"
        profile.write_text("export KEEP_LOGIN=1\n")
        zdir = self.home / "zsh-config"
        zdir.mkdir()
        env = {"ZDOTDIR": self.to_posix(zdir)}
        self.run_installer("--user", env=env)
        self.assertIn("XenTerm installer PATH", profile.read_text(encoding="utf-8", errors="surrogateescape"))
        self.assertFalse((self.home / ".profile").exists())
        self.assertIn("XenTerm installer PATH", (zdir / ".zshrc").read_text(encoding="utf-8", errors="surrogateescape"))
        self.assertFalse((self.home / ".zshrc").exists())
        self.uninstall("--user", env=env)
        self.assertEqual(profile.read_text(encoding="utf-8", errors="surrogateescape"), "export KEEP_LOGIN=1\n")

    def test_changed_shell_context_blocks_before_removal(self):
        self.run_installer("--user")
        zdir = self.home / "another-zsh"
        zdir.mkdir()
        rc = self.home / ".bashrc"
        before = rc.read_text(encoding="utf-8", errors="surrogateescape")
        self.run_installer(
            "--user", success=False, env={"ZDOTDIR": self.to_posix(zdir)}
        )
        self.assertTrue((self.home / ".local/bin/xenterm").is_file())
        self.assertEqual(rc.read_text(encoding="utf-8", errors="surrogateescape"), before)
        self.uninstall(
            "--user", success=False, env={"ZDOTDIR": self.to_posix(zdir)}
        )
        self.assertTrue((self.home / ".local/bin/xenterm").is_file())

    def test_modified_path_block_blocks_uninstall(self):
        self.run_installer("--user")
        rc = self.home / ".bashrc"
        changed = rc.read_text(encoding="utf-8", errors="surrogateescape").replace("# <<< XenTerm", "export KEEP_ME=1\n# <<< XenTerm")
        rc.write_text(changed)
        self.uninstall("--user", success=False)
        self.assertEqual(rc.read_text(encoding="utf-8", errors="surrogateescape"), changed)
        self.assertTrue((self.home / ".local/bin/xenterm").is_file())
        rc.write_text(changed.replace("export KEEP_ME=1\n", ""))
        self.uninstall("--user")

    def test_shell_rc_symlink_is_not_followed(self):
        if not POSIX:
            self.skipTest(SKIP_POSIX)
        foreign = self.root / "foreign-rc"
        foreign.write_text("untouched\n")
        (self.home / ".bashrc").symlink_to(foreign)
        self.run_installer("--user", success=False)
        self.assertEqual(foreign.read_text(encoding="utf-8", errors="surrogateescape"), "untouched\n")
        self.assert_no_install()

    # ── install/upgrade/uninstall file ownership ───────────────────────────

    def test_repeat_install_is_idempotent(self):
        self.run_installer("--user", "--prefix", self.prefix)
        first = self.manifest_paths(self.prefix)
        self.run_installer("--user", "--prefix", self.prefix)
        self.assertEqual(self.manifest_paths(self.prefix), first)
        paths = [path for _, path in first]
        self.assertEqual(len(paths), len(set(paths)), "duplicate manifest entries")
        self.uninstall("--user", "--prefix", self.prefix)

    def test_upgrade_replaces_and_prunes_files(self):
        self.run_installer("--user", "--prefix", self.prefix)
        added = self.bundle / "lib/new-module.so"
        added.write_bytes(b"new module")
        (self.bundle / "data/flutter_assets/AssetManifest.bin").write_bytes(b"updated assets")
        self.run_installer("--user", "--prefix", self.prefix)
        app_dir = self.prefix / "lib/xenterm/app"
        self.assertEqual((app_dir / "lib/new-module.so").read_bytes(), b"new module")
        self.assertEqual(
            (app_dir / "data/flutter_assets/AssetManifest.bin").read_bytes(), b"updated assets"
        )
        added.unlink()
        self.run_installer("--user", "--prefix", self.prefix)
        self.assertFalse(
            (app_dir / "lib/new-module.so").exists(), "stale file survived the upgrade"
        )
        self.uninstall("--user", "--prefix", self.prefix)

    def test_modified_installed_file_preserves_everything_and_fails(self):
        self.run_installer("--user", "--prefix", self.prefix)
        target = self.prefix / "lib/xenterm/app/lib/libxenterm_bridge.so"
        target.write_bytes(b"my replacement")
        wrapper = self.prefix / "bin/xenterm"
        wrapper_before = wrapper.read_bytes()
        result = self.uninstall("--user", "--prefix", self.prefix, success=False)
        self.assertIn("preserved modified file", result.stderr)
        self.assertEqual(target.read_bytes(), b"my replacement")
        self.assertEqual(wrapper.read_bytes(), wrapper_before)
        self.assertTrue((self.prefix / "share/xenterm/installer/files.sha256").is_file())
        target.write_bytes(b"fixture bridge")
        self.uninstall("--user", "--prefix", self.prefix)

    def test_clean_uninstall_preserves_unrelated_files(self):
        self.run_installer("--user", "--prefix", self.prefix)
        keep = self.prefix / "share/applications/keep.txt"
        keep.write_text("unrelated\n")
        home_keep = self.home / "notes.txt"
        home_keep.write_text("mine\n")
        self.uninstall("--user", "--prefix", self.prefix)
        app_dir = self.prefix / "lib/xenterm/app"
        self.assertFalse((self.prefix / "bin/xenterm").exists())
        self.assertFalse((self.prefix / "share/applications/xenterm.desktop").exists())
        self.assertFalse((self.prefix / "share/icons/hicolor/512x512/apps/xenterm.png").exists())
        self.assertFalse((self.prefix / "share/xenterm/installer/files.sha256").exists())
        self.assertFalse(app_dir.exists())
        self.assertEqual(keep.read_text(encoding="utf-8", errors="surrogateescape"), "unrelated\n")
        self.assertEqual(home_keep.read_text(encoding="utf-8", errors="surrogateescape"), "mine\n")
        result = self.uninstall("--user", "--prefix", self.prefix)
        self.assertIn("No managed installation", result.stdout)

    def test_foreign_launchers_and_wrapper_are_not_overwritten(self):
        cases = (
            (self.prefix / "bin/xenterm", "foreign wrapper"),
            (self.prefix / "share/icons/hicolor/512x512/apps/xenterm.png", "foreign icon"),
            (self.prefix / "share/applications/xenterm.desktop", "foreign launcher"),
        )
        for path, content in cases:
            with self.subTest(path=path.name):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)
                self.run_installer("--prefix", self.prefix, success=False)
                self.assertEqual(path.read_text(encoding="utf-8", errors="surrogateescape"), content)
                self.assert_no_install()
                path.unlink()

    def test_foreign_user_desktop_shortcut_is_not_overwritten(self):
        desktop = self.home / "Desktop"
        desktop.mkdir()
        shortcut = desktop / "xenterm.desktop"
        shortcut.write_text("foreign shortcut")
        self.add_xdg_user_dir(desktop)
        self.run_installer("--user", success=False)
        self.assertEqual(shortcut.read_text(encoding="utf-8", errors="surrogateescape"), "foreign shortcut")
        self.assert_no_install()

    def test_empty_uninstall_is_safe(self):
        result = self.uninstall("--user", "--prefix", self.root / "never-installed")
        self.assertIn("No managed installation", result.stdout)
        self.assert_no_install()
        self.assertEqual(self.commands("rm"), [])

    # ── system installs ────────────────────────────────────────────────────

    def test_system_install_rejects_integration(self):
        for args in (("--add-to-path",), ("--desktop-shortcut",)):
            with self.subTest(args=args):
                self.run_installer("--prefix", self.prefix, *args, success=False)
        self.assert_no_install()

    def test_system_install_without_home(self):
        env = dict(self.env)
        env.pop("HOME", None)
        self.run_installer("--prefix", self.prefix, env=env)
        self.assert_installed(self.prefix, path=False)
        options = (self.prefix / "share/xenterm/installer/options").read_text(encoding="utf-8", errors="surrogateescape")
        self.assertIn("path=0", options)
        self.assertIn("desktop=0", options)
        self.assertIn("menu=1", options)
        self.assertFalse((self.prefix / "share/xenterm/installer/path.sh").exists())
        self.uninstall("--prefix", self.prefix, env=env)

    def test_fresh_writable_prefix_needs_no_sudo(self):
        self.run_installer("--prefix", self.prefix)
        self.assert_installed(self.prefix, path=False)
        self.assertEqual(self.commands("sudo"), [])

    # ── cache tools are optional ───────────────────────────────────────────

    def test_cache_tools_are_optional(self):
        (self.tools / "update-desktop-database").unlink()
        (self.tools / "gtk-update-icon-cache").unlink()
        self.run_installer("--user", "--prefix", self.prefix)
        self.assert_installed(self.prefix)
        self.uninstall("--user", "--prefix", self.prefix)

    # ── POSIX-only permission and sudo behaviour ───────────────────────────

    def locked_prefix(self):
        if not POSIX:
            self.skipTest(SKIP_POSIX)
        if os.geteuid() == 0:
            self.skipTest(SKIP_ROOT)
        self.prefix.mkdir(parents=True)
        self.prefix.chmod(0o555)
        self.addCleanup(self.prefix.chmod, 0o755)

    def test_unwritable_prefix_uses_sudo_for_system_install(self):
        self.locked_prefix()
        self.run_installer(
            "--prefix",
            self.prefix,
            env={
                "FIXTURE_SUDO": "allow",
                "FIXTURE_UNLOCK_NATIVE": str(self.prefix),
            },
        )
        self.assert_installed(self.prefix, path=False)
        self.assertEqual(self.commands("sudo")[0], ["sudo", "-v"])

    def test_user_mode_never_escalates(self):
        self.locked_prefix()
        result = self.run_installer("--user", "--prefix", self.prefix, success=False)
        self.assertIn("--user never uses sudo", result.stderr)
        self.assertEqual(self.commands("sudo"), [])
        self.assert_no_install()

    def test_unwritable_prefix_without_sudo_has_actionable_error(self):
        self.locked_prefix()
        (self.tools / "sudo").unlink()
        result = self.run_installer("--prefix", self.prefix, success=False)
        self.assertIn("writable --prefix", result.stderr)
        self.assert_no_install()


if __name__ == "__main__":
    unittest.main(verbosity=2)
