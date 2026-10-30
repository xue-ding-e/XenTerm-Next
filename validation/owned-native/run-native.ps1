$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Set-Location $PSScriptRoot
$stage = 'source-identity'
$nativeExit = $null
$passed = $false
$initialIdentity = $false
$finalIdentity = $false
$failureType = $null
$identityFailureCode = $null
$initialIdentityFailureCode = $null
$finalIdentityFailureCode = $null
$identityPathSha256 = $null
$initialIdentityPathSha256 = $null
$finalIdentityPathSha256 = $null
$fixtureHash = $null
$harnessHash = $null
$manifestHash = $null
$receipt = $null
$metadataPreexisting = Test-Path -LiteralPath ./native-metadata
# The expected inventory is explicit, including this bootstrap and every build input.
$required = @('.github/workflows/ci.yml','Fixture/Fixture.csproj','Fixture/Program.cs','Harness/Harness.csproj','Harness/Program.cs','Harness/windows-owned-process-fault-fixture.cs','Harness/windows-owned-process.cs','NuGet.Config','global.json','run-native.ps1','test_preparation.py')
function Stop-Identity([string]$Code) {
    $script:identityFailureCode = $Code
    throw 'Source identity gate failed'
}
function Assert-SourceIdentity {
    param([bool]$AllowBuildOutputs = $false)
    $script:identityFailureCode = 'identity-read'
    $script:identityPathSha256 = $null
    if ((Get-FileHash ./source-hashes.json -Algorithm SHA256).Hash -ne $script:manifestHash) { Stop-Identity 'manifest-changed' }
    if ((Get-FileHash ../../.github/workflows/ci.yml -Algorithm SHA256).Hash -ne (Get-FileHash ./.github/workflows/ci.yml -Algorithm SHA256).Hash) { Stop-Identity 'workflow-mirror' }
    $inventory = Get-Content ./source-hashes.json -Raw | ConvertFrom-Json -AsHashtable
    if (@(Compare-Object ($script:required | Sort-Object) (@($inventory.Keys) | Sort-Object)).Count -ne 0) { Stop-Identity 'manifest-inventory' }
    $actual = @(Get-ChildItem -LiteralPath $PSScriptRoot -Recurse -Force -File | ForEach-Object {
        [IO.Path]::GetRelativePath($PSScriptRoot,$_.FullName).Replace('\','/')
    } | Where-Object { $_ -ne 'source-hashes.json' -and $_ -notmatch '^\.git/' -and (-not $AllowBuildOutputs -or ($_ -notmatch '^(Fixture/(bin|obj)/|Harness/(bin|obj)/|native-output/|native-metadata/)' -and $_ -notin @('fixture-build.log','harness-build.log'))) })
    if (@(Compare-Object ($script:required | Sort-Object) ($actual | Sort-Object)).Count -ne 0) { Stop-Identity 'source-inventory' }
    foreach ($item in $inventory.GetEnumerator()) {
        $script:identityPathSha256 = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($item.Key))).ToLowerInvariant()
        if ((Get-FileHash -LiteralPath $item.Key -Algorithm SHA256).Hash.ToLowerInvariant() -ne $item.Value) { Stop-Identity 'source-digest' }
    }
    $script:identityFailureCode = $null
    $script:identityPathSha256 = $null
}
try {
    if ($metadataPreexisting) { throw 'Refusing preexisting metadata' }
    if (-not $IsWindows) { throw 'Windows native validation required' }
    $manifestHash = (Get-FileHash ./source-hashes.json -Algorithm SHA256).Hash
    Assert-SourceIdentity
    $initialIdentity = $true
    # No SDK fallback downloads or package feeds. Missing .NET 8 is an environment failure.
    $stage = 'sdk-preflight'
    $sdks = & dotnet --list-sdks
    if ($LASTEXITCODE -ne 0 -or -not ($sdks -match '^8\.0\.')) { throw 'Installed .NET 8 SDK required' }
    # Compiler diagnostics remain local; bootstrap is not evidence of owned-job containment.
    $stage = 'fixture-compile'
    & dotnet build Fixture/Fixture.csproj -c Release "-p:RestoreConfigFile=$PSScriptRoot/NuGet.Config" -p:NuGetAudit=false -v:q *> fixture-build.log
    if ($LASTEXITCODE -ne 0) { throw 'Fixture compilation failed' }
    $stage = 'harness-compile'
    & dotnet build Harness/Harness.csproj -c Release "-p:RestoreConfigFile=$PSScriptRoot/NuGet.Config" -p:NuGetAudit=false -v:q *> harness-build.log
    if ($LASTEXITCODE -ne 0) { throw 'Harness compilation failed' }
    $fixture = (Resolve-Path ./Fixture/bin/Release/net8.0/Fixture.exe).Path
    $harness = (Resolve-Path ./Harness/bin/Release/net8.0/Harness.exe).Path
    $fixtureHash = (Get-FileHash $fixture).Hash.ToLowerInvariant()
    $harnessHash = (Get-FileHash $harness).Hash.ToLowerInvariant()
    $output = Join-Path $PSScriptRoot 'native-output'
    $stage = 'native-execution'
    & $harness $fixture $output
    $nativeExit = $LASTEXITCODE
    $receiptPath = Join-Path $output 'native-receipt.json'
    if (Test-Path -LiteralPath $receiptPath) {
        if ((Get-Item -LiteralPath $receiptPath).Length -gt 65536) { throw 'Oversized receipt' }
        $receipt = Get-Content $receiptPath -Raw | ConvertFrom-Json
    }
    if ($nativeExit -ne 0 -or $null -eq $receipt) { throw 'Native fixture failed or receipt missing' }
    $stage = 'receipt-validation'
    if (-not $receipt.passed -or -not $receipt.nativeFixtureExecuted -or $receipt.productExecuted -or $receipt.cases.Count -ne 10) { throw 'Native acceptance incomplete' }
    foreach ($case in $receipt.cases) {
        if (-not $case.passed) { throw 'Native case failed' }
        if ($case.actualExecutions -gt 0 -and (-not $case.treeEmpty -or $case.actualExecutions -ne 3)) { throw 'Missing executed tree evidence' }
    }
    $passed = $true
} catch {
    # No exception messages, compiler diagnostics, process output, or environment values are exported.
    $failureType = $_.Exception.GetType().Name
    if (-not $initialIdentity) { $initialIdentityFailureCode = $identityFailureCode; $initialIdentityPathSha256 = $identityPathSha256 }
} finally {
    try { Assert-SourceIdentity -AllowBuildOutputs $true; $finalIdentity = $true } catch {
        $finalIdentityFailureCode = $identityFailureCode
        $finalIdentityPathSha256 = $identityPathSha256
        $passed = $false
        if ($null -eq $failureType) { $failureType = $_.Exception.GetType().Name; $stage = 'final-source-identity' }
    }
    if (-not $metadataPreexisting) {
    New-Item -ItemType Directory -Path ./native-metadata | Out-Null
    # A source-identity failure suppresses the harness receipt altogether.
    if ($initialIdentity -and $finalIdentity -and $null -ne $receipt) {
        $receipt | ConvertTo-Json -Depth 8 | Set-Content native-metadata/native-receipt.json -Encoding utf8NoBOM
    }
    @{schema=1;passed=$passed;stage=$stage;failureType=$failureType;initialIdentityPathSha256=$initialIdentityPathSha256;finalIdentityPathSha256=$finalIdentityPathSha256;initialIdentityFailureCode=$initialIdentityFailureCode;finalIdentityFailureCode=$finalIdentityFailureCode;initialSourceIdentityVerified=$initialIdentity;finalSourceIdentityVerified=$finalIdentity;fixtureSha256=$fixtureHash;harnessSha256=$harnessHash;controlSha=$env:GITHUB_SHA;nativeExit=$nativeExit;productExecuted=$false} | ConvertTo-Json | Set-Content native-metadata/provenance.json -Encoding utf8NoBOM
    if ($env:GITHUB_OUTPUT) { 'metadata_ready=true' | Add-Content -LiteralPath $env:GITHUB_OUTPUT }
    }
}
if (-not $passed) { exit 1 }
