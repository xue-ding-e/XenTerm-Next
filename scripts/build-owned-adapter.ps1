$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Windows host only' }
$control = Split-Path $PSScriptRoot -Parent
$out = Join-Path $env:RUNNER_TEMP 'key-import-native-evidence'
$prepared = Get-Content (Join-Path $out 'prepared.json') -Raw | ConvertFrom-Json -AsHashtable
if ($prepared.control_sha -ne $env:EXPECTED_CONTROL_SHA -or $env:GITHUB_SHA -ne $env:EXPECTED_CONTROL_SHA) { throw 'Control identity mismatch' }
function Verify-Control {
    foreach ($entry in $prepared.control_files.GetEnumerator()) {
        if ((Get-FileHash (Join-Path $control $entry.Key) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $entry.Value) { throw 'Control file changed' }
    }
}
Verify-Control
$transport = '3d5a75d1baa1c90a843f26cb47a5a5a7c9328fbc73eb992326495cf23d6aaaef'
if ((Get-FileHash (Join-Path $PSScriptRoot 'owned_adapter/windows-owned-process.cs')).Hash.ToLowerInvariant() -ne $transport) { throw 'Unqualified transport source' }
$build = Join-Path $out 'owned-adapter-build'
if (Test-Path $build) { throw 'Adapter build output exists' }
New-Item -ItemType Directory $build | Out-Null
foreach ($name in @('OwnedAdapter.csproj','Program.cs','windows-owned-process.cs','NuGet.Config','global.json')) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot "owned_adapter/$name") -Destination (Join-Path $build $name)
}
function Get-CompilerCodeCounts([string]$Path) {
    # Local compiler diagnostics never leave the runner; only allowlisted code counts do.
    if (-not (Test-Path -LiteralPath $Path) -or (Get-Item -LiteralPath $Path).Length -gt 16777216) { throw 'Diagnostic unavailable or oversized' }
    $counts = @{}
    foreach ($line in [IO.File]::ReadLines($Path)) {
        foreach ($match in [regex]::Matches($line,'\berror\s+(CS[0-9]{4}|MSB[0-9]{4}|NU[0-9]{4})\b')) {
            $code = $match.Groups[1].Value
            if (-not $counts.ContainsKey($code)) {
                if ($counts.Count -ge 32) { throw 'Too many diagnostic codes' }
                $counts[$code] = 0
            }
            $counts[$code] = [Math]::Min(1000,$counts[$code]+1)
        }
    }
    foreach ($code in @($counts.Keys | Sort-Object)) { @{code=$code;count=$counts[$code]} }
}
$compilerCodes = @()
$compilerCodeExtraction = 'not-needed'
$compileExit = $null
$code = 1
$stage = 'build'
try {
    Set-Location $build
    & dotnet build OwnedAdapter.csproj -c Release "-p:RestoreConfigFile=$build/NuGet.Config" -p:NuGetAudit=false -v:q *> (Join-Path $out 'owned-adapter-build.log')
    $compileExit = $LASTEXITCODE
    if ($compileExit -ne 0) { throw 'Owned adapter compilation failed' }
    $publish = Join-Path $build 'bin/Release/net8.0'
    $hashes = @{}
    foreach ($name in @('OwnedAdapter.exe','OwnedAdapter.dll','OwnedAdapter.deps.json','OwnedAdapter.runtimeconfig.json')) {
        $hashes[$name] = (Get-FileHash (Join-Path $publish $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    Verify-Control
    @{directory=$publish;artifacts=$hashes;transport_sha256=$transport;control_sha=$env:EXPECTED_CONTROL_SHA} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $out 'owned-adapter-local.json') -Encoding utf8NoBOM
    $code = 0
    $stage = 'compiled'
} catch { $stage = 'build-failed' } finally {
    if ($stage -eq 'build-failed') {
        try { $compilerCodes = @(Get-CompilerCodeCounts (Join-Path $out 'owned-adapter-build.log')); $compilerCodeExtraction = 'codes-only' } catch { $compilerCodeExtraction = 'unavailable' }
    }
    try { Verify-Control } catch { $code=1;$stage='identity-failed' }
    @{compileExit=$compileExit;compilerCodes=$compilerCodes;compilerCodeExtraction=$compilerCodeExtraction;state=$stage;passed=($code -eq 0);transport_sha256=$transport;control_sha=$env:EXPECTED_CONTROL_SHA;product_compile_attempted=$false;raw_logs_uploaded=$false} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $out 'owned-adapter-build.json') -Encoding utf8NoBOM
}
exit $code
