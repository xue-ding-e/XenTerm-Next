param([Parameter(Mandatory=$true)][string]$SourceDir,[Parameter(Mandatory=$true)][string]$EvidenceDir)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Set-Location -LiteralPath $SourceDir
$report=[ordered]@{status='failed';sha=$null;src_tree=$null;reason=$null;product_source_sha='88536f15dfb4fd18057631d450ae48d561f9b2bb';tested_baseline_sha='461fd85774ae8e98c0bcf6e788226c8d52c32b58'}
$expectedObjects=[ordered]@{
    'src'='a8f4de26c9c0b0782b5cce12e629133664ce5a5d'
    'Cargo.toml'='6646afc7299b2f533d9fdf4ea028f70866a2712f'
    'Cargo.lock'='b233737afbdb2126bb7be9bdb90360a5e961619a'
    'build.rs'='71fab7fb90ffab62e7111ef5b0d256976b6bd197'
    'assets'='9fea0ceab930f971e8295a6cf37380ac9cc649e1'
    'scripts/version.sh'='4b04370a93e8245a37895fb487ac9eef7cb1d4b7'
}
try {
    $report.sha=(& git rev-parse HEAD | Out-String).Trim(); if ($LASTEXITCODE -ne 0) { throw 'Cannot read candidate HEAD' }
    $report.src_tree=(& git rev-parse HEAD:src | Out-String).Trim(); if ($LASTEXITCODE -ne 0) { throw 'Cannot read src tree' }
    if ($env:GITHUB_SHA -notmatch '^[0-9a-f]{40}$' -or $report.sha -ne $env:GITHUB_SHA) { throw 'Checkout differs from exact workflow run SHA' }
    $report.build_input_objects=[ordered]@{}
    foreach ($path in $expectedObjects.Keys) {
        $actual=(& git rev-parse ('HEAD:'+ $path) | Out-String).Trim()
        if ($LASTEXITCODE -ne 0 -or $actual -ne $expectedObjects[$path]) { throw ('Product build input mismatch: '+$path) }
        $report.build_input_objects[$path]=$actual
    }
    # The harness is additive, not a replacement Cargo configuration or compiler wrapper.
    if (Test-Path -LiteralPath '.cargo') { throw 'Unexpected repository Cargo configuration' }
    if ($env:RUSTC_WRAPPER -or $env:RUSTC_WORKSPACE_WRAPPER -or $env:XT_VERSION -or $env:XT_VERSION_JSON) { throw 'Unexpected compiler/version override' }
    $dirty=(& git status --porcelain --untracked-files=all | Out-String).Trim(); if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Candidate checkout must be clean' }
    $report.cargo_lock_sha256=(Get-FileHash -LiteralPath Cargo.lock -Algorithm SHA256).Hash.ToLowerInvariant()
    $report.rustc=(& rustc +stable -Vv | Out-String).Trim(); if ($LASTEXITCODE -ne 0) { throw 'rustc metadata failed' }
    $report.cargo=(& cargo +stable -V | Out-String).Trim(); if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $report.command='cargo +stable build --locked --target x86_64-pc-windows-msvc --jobs 2 --bin xenterm'
    # MSVC environment comes from the same official action already used by the repository.
    $report.msvc_cl=(& where.exe cl.exe | Out-String).Trim(); if ($LASTEXITCODE -ne 0) { throw 'MSVC compiler unavailable' }
    # Do not aggregate ReadToEndAsync into RAM until exit: a job timeout would
    # lose all build diagnostics. Pure .NET line callbacks flush each stream live.
    # This worker and cargo inherit the existing 2400-second owning Job Object.
    $report.log_files=@('build.stdout.log','build.stderr.log')
    $report.status='building'
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $EvidenceDir 'build-metadata.json') -Encoding UTF8
    $report.cargo_exit_code=[XenTermNativeProbe.ProcessLogs]::Run(
        (Get-Command cargo -CommandType Application).Source,
        '+stable build --locked --target x86_64-pc-windows-msvc --jobs 2 --bin xenterm',
        $SourceDir,(Join-Path $EvidenceDir 'build.stdout.log'),(Join-Path $EvidenceDir 'build.stderr.log'))
    if ($report.cargo_exit_code -ne 0) { throw ('cargo build returned '+$report.cargo_exit_code) }
    $dirty=(& git status --porcelain --untracked-files=all | Out-String).Trim(); if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Candidate checkout changed during build' }
    $exe=Join-Path $SourceDir 'target\x86_64-pc-windows-msvc\debug\xenterm.exe'
    $report.binary_sha256=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
    $report.binary_bytes=(Get-Item -LiteralPath $exe).Length
    if (-not (Test-Path -LiteralPath (Join-Path $SourceDir 'version.json'))) { throw 'Build completed but scripts/version.sh metadata version.json is absent' }
    $report.version_metadata=Get-Content -Raw -LiteralPath (Join-Path $SourceDir 'version.json') | ConvertFrom-Json
    $report.status='passed'
} catch { $report.status='failed'; $report.reason=$_.Exception.Message }
finally { $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $EvidenceDir 'build-metadata.json') -Encoding UTF8 }
if ($report.status -ne 'passed') { exit 1 }
exit 0
