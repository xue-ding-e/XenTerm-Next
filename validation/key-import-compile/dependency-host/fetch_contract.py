"""Pure candidate contracts; no launcher, no network, no qualified fetch yet."""
from pathlib import Path
from adapter.compile_adapter import plain, require

POLICY = 'cargo-fetch-only-v1'
ARGS = ['fetch', '--locked', '--target', 'x86_64-pc-windows-msvc']
from fetch_evidence import verify_transport_evidence
HOMES = dict(HOME='home', USERPROFILE='home', APPDATA='appdata',
             LOCALAPPDATA='localappdata', TEMP='tmp', TMP='tmp',
             CARGO_HOME='cargo-home', RUSTUP_HOME='rustup-home')


def expected_environment(root, windows_root):
    root = plain(root)
    windows_root = plain(windows_root)
    binary = root / 'rustup-home/toolchains/1.99.1-x86_64-pc-windows-msvc/bin'
    values = {key: str(plain(root / part)) for key, part in HOMES.items()}
    require(all((root / part).is_dir() for part in HOMES.values()), 'isolated directories required')
    rustc = plain(binary / 'rustc.exe')
    require(rustc.is_file(), 'real rustc file required')
    values.update(SYSTEMROOT=str(windows_root), WINDIR=str(windows_root),
                  PATH=str(binary) + ';' + str(windows_root / 'System32'),
                  RUSTC=str(rustc), CARGO_HTTP_TIMEOUT='30', CARGO_NET_RETRY='0',
                  CARGO_REGISTRIES_CRATES_IO_PROTOCOL='sparse')
    return values


def validate_request(request, root, windows_root):
    root = plain(root)
    require(isinstance(request, dict) and set(request) == {
        'executable', 'args', 'cwd', 'seconds', 'stdout', 'stderr',
        'commandPolicy', 'completionMode', 'environment'}, 'fetch request inventory')
    require(request['commandPolicy'] == POLICY and request['completionMode'] == 'natural-tree', 'fetch policy')
    require(type(request['seconds']) is int and request['seconds'] == 600 and request['args'] == ARGS, 'fixed fetch command')
    cargo = root / 'rustup-home/toolchains/1.99.1-x86_64-pc-windows-msvc/bin/cargo.exe'
    require(request['executable'] == str(cargo) and request['cwd'] == str(root / 'source'), 'fixed fetch paths')
    require(plain(cargo).is_file() and plain(root / 'source').is_dir(), 'fetch input paths')
    for key in ('stdout', 'stderr'):
        path = root / ('fetch.' + key + '.log')
        require(request[key] == str(path) and not plain(path, False).exists(), 'new fixed fetch logs')
    env = request['environment']
    require(isinstance(env, dict), 'fetch environment')
    folded = {}
    for key, value in env.items():
        require(isinstance(key, str) and key.upper() not in folded and isinstance(value, str) and '\0' not in value, 'ambiguous fetch environment')
        folded[key.upper()] = value
    require(folded == expected_environment(root, windows_root), 'exact isolated fetch environment')
    return True


def qualification_gate(metadata_path=None):
    require(metadata_path is not None, 'fetch native evidence path required')
    # This proves transport fixtures and Host compilation, not Host execution.
    return verify_transport_evidence(metadata_path)


def execute(request, transport=None, qualification=None):
    # Even authenticated synthetic evidence cannot authorize real acquisition.
    require(False, 'fetch stage readiness incomplete')
