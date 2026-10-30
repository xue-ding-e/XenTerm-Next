# Preparation only. Hosted job/step limits must be reviewed before any execution.
# No Cargo, product test binary, release, installer, or external package feeds.
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
Set-Location $PSScriptRoot
$passed=$false;$initial=$false;$final=$false;$stage='identity';$failureType=$null
$results=@{};$compilerCodes=@();$currentLog=$null;$manifestHash=$null
$metadata=$null;$createdMetadata=$false
$required=@('.github/workflows/ci.yml','NuGet.Config','global.json','run-combined.ps1','Fixture/Program.cs','Fixture/Fixture.csproj','Harness/Program.cs','Harness/Harness.csproj','Harness/windows-owned-process.cs','Harness/short-clock-owned-process.cs','Default15/Fixture/Program.cs','Default15/Fixture/Fixture.csproj','Default15/Harness/Program.cs','Default15/Harness/Harness.csproj','Default15/Harness/windows-owned-process.cs','Default15/Harness/windows-owned-process-fault-fixture.cs')
function Assert-Identity {
    if((Get-FileHash source-hashes.json -Algorithm SHA256).Hash -ne $script:manifestHash){throw 'manifest changed'}
    if((Get-FileHash (Join-Path $env:GITHUB_WORKSPACE '.github/workflows/ci.yml') -Algorithm SHA256).Hash -ne (Get-FileHash .github/workflows/ci.yml -Algorithm SHA256).Hash){throw 'workflow identity mismatch'}
    $map=Get-Content source-hashes.json -Raw | ConvertFrom-Json -AsHashtable
    if(@(Compare-Object ($script:required|Sort-Object) (@($map.Keys)|Sort-Object)).Count -ne 0){throw 'inventory mismatch'}
    $actual=@(Get-ChildItem -LiteralPath $PSScriptRoot -Recurse -Force -File | ForEach-Object { [IO.Path]::GetRelativePath($PSScriptRoot,$_.FullName).Replace('\','/') } | Where-Object { $_ -ne 'source-hashes.json' })
    if(@(Compare-Object ($script:required|Sort-Object) ($actual|Sort-Object)).Count -ne 0){throw 'unexpected build input'}
    foreach($name in $script:required){if((Get-FileHash -LiteralPath $name -Algorithm SHA256).Hash.ToLowerInvariant() -ne $map[$name]){throw 'source changed'}}
}
function Build-Fixture([string]$Project,[string]$Name,[string]$Exe) {
    $script:stage='build-'+$Name
    $script:currentLog=Join-Path $script:metadata ($Name+'-build.log')
    $bin=Join-Path $script:metadata ($Name+'/bin/')
    $obj=Join-Path $script:metadata ($Name+'/obj/')
    & dotnet build $Project -c Release "-p:RestoreConfigFile=$PSScriptRoot/NuGet.Config" '-p:NuGetAudit=false' '-p:ImportDirectoryBuildProps=false' '-p:ImportDirectoryBuildTargets=false' "-p:BaseOutputPath=$bin" "-p:BaseIntermediateOutputPath=$obj" -v:q *> $script:currentLog
    if($LASTEXITCODE -ne 0){throw 'synthetic compilation failed'}
    $path=Join-Path $bin ('Release/net8.0/'+$Exe)
    if(-not(Test-Path -LiteralPath $path -PathType Leaf)){throw 'synthetic artifact missing'}
    return $path
}
function Get-ExecutionIdentity([string]$Exe) {
    $base=[IO.Path]::GetFileNameWithoutExtension($Exe)
    $directory=[IO.Path]::GetDirectoryName($Exe)
    $map=@{}
    foreach($name in @("$base.exe","$base.dll","$base.deps.json","$base.runtimeconfig.json")) {
        $path=Join-Path $directory $name
        if(-not(Test-Path -LiteralPath $path -PathType Leaf)){throw 'runtime file missing'}
        $map[$name]=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    return $map
}
function Assert-ExecutionIdentity([string]$Exe,$Expected) {
    $actual=Get-ExecutionIdentity $Exe
    foreach($name in $Expected.Keys){if($actual[$name] -ne $Expected[$name]){throw 'runtime file changed'}}
}
try {
    if(-not $IsWindows){throw 'Windows required'}
    if($env:GITHUB_SHA -notmatch '^[0-9a-f]{40}$'){throw 'control identity absent'}
    $metadata=Join-Path $env:RUNNER_TEMP ('xenterm-compile-qualification-'+$env:GITHUB_SHA)
    if(Test-Path -LiteralPath $metadata){throw 'output preexists'}
    New-Item -ItemType Directory -Path $metadata | Out-Null;$createdMetadata=$true
    $manifestHash=(Get-FileHash source-hashes.json -Algorithm SHA256).Hash
    Assert-Identity;$initial=$true
    $stage='sdk-preflight';$sdks=& dotnet --list-sdks
    if($LASTEXITCODE -ne 0 -or -not($sdks -match '^8\.0\.')){throw 'installed SDK8 required'}
    $defaultFixture=Build-Fixture 'Default15/Fixture/Fixture.csproj' 'default-fixture' 'Fixture.exe'
    $defaultHarness=Build-Fixture 'Default15/Harness/Harness.csproj' 'default-harness' 'Harness.exe'
    $compileFixture=Build-Fixture 'Fixture/Fixture.csproj' 'compile-fixture' 'cargo.exe'
    $compileHarness=Build-Fixture 'Harness/Harness.csproj' 'compile-harness' 'CompilePolicyHarness.exe'
    foreach($suite in @('default15','compile7')) {
        $stage='native-'+$suite;$output=Join-Path $metadata $suite
        $harness=if($suite -eq 'default15'){$defaultHarness}else{$compileHarness}
        $fixture=if($suite -eq 'default15'){$defaultFixture}else{$compileFixture}
        $harnessFiles=Get-ExecutionIdentity $harness
        $fixtureFiles=Get-ExecutionIdentity $fixture
        try { & $harness $fixture $output; $code=$LASTEXITCODE } finally {
            Assert-ExecutionIdentity $harness $harnessFiles
            Assert-ExecutionIdentity $fixture $fixtureFiles
        }
        $name=if($suite -eq 'default15'){'native-receipt.json'}else{'native-result.json'}
        $receiptPath=Join-Path $output $name
        if(-not(Test-Path -LiteralPath $receiptPath) -or (Get-Item -LiteralPath $receiptPath).Length -gt 65536){throw 'receipt missing or oversized'}
        $r=Get-Content $receiptPath -Raw | ConvertFrom-Json
        $results[$suite]=@{exit=$code;receipt=$r;fixtureFiles=$fixtureFiles;harnessFiles=$harnessFiles;controlSha=$env:GITHUB_SHA;runId=$env:GITHUB_RUN_ID}
        $count=if($suite -eq 'default15'){15}else{7}
        if($code -ne 0 -or -not $r.passed -or $r.cases.Count -ne $count){throw 'native suite failed'}
        foreach($case in $r.cases){if(-not $case.passed -or ($case.actualExecutions -gt 0 -and ($case.actualExecutions -ne 3 -or -not $case.treeEmpty))){throw 'case evidence missing'}}
    }
    $passed=$true
} catch {$failureType=$_.Exception.GetType().Name} finally {
    if($stage.StartsWith('build-') -and $currentLog -and (Test-Path -LiteralPath $currentLog)) {
        try {
            if((Get-Item $currentLog).Length -gt 16777216){throw 'diagnostic oversized'}
            $counts=@{}
            foreach($line in [IO.File]::ReadLines($currentLog)){foreach($m in [regex]::Matches($line,'\berror\s+(CS[0-9]{4}|MSB[0-9]{4}|NU[0-9]{4})\b')){$key=$m.Groups[1].Value;if(-not $counts.ContainsKey($key)){if($counts.Count -ge 32){throw 'code bound'};$counts[$key]=0};$counts[$key]=[Math]::Min(1000,$counts[$key]+1)}}
            $compilerCodes=@($counts.Keys|Sort-Object|ForEach-Object{@{code=$_;count=$counts[$_]}})
        } catch {$compilerCodes=@()}
    }
    try{Assert-Identity;$final=$true}catch{$passed=$false;if(-not $failureType){$failureType=$_.Exception.GetType().Name}}
    if($createdMetadata -and (Test-Path -LiteralPath $metadata)) {
        if(-not($initial -and $final)){$results=@{}}
        @{passed=$passed;stage=$stage;failureType=$failureType;initialIdentity=$initial;finalIdentity=$final;compilerCodes=$compilerCodes;suites=$results;controlSha=$env:GITHUB_SHA;runId=$env:GITHUB_RUN_ID;sourceManifestSha256=$manifestHash;productCompileAttempted=$false;productRuntimeAttempted=$false;full45MinuteElapsedValidation=$false} | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $metadata 'metadata.json') -Encoding utf8NoBOM
    }
}
if(-not $passed){exit 1}
