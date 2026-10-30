# Source candidate only; the caller supplies a bounded hosted Windows step.
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$root=Join-Path $env:RUNNER_TEMP 'key-import-compile-evidence'
$build=Join-Path $root 'host-build'
$recordPath=Join-Path $root 'host-build.json'
$required=@('Host.csproj','Program.cs','windows-owned-process.cs','NuGet.Config','global.json')
$state='not-started';$exitCode=$null;$initial=$false;$final=$false;$hashes=@{};$sources=@{};$codes=@();$ownedOutput=$false;$copiedInputs=$false
function Plain-Path([string]$Path) {
    if(-not [IO.Path]::IsPathFullyQualified($Path) -or [IO.Path]::GetFullPath($Path) -ne $Path){throw 'Noncanonical path'}
    $p=$Path
    while($p){if(Test-Path -LiteralPath $p){if(((Get-Item -LiteralPath $p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'Reparse path'}};$p=[IO.Path]::GetDirectoryName($p)}
}
function Verify-Control {
    if($env:EXPECTED_CONTROL_SHA -notmatch '^[0-9a-f]{40}$' -or $env:GITHUB_SHA -ne $env:EXPECTED_CONTROL_SHA -or $env:GITHUB_RUN_ID -notmatch '^[0-9]+$'){throw 'Control run identity'}
    if([IO.Path]::GetFullPath($PSScriptRoot) -ne [IO.Path]::GetFullPath((Join-Path $env:GITHUB_WORKSPACE 'validation/key-import-compile'))){throw 'Unexpected build producer location'}
    $head=& git -C $env:GITHUB_WORKSPACE rev-parse HEAD
    if($LASTEXITCODE -ne 0 -or $head -ne $env:EXPECTED_CONTROL_SHA){throw 'Control head mismatch'}
    $status=& git -C $env:GITHUB_WORKSPACE status --porcelain --untracked-files=all
    if($LASTEXITCODE -ne 0 -or $status){throw 'Control mutated or untracked'}
    $actual=@(Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'Host') -Recurse -File -Force | ForEach-Object {[IO.Path]::GetRelativePath((Join-Path $PSScriptRoot 'Host'),$_.FullName).Replace('\','/')})
    if(@(Compare-Object ($required|Sort-Object) ($actual|Sort-Object)).Count -ne 0){throw 'Host source inventory'}
    foreach($name in $required){$path=Join-Path $PSScriptRoot ('Host/'+$name);Plain-Path $path;$h=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant();if($script:sources.ContainsKey('Host/'+$name)){if($script:sources['Host/'+$name] -ne $h){throw 'Host source changed'}}else{$script:sources['Host/'+$name]=$h}}
    if($sources['Host/windows-owned-process.cs'] -ne '62f2b8a074e57524840f5f56134fdf63eccb6079f9d3c199e5a6e5c372728dbb'){throw 'Unqualified transport'}
}
function Verify-BuildCopy {
    foreach($name in $required){
        $path=Join-Path $build $name;Plain-Path $path
        if((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sources['Host/'+$name]){throw 'Compiled input copy changed'}
    }
    $actual=@(Get-ChildItem -LiteralPath $build -Recurse -File -Force | ForEach-Object {[IO.Path]::GetRelativePath($build,$_.FullName).Replace('\','/')} | Where-Object {$_ -notmatch '^(bin|obj)/' -and $_ -ne 'compiler.log'})
    if(@(Compare-Object ($required|Sort-Object) ($actual|Sort-Object)).Count -ne 0){throw 'Unexpected compiled input'}
}

try {
    if(-not $IsWindows -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted'){throw 'Disposable hosted Windows required'}
    Plain-Path $root
    if(-not(Test-Path -LiteralPath $root -PathType Container) -or (Test-Path -LiteralPath $build) -or (Test-Path -LiteralPath $recordPath)){throw 'Output ownership'}
    New-Item -ItemType Directory -Path $build | Out-Null;$ownedOutput=$true
    Verify-Control;$initial=$true
    foreach($name in $required){Copy-Item -LiteralPath (Join-Path $PSScriptRoot ('Host/'+$name)) -Destination (Join-Path $build $name)}
    $copiedInputs=$true;Verify-BuildCopy
    Push-Location $build
    try {
        $state='building'
        # Installed SDK only; clear feeds, no ancestor Directory.Build imports.
        & dotnet build Host.csproj -c Release "-p:RestoreConfigFile=$build/NuGet.Config" -p:NuGetAudit=false -p:ImportDirectoryBuildProps=false -p:ImportDirectoryBuildTargets=false -v:q *> (Join-Path $build 'compiler.log')
        $exitCode=$LASTEXITCODE
        if($exitCode -ne 0){throw 'Host compilation failed'}
    } finally {Pop-Location}
    foreach($name in @('CompileOnlyHost.exe','CompileOnlyHost.dll','CompileOnlyHost.deps.json','CompileOnlyHost.runtimeconfig.json')){$path=Join-Path $build ('bin/Release/net8.0/'+$name);Plain-Path $path;$hashes[$name]=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()}
    Verify-BuildCopy
    $state='compiled'
} catch {$state='failed'} finally {
    try {Verify-Control;if($copiedInputs){Verify-BuildCopy};$final=$true}catch{$state='identity-failed'}
    if($ownedOutput) {
        if($state -ne 'compiled') {
            try {$log=Join-Path $build 'compiler.log';if((Get-Item $log).Length -gt 16777216){throw 'oversized'};$counts=@{};foreach($line in [IO.File]::ReadLines($log)){foreach($m in [regex]::Matches($line,'\berror\s+(CS[0-9]{4}|MSB[0-9]{4}|NU[0-9]{4})\b')){$k=$m.Groups[1].Value;if(-not $counts.ContainsKey($k)){if($counts.Count -ge 32){throw 'too many codes'};$counts[$k]=0};$counts[$k]=[Math]::Min(1000,$counts[$k]+1)}};$codes=@($counts.Keys|Sort-Object|ForEach-Object{@{code=$_;count=$counts[$_]}})}catch{$codes=@()}
        }
        # No log/output/environment content is exported. This is build evidence,
        # not a claim that the new Host has already passed native execution tests.
        @{state=$state;compile_exit=$exitCode;initial_identity=$initial;final_identity=$final;control_sha=$env:EXPECTED_CONTROL_SHA;run_id=$env:GITHUB_RUN_ID;host_sources=$sources;compiled_copy_identity=($copiedInputs -and $final);files=$hashes;compiler_codes=$codes;product_compile_attempted=$false;product_runtime_attempted=$false} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $recordPath -Encoding utf8NoBOM
    }
}
if($state -ne 'compiled' -or -not($initial -and $final)){exit 1}
