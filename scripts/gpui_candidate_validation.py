"""Bounded GPUI TestPlatform checks; not native macOS Dock certification."""
import os
from pathlib import Path
import subprocess
import sys

SOURCE = '0205fe53a74fc286d075a3f7a9b56556be3e99f4'
GATES = (
    ('terminal::links::tests::', 8),
    ('ui::view::insert_tests::terminal_link_', 11),
    ('ui::shell::shell_reopen_tests::', 8),
)


def check_listing(output, prefix, expected):
    names = [line.removesuffix(': test') for line in output.splitlines() if line.endswith(': test')]
    if len(names) != expected or len(set(names)) != expected or any(not n.startswith(prefix) for n in names):
        raise RuntimeError(f'{prefix}: expected {expected} unique tests, got {names!r}')
    return names


def run(target):
    if subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip() != SOURCE:
        raise RuntimeError('Product checkout is not the immutable reviewed source')
    env = os.environ.copy()
    home = Path.home()
    # Keep runner-owned toolchains/caches while isolating all app profile locations.
    env.setdefault('CARGO_HOME', str(home / '.cargo'))
    env.setdefault('RUSTUP_HOME', str(home / '.rustup'))
    isolated = Path(env['RUNNER_TEMP']) / 'xenterm-gpui-candidate-profile'
    for name, sub in [('HOME', 'home'), ('USERPROFILE', 'home'), ('XDG_CONFIG_HOME', 'config'),
                      ('XDG_DATA_HOME', 'data'), ('XDG_CACHE_HOME', 'cache'),
                      ('XDG_RUNTIME_DIR', 'runtime'), ('APPDATA', 'appdata'), ('LOCALAPPDATA', 'localappdata')]:
        path = isolated / sub
        path.mkdir(parents=True, exist_ok=True, mode=0o700)
        env[name] = str(path)
    common = ['cargo', 'test', '--locked', '--bin', 'xenterm', '--target', target, '--jobs', '2']
    for prefix, expected in GATES:
        listing = subprocess.run(common + [prefix, '--', '--list'], env=env, text=True,
                                 stdout=subprocess.PIPE, check=True, timeout=3600)
        print(listing.stdout, flush=True)
        names = check_listing(listing.stdout, prefix, expected)
        print(f'Executing {len(names)} enumerated tests for {prefix}', flush=True)
        subprocess.run(common + [prefix, '--', '--test-threads=1'], env=env, check=True, timeout=600)
    subprocess.run([sys.executable, 'tests/macos_reopen_wiring_test.py'], env=env, check=True, timeout=30)
    print('27 GPUI/Rust tests plus 5 source-wiring checks passed; native Dock delivery NOT tested.', flush=True)


if __name__ == '__main__':
    run(sys.argv[1])
