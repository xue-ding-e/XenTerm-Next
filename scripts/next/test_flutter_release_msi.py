"""Exercise release validation helpers/PowerShell AST; never run MSI locally.

Synthetic .exe/.dll/.msi files below are inert text. Portable tests inspect the
source/AST. Windows-only tests extract function definitions and invoke validation
helpers in a separate process; only the fail-closed runner gate may execute the
full script. Having pwsh on Linux/macOS does not enable Windows path helpers.
"""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
import warnings
import zipfile


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / 'scripts/next/Test-Flutter-ReleaseMsi.ps1'
POWERSHELL = shutil.which('pwsh') or shutil.which('powershell')
SHA = 'a' * 40
VERSION = 'xt-v0.7.3-beta.42+aaaaaaaa'
STEM = f'XenTerm-Next-Flutter-{SHA}-windows-x86_64'
BINARIES = ('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll',
            'flutter_windows.dll', 'msvcp140.dll', 'vcruntime140.dll',
            'vcruntime140_1.dll', 'msvcp140_atomic_wait.dll', 'plugins/fixture.dll')
MARKER = 'RELEASE_MSI_HELPER_RESULT='

LOAD_FUNCTIONS = r'''
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($env:XENTERM_MSI_TEST_SCRIPT, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw ($errors.Message -join '; ') }
$definitions = $ast.FindAll({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] }, $false)
foreach ($definition in $definitions) { . ([scriptblock]::Create($definition.Extent.Text)) }
'''


def powershell(code, extra_env=None):
    env = os.environ.copy()
    env['XENTERM_MSI_TEST_SCRIPT'] = str(SCRIPT)
    if extra_env:
        env.update(extra_env)
    # EncodedCommand handles Windows paths and literal newlines without shell
    # interpolation or writing an executable helper into the shared checkout.
    encoded = base64.b64encode(code.encode('utf-16-le')).decode('ascii')
    result = subprocess.run(
        [POWERSHELL, '-NoProfile', '-NonInteractive', '-EncodedCommand', encoded],
        capture_output=True, text=True, encoding='utf-8', errors='replace', env=env,
        timeout=35,
    )
    if result.returncode:
        raise AssertionError(f'PowerShell harness failed ({result.returncode}): {result.stderr}\n{result.stdout}')
    lines = [line[len(MARKER):] for line in result.stdout.splitlines() if line.startswith(MARKER)]
    if len(lines) != 1:
        raise AssertionError(f'Missing single helper result: {result.stdout}\n{result.stderr}')
    return json.loads(lines[0])


def helper(action, **kwargs):
    payload = base64.b64encode(json.dumps(dict(action=action, **kwargs)).encode()).decode('ascii')
    return powershell(LOAD_FUNCTIONS + f"\n$request = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{payload}')) | ConvertFrom-Json\n" + r'''
try {
    $value = $null
    switch ($request.action) {
        'receipt' {
            $inputs = Read-ReleaseInputs $request.directory $request.sha $request.version
            $value = @{ candidate = $inputs.Candidate; version = $inputs.Receipt.msi_product_version }
        }
        'bundle' {
            $receipt = Get-Content -LiteralPath $request.receipt -Raw | ConvertFrom-Json
            $value = Read-PortableRuntime $request.bundle $request.sha $request.version $receipt
        }
        'archive' { Assert-PortableArchive $request.archive $request.stem }
        'version' { Assert-ReleaseProductVersion $request.version }
        'path' { $value = Assert-PathWithin $request.path $request.directory }
        'native-inventory' {
            # Read-only native enumeration: never install, reconfigure or
            # output the user's product names, versions or locations.
            $nativeInstaller = New-Object -ComObject WindowsInstaller.Installer
            try {
                $snapshot = @(Get-OtherProductsSnapshot $nativeInstaller)
                $codes = @(Get-MsiStringList $nativeInstaller 'Products' $null)
                $value = @{ count = $snapshot.Count; complete = ($snapshot.Count -eq $codes.Count) }
            } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($nativeInstaller) }
        }
        'environment-clear' {
            $name = 'XENTERM_MSI_LIFECYCLE_ENV_PROBE'
            try {
                [Environment]::SetEnvironmentVariable($name, 'fixture', 'Process')
                $setters = @($ast.FindAll({param($n)
                    $n -is [Management.Automation.Language.InvokeMemberExpressionAst] -and
                    $n.Member.Extent.Text -eq 'SetEnvironmentVariable' -and
                    $n.Arguments[0].Extent.Text -eq '$name'
                }, $true))
                if (-not $setters.Count) { throw 'Missing production environment clear call.' }
                . ([scriptblock]::Create($setters[0].Extent.Text))
                $value = @{ removed = -not [Environment]::GetEnvironmentVariables('Process').Contains($name) }
            } finally { [Environment]::SetEnvironmentVariable($name, [NullString]::Value, 'Process') }
        }
        'msi-identity' {
            # Inject database rows, never COM/msiexec. This executes the same
            # candidate identity/component checks against deliberately bad MSIs.
            function Read-MsiRows {
                param($Installer, [string] $Msi, [string] $Query, [int] $Columns)
                $rows = switch ($Query) {
                    'SELECT `Property`, `Value` FROM `Property`' { $request.rows.properties }
                    'SELECT `UpgradeCode` FROM `Upgrade`' { $request.rows.upgrades }
                    'SELECT `FileName` FROM `File`' { $request.rows.files }
                    'SELECT `Registry`, `Root`, `Key`, `Name`, `Value` FROM `Registry`' { $request.rows.registry }
                    'SELECT `Attributes`, `KeyPath` FROM `Component`' { $request.rows.components }
                    'SELECT `FileName` FROM `RemoveFile`' { $request.rows.removals }
                    default { throw "Unexpected MSI query: $Query" }
                }
                foreach ($row in $rows) { [pscustomobject]@{ Values = @($row.values) } }
            }
            $value = Read-MsiIdentity $null 'INERT-NOT-OPENED.msi' '0.7.3' @('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll', 'msvcp140.dll')
        }
        'ast' {
            $commands = @($ast.FindAll({ param($n) $n -is [Management.Automation.Language.CommandAst] }, $true) | ForEach-Object {
                $parent = $_.Parent
                while ($parent -and $parent -isnot [Management.Automation.Language.FunctionDefinitionAst]) { $parent = $parent.Parent }
                @{ name = $_.GetCommandName(); text = $_.Extent.Text; owner = $(if ($parent) { $parent.Name } else { '' }); offset = $_.Extent.StartOffset }
            })
            $value = @{ commands = $commands; functions = @($definitions.Name) }
        }
        default { throw 'Unrecognized test action.' }
    }
    $result = @{ ok = $true; value = $value }
} catch { $result = @{ ok = $false; error = $_.Exception.Message } }
Write-Output ('RELEASE_MSI_HELPER_RESULT=' + ($result | ConvertTo-Json -Depth 12 -Compress))
''')


class ReleaseMsiSourceContractTests(unittest.TestCase):
    """Keep source contracts active even on hosts without PowerShell."""

    @classmethod
    def setUpClass(cls):
        cls.source = SCRIPT.read_text(encoding='utf-8-sig')

    def test_cleanup_tracks_before_install_and_evidence_follows_uninstall(self):
        self.assertLess(self.source.index('$trackedProducts.Add($fixtureCode)'), self.source.index("Invoke-ReleaseMsi $installArguments"))
        self.assertLess(self.source.index('$trackedProducts.Add($candidateCode)'), self.source.index("Invoke-ReleaseMsi @('/i'"))
        self.assertIn('$index = $trackedProducts.Count - 1', self.source)
        self.assertIn('Assert-MsiProductState $installer $code -1', self.source)
        self.assertLess(self.source.index('Assert-UninstalledPayload $install'), self.source.index('$result.passed = $true'))
        self.assertLess(self.source.index('$result.passed = $true'), self.source.index('Write-AcceptanceEvidence $EvidencePath'))
        self.assertIn('[IO.FileMode]::CreateNew', self.source)
        self.assertIn('XENTERM_RELEASE_MSI_ACCEPTANCE=', self.source)

    def test_com_lists_use_native_count_and_parameterized_invokemember(self):
        for contract in ("InvokeMember('OpenDatabase', 'InvokeMethod'", "InvokeMember('OpenView', 'InvokeMethod'",
                         "InvokeMember('StringData', 'GetProperty'", "InvokeMember('Count', 'GetProperty', $null, $list, $null)",
                         "InvokeMember('Item', 'GetProperty'", "InvokeMember('ProductState', 'GetProperty'",
                         "'SELECT `Attributes`, `KeyPath` FROM `Component`'", "'SELECT `FileName` FROM `RemoveFile`'",
                         'Assert-OtherProductsUnchanged $installer $before', 'Assert-UpgradeFamily $installer $result.upgrade_code'):
            self.assertIn(contract, self.source)


@unittest.skipUnless(POWERSHELL, 'PowerShell AST checks require pwsh or powershell')
class ReleaseMsiAstContractTests(unittest.TestCase):
    """Parse source on Windows/Linux/macOS without executing product helpers."""

    @classmethod
    def setUpClass(cls):
        cls.source = SCRIPT.read_text(encoding='utf-8-sig')
        parsed = helper('ast')
        if not parsed['ok']:
            raise AssertionError(parsed)
        cls.commands = parsed['value']['commands']

    def test_builds_only_old_fixture_and_installs_exact_candidate(self):
        builds = [c for c in self.commands if c['name'] is None and 'package_windows_installer.ps1' in c['text']]
        self.assertEqual(len(builds), 1)
        self.assertIn('-ProductVersion 0.1.0', builds[0]['text'])
        self.assertIn('-BundleDir $bundle', builds[0]['text'])
        self.assertIn('-WixBin $WixBin', builds[0]['text'])
        calls = [c for c in self.commands if c['name'] == 'Invoke-ReleaseMsi' and not c['owner']]
        self.assertEqual(len(calls), 4)  # old, exact candidate, normal uninstall, failure cleanup
        upgrade = next(c for c in calls if 'upgrade-candidate.log' in c['text'])
        self.assertIn('$inputs.Candidate', upgrade['text'])
        self.assertNotIn('INSTALLFOLDER', upgrade['text'])
        digest = self.source.index('Assert-FileDigest $inputs.Candidate')
        self.assertLess(digest, upgrade['offset'])

    def test_gate_precedes_every_main_side_effect(self):
        main = [c for c in self.commands if not c['owner']]
        gate = next(c for c in main if c['name'] == 'Assert-HostedWindowsRunner')
        for command in main:
            if command['name'] in ('New-Item', 'New-Object', 'Expand-Archive', 'Invoke-ReleaseMsi') or 'package_windows_installer.ps1' in command['text']:
                self.assertGreater(command['offset'], gate['offset'])

    def test_native_installer_is_quiet_logged_and_never_rebooted(self):
        starts = [c for c in self.commands if c['name'] == 'Start-Process']
        self.assertEqual(len(starts), 2)  # native installer and exact installed CLI
        for call in starts:
            for option in ('-Wait', '-PassThru', '-WindowStyle Hidden'):
                self.assertIn(option, call['text'])
        for flag in ("'/qn'", "'/norestart'", "'/l*v'"):
            self.assertIn(flag, self.source)
        self.assertFalse([c for c in self.commands if c['name'] in ('Stop-Process', 'taskkill', 'taskkill.exe', 'Remove-Item', 'Set-ItemProperty', 'Remove-ItemProperty')])


@unittest.skipUnless(POWERSHELL and os.name == 'nt', 'Windows PowerShell dynamic helper checks require Windows')
class ReleaseMsiWindowsHelperTests(unittest.TestCase):
    def test_full_program_refuses_each_non_hosted_environment_before_writes(self):
        with tempfile.TemporaryDirectory(prefix='xenterm-msi-gate-') as directory:
            valid = {'GITHUB_ACTIONS': 'true', 'RUNNER_OS': 'Windows',
                     'RUNNER_ENVIRONMENT': 'github-hosted', 'RUNNER_TEMP': directory}
            for field, wrong in (('GITHUB_ACTIONS', ''), ('RUNNER_OS', 'Linux'),
                                 ('RUNNER_ENVIRONMENT', 'self-hosted'), ('RUNNER_TEMP', '')):
                with self.subTest(field=field):
                    env = os.environ.copy()
                    env.update(valid)
                    env[field] = wrong
                    result = subprocess.run(
                        [POWERSHELL, '-NoProfile', '-NonInteractive', '-File', str(SCRIPT),
                         '-ArtifactDirectory', directory, '-ExpectedSourceSha', SHA,
                         '-ExpectedRuntimeVersion', VERSION, '-WixBin', directory],
                        capture_output=True, text=True, encoding='utf-8', errors='replace',
                        env=env, timeout=30,
                    )
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('disposable GitHub-hosted Windows runner', result.stderr)
                    self.assertEqual(list(Path(directory).iterdir()), [])

    def test_temp_path_boundary_handles_siblings_traversal_and_canonical_drive_prefix(self):
        cases = ((r'C:\runner\temp\task\evidence.json', True),
                 (r'\\?\C:\runner\temp\task\evidence.json', True),
                 (r'C:\runner\temp', False), (r'C:\runner\temp-other\evidence.json', False),
                 (r'C:\runner\temp\..\elsewhere\evidence.json', False),
                 (r'\\server\share\evidence.json', False), ('evidence.json', False))
        for path, expected in cases:
            with self.subTest(path=path):
                self.assertEqual(helper('path', path=path, directory=r'C:\runner\temp')['ok'], expected)

    def test_numeric_msi_version_must_support_real_upgrade(self):
        for version, expected in (('0.1.1', True), ('1.0.0', True), ('255.255.65535', True),
                                  ('0.1.0', False), ('0.0.9', False), ('256.0.0', False),
                                  ('1.256.0', False), ('1.0.65536', False),
                                  ('1.2.3.4', False), ('01.2.3', False), ('beta', False)):
            with self.subTest(version=version):
                self.assertEqual(helper('version', version=version)['ok'], expected)

    def test_actual_msi_database_must_match_receipt_family_payload_and_native_components(self):
        family = '{D308C8B0-2E75-4F13-B4A8-7DD705095893}'
        product = '{E8DF03B7-8AC7-49F7-98E9-B36BC348A120}'
        row = lambda *values: {'values': list(values)}
        valid = {
            'properties': [row('ProductCode', product), row('UpgradeCode', family),
                           row('ProductName', 'XenTerm Next Flutter'), row('ProductVersion', '0.7.3')],
            'upgrades': [row(family)],
            'files': [row('GUI|xenterm.exe'), row('CLI|xenterm-cli.exe'),
                      row('BRIDGE|xenterm_bridge.dll'), row('CRT|msvcp140.dll')],
            'registry': [row('InstallLocation', '1', r'Software\XenTerm-Next\Flutter',
                             'InstallLocation', '[INSTALLFOLDER]')],
            'components': [row('260', 'InstallLocation')], 'removals': [row('')],
        }
        accepted = helper('msi-identity', rows=valid)
        self.assertTrue(accepted['ok'], accepted)
        self.assertEqual(accepted['value'], product)
        mutations = (
            ('properties', 3, row('ProductVersion', '0.7.2')),
            ('properties', 1, row('UpgradeCode', '{6E8F2A1B-3C4D-4E5F-9A0B-1C2D3E4F5A6B}')),
            ('properties', 2, row('ProductName', 'XenTerm Next GPUI')),
            ('upgrades', 0, row('{6E8F2A1B-3C4D-4E5F-9A0B-1C2D3E4F5A6B}')),
            ('files', 3, row('WRONG|other.dll')),
            ('registry', 0, row('InstallLocation', '2', r'Software\XenTerm', 'InstallLocation', '[INSTALLFOLDER]')),
            ('components', 0, row('256', 'InstallLocation')),
            ('removals', 0, row('*')),
        )
        for table, index, changed in mutations:
            with self.subTest(table=table, row=changed):
                bad = json.loads(json.dumps(valid))
                bad[table][index] = changed
                self.assertFalse(helper('msi-identity', rows=bad)['ok'])


@unittest.skipUnless(POWERSHELL and os.name == 'nt', 'Windows PowerShell helper checks require Windows')
class ReleaseMsiArtifactTests(unittest.TestCase):
    def test_native_installer_inventory_preserves_advertised_registrations(self):
        result = helper('native-inventory')
        self.assertTrue(result['ok'], result)
        self.assertTrue(result['value']['complete'])

    def test_data_directory_override_is_removed_not_set_to_an_empty_string(self):
        result = helper('environment-clear')
        self.assertTrue(result['ok'], result)
        self.assertTrue(result['value']['removed'])

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='xenterm-release-msi-test-')
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.artifacts = self.directory / 'artifacts'
        self.artifacts.mkdir()
        self.bundle = self.directory / 'bundle'
        self.bundle.mkdir()
        for name in (*BINARIES, 'data/flutter_assets/fixture.txt'):
            path = self.bundle / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(f'INERT SYNTHETIC FIXTURE: {name}\n'.encode())
        self.runtime = {
            'source_sha': SHA, 'core_version': VERSION, 'runtime_mode': 'app-local',
            'binary_sha256': {name: hashlib.sha256((self.bundle / name).read_bytes()).hexdigest() for name in BINARIES},
        }
        self.runtime_path = self.bundle / 'WINDOWS-RUNTIME.json'
        self.write_runtime()
        self.archive = self.artifacts / f'{STEM}.zip'
        with zipfile.ZipFile(self.archive, 'w') as archive:
            for path in self.bundle.rglob('*'):
                if path.is_file():
                    archive.write(path, f'{STEM}/{path.relative_to(self.bundle).as_posix()}')
        self.msi = self.artifacts / f'{STEM}.msi'
        self.msi.write_bytes(b'INERT SYNTHETIC MSI. NEVER EXECUTE.\n')
        self.receipt_path = self.artifacts / f'{STEM}-BUILD.json'
        self.receipt = {
            'source_sha': SHA, 'runtime_version': VERSION, 'msi_product_version': '0.7.3',
            'files': [{'name': path.name, 'size': path.stat().st_size,
                       'sha256': hashlib.sha256(path.read_bytes()).hexdigest()} for path in (self.archive, self.msi)],
        }
        self.write_receipt()
        self.manifest = self.artifacts / f'{STEM}.sha256'
        self.manifest.write_text(''.join(f'{f["sha256"]}  {f["name"]}\n' for f in self.receipt['files']), encoding='utf-8')

    def write_runtime(self):
        self.runtime_path.write_text(json.dumps(self.runtime), encoding='utf-8')

    def write_receipt(self):
        self.receipt_path.write_text(json.dumps(self.receipt), encoding='utf-8')

    def validate_receipt(self):
        return helper('receipt', directory=str(self.artifacts), sha=SHA, version=VERSION)

    def validate_bundle(self):
        return helper('bundle', bundle=str(self.bundle), receipt=str(self.receipt_path), sha=SHA, version=VERSION)

    def assert_rejected(self, result, message):
        self.assertFalse(result['ok'], result)
        self.assertIn(message, result['error'])

    def test_valid_pair_is_bound_to_receipt_manifest_and_runtime_hashes(self):
        receipt = self.validate_receipt()
        self.assertTrue(receipt['ok'], receipt)
        self.assertEqual(receipt['value']['candidate'], str(self.msi))
        bundle = self.validate_bundle()
        self.assertTrue(bundle['ok'], bundle)
        self.assertEqual(set(bundle['value']), set(BINARIES))
        self.assertTrue(helper('archive', archive=str(self.archive), stem=STEM)['ok'])

    def test_short_windows_temp_path_preserves_relative_binary_names(self):
        import ctypes
        buffer = ctypes.create_unicode_buffer(32768)
        get_short = ctypes.windll.kernel32.GetShortPathNameW
        get_short.argtypes = (ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint)
        get_short.restype = ctypes.c_uint
        result = get_short(str(self.bundle), buffer, len(buffer))
        if not result:
            self.fail('GetShortPathNameW failed for the existing fixture bundle')
        short = buffer.value
        validated = helper('bundle', bundle=short, receipt=str(self.receipt_path), sha=SHA, version=VERSION)
        self.assertTrue(validated['ok'], validated)
        self.assertEqual(set(validated['value']), set(BINARIES))

    def test_wrong_receipt_source_runtime_and_numeric_version_are_rejected(self):
        for field, wrong, message in (('source_sha', 'b' * 40, 'source SHA mismatch'),
                                      ('runtime_version', VERSION + '-wrong', 'runtime version mismatch'),
                                      ('msi_product_version', '0.1.0', 'newer than fixture')):
            with self.subTest(field=field):
                original = self.receipt[field]
                self.receipt[field] = wrong
                self.write_receipt()
                self.assert_rejected(self.validate_receipt(), message)
                self.receipt[field] = original

    def test_actual_candidate_bytes_and_recorded_size_are_verified(self):
        self.msi.write_bytes(b'WRONG PRODUCED CANDIDATE')
        self.assert_rejected(self.validate_receipt(), 'size/SHA256 mismatch')
        self.receipt['files'][1]['size'] = self.msi.stat().st_size
        self.write_receipt()
        self.assert_rejected(self.validate_receipt(), 'size/SHA256 mismatch')

    def test_missing_duplicate_unsafe_and_nonintegral_receipt_files_are_rejected(self):
        original = list(self.receipt['files'])
        variants = (original[:1], original + [original[0]],
                    [dict(original[0], name='../outside.zip'), original[1]],
                    [dict(original[0], size=1.25), original[1]],
                    [dict(original[0], size=True), original[1]])
        for files in variants:
            with self.subTest(files=files):
                self.receipt['files'] = files
                self.write_receipt()
                self.assertFalse(self.validate_receipt()['ok'])

    def test_manifest_must_cover_candidate_and_disallow_duplicate_or_escaped_names(self):
        original = self.manifest.read_text(encoding='utf-8')
        variants = (original.splitlines()[0] + '\n', original + original,
                    original + 'f' * 64 + '  ../outside\n',
                    original.replace(self.receipt['files'][1]['sha256'], 'f' * 64))
        for text in variants:
            with self.subTest(manifest=text):
                self.manifest.write_text(text, encoding='utf-8')
                self.assertFalse(self.validate_receipt()['ok'])

    def test_runtime_requires_exact_source_version_app_local_mode_and_crt_hashes(self):
        for field, wrong, message in (('source_sha', 'b' * 40, 'source SHA mismatch'),
                                      ('core_version', 'wrong', 'runtime version mismatch'),
                                      ('runtime_mode', 'external-prerequisite', 'app-local')):
            with self.subTest(field=field):
                original = self.runtime[field]
                self.runtime[field] = wrong
                self.write_runtime()
                self.assert_rejected(self.validate_bundle(), message)
                self.runtime[field] = original
        del self.runtime['binary_sha256']['vcruntime140_1.dll']
        self.write_runtime()
        self.assert_rejected(self.validate_bundle(), 'missing a required binary hash')

    def test_mutated_bridge_and_crt_bytes_are_rejected(self):
        for name in ('xenterm_bridge.dll', 'msvcp140.dll'):
            with self.subTest(binary=name):
                path = self.bundle / name
                original = path.read_bytes()
                path.write_bytes(b'ALTERED INERT BYTES')
                self.assert_rejected(self.validate_bundle(), 'size/SHA256 mismatch')
                path.write_bytes(original)

    def test_additional_app_local_crt_requires_runtime_hash(self):
        del self.runtime['binary_sha256']['msvcp140_atomic_wait.dll']
        self.write_runtime()
        self.assert_rejected(self.validate_bundle(), 'App-local CRT hash is missing')

    def test_embedded_platform_runtime_cannot_disagree_with_portable(self):
        self.receipt['windows_runtime'] = json.loads(json.dumps(self.runtime))
        self.receipt['windows_runtime']['binary_sha256']['xenterm.exe'] = 'f' * 64
        self.write_receipt()
        self.assert_rejected(self.validate_bundle(), 'binary hashes disagree')

    def test_archive_rejects_traversal_other_root_profiles_duplicates_and_symlinks(self):
        for member in (f'{STEM}/../escape.dll', 'other-root/xenterm.exe',
                       f'{STEM}/config-flutter-next/sessions.db', f'{STEM}/config/sessions.db',
                       f'{STEM}/log/private.log', f'{STEM}/xenterm.exe'):
            with self.subTest(member=member):
                bad = self.directory / 'bad.zip'
                with warnings.catch_warnings():
                    warnings.simplefilter('ignore', UserWarning)
                    with zipfile.ZipFile(bad, 'w') as archive:
                        archive.writestr(f'{STEM}/xenterm.exe', 'inert')
                        archive.writestr(member, 'inert')
                self.assertFalse(helper('archive', archive=str(bad), stem=STEM)['ok'])
        bad = self.directory / 'symlink.zip'
        with zipfile.ZipFile(bad, 'w') as archive:
            link = zipfile.ZipInfo(f'{STEM}/xenterm.exe')
            link.create_system = 3
            link.external_attr = 0o120777 << 16
            archive.writestr(link, 'outside')
        self.assertFalse(helper('archive', archive=str(bad), stem=STEM)['ok'])


if __name__ == '__main__':
    unittest.main()
