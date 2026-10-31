# Test-only preparation, no workflow/dispatch. Do not call without separate approval.
param([Parameter(Mandatory)][string]$OutputRoot)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
if(-not $IsWindows){throw 'Windows required'}
function Plain([string]$Path) {
    if(-not [IO.Path]::IsPathFullyQualified($Path) -or [IO.Path]::GetFullPath($Path) -ne $Path){throw 'canonical path required'}
    $p=$Path
    while($p){if(Test-Path -LiteralPath $p){if(((Get-Item -LiteralPath $p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'reparse input'}};$p=[IO.Path]::GetDirectoryName($p)}
}
Plain $OutputRoot
if(Test-Path -LiteralPath $OutputRoot){throw 'new output root required'}
$production=Join-Path (Split-Path -Parent $PSScriptRoot) 'fetch-native/Host'
$fixed=@{
    'Program.cs'='0f94cd746e8a04d7bb5e794683a30b7512d406319eccb3e3c4e6fb3d79e29ae3';
    'fetch-owned-process.cs'='d6d9d3b5c31a2f697eb435cdbf99f0fc968e2af912b069923bc6f8c7497331b3';
    'FetchHost.csproj'='fdcf2d82f51d9986e855c388eeafb0a67b186ffbfc31a0bf47fab47ba52defa9';
    'NuGet.Config'='9508cae5f76364f56e80eb97cb16493977243cf38e5cca6284e3d74f83b20a2c';
    'global.json'='0503dacfc3e664e4b7fc0ac7da920e5939da4ee62ab852ce7aec1a0bdbdf38a2'
}
function Check-Production {
    foreach($name in $fixed.Keys){$p=Join-Path $production $name;Plain $p;if((Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant() -ne $fixed[$name]){throw 'production source changed'}}
}
Check-Production
$local=@('Fixture/Fixture.csproj','Fixture/Program.cs','Harness/Harness.csproj','Harness/Program.cs','run-host-entry.ps1')
$before=@{}
foreach($name in $local){$p=Join-Path $PSScriptRoot $name;Plain $p;$before[$name]=(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash}
[IO.Directory]::CreateDirectory($OutputRoot)|Out-Null
$work=Join-Path $OutputRoot 'build';[IO.Directory]::CreateDirectory($work)|Out-Null
foreach($part in @('Host','Fixture','Harness')){[IO.Directory]::CreateDirectory((Join-Path $work $part))|Out-Null}
foreach($name in $fixed.Keys){Copy-Item -LiteralPath (Join-Path $production $name) -Destination (Join-Path $work ('Host/'+$name))}
foreach($name in $local | Where-Object {$_ -ne 'run-host-entry.ps1'}){Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination (Join-Path $work $name)}
foreach($name in @('NuGet.Config','global.json')){Copy-Item -LiteralPath (Join-Path $production $name) -Destination (Join-Path $work $name)}
function Check-Copies {
    foreach($name in $fixed.Keys){if((Get-FileHash -LiteralPath (Join-Path $work ('Host/'+$name)) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $fixed[$name]){throw 'production copy changed'}}
    foreach($name in $local | Where-Object {$_ -ne 'run-host-entry.ps1'}){if((Get-FileHash -LiteralPath (Join-Path $work $name) -Algorithm SHA256).Hash -ne $before[$name]){throw 'synthetic copy changed'}}
    foreach($name in @('NuGet.Config','global.json')){if((Get-FileHash -LiteralPath (Join-Path $work $name) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $fixed[$name]){throw 'build configuration changed'}}
}
function Build([string]$Project,[string]$Name,[string]$Exe) {
    $bin=Join-Path $OutputRoot ($Name+'/bin/');$obj=Join-Path $OutputRoot ($Name+'/obj/')
    $info=[Diagnostics.ProcessStartInfo]::new((Get-Command dotnet -CommandType Application).Source)
    $info.WorkingDirectory=$work
    $info.UseShellExecute=$false;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
    foreach($arg in @('build',$Project,'-c','Release',"-p:RestoreConfigFile=$work/NuGet.Config",'-p:NuGetAudit=false','-p:ImportDirectoryBuildProps=false','-p:ImportDirectoryBuildTargets=false',"-p:BaseOutputPath=$bin","-p:BaseIntermediateOutputPath=$obj",'-v:q')){$info.ArgumentList.Add($arg)}
    $out=[IO.File]::Open((Join-Path $OutputRoot ($Name+'-build.stdout.log')),[IO.FileMode]::CreateNew)
    $err=[IO.File]::Open((Join-Path $OutputRoot ($Name+'-build.stderr.log')),[IO.FileMode]::CreateNew)
    $process=$null
    try {
        $process=[Diagnostics.Process]::Start($info)
        $copies=[Threading.Tasks.Task[]]@($process.StandardOutput.BaseStream.CopyToAsync($out),$process.StandardError.BaseStream.CopyToAsync($err))
        if(-not $process.WaitForExit(60000)){
            try{$process.Kill($true);$null=$process.WaitForExit(5000)}finally{throw 'synthetic build timeout; lifecycle unknown'}
        }
        if(-not [Threading.Tasks.Task]::WaitAll($copies,5000)){throw 'synthetic build log completion unknown'}
        if($process.ExitCode -ne 0 -or $out.Length+$err.Length -gt 16777216){throw 'test-only build failed or diagnostic oversized'}
    } finally {if($process){$process.Dispose()};$out.Dispose();$err.Dispose()}
    return (Join-Path $bin ('Release/net8.0/'+$Exe))
}
$passed=$false
$runtime=@{}
Push-Location $work
try {
    Check-Copies
    $fetchHost=Build 'Host/FetchHost.csproj' 'host' 'FetchHost.exe'
    $fixture=Build 'Fixture/Fixture.csproj' 'fixture' 'cargo.exe'
    $harness=Build 'Harness/Harness.csproj' 'harness' 'FetchHostEntryHarness.exe'
    Check-Copies
    foreach($exe in @($fetchHost,$fixture,$harness)) {
        $base=[IO.Path]::GetFileNameWithoutExtension($exe);$dir=[IO.Path]::GetDirectoryName($exe)
        foreach($suffix in @('.exe','.dll','.deps.json','.runtimeconfig.json')) {
            $p=Join-Path $dir ($base+$suffix);Plain $p
            $runtime[$p]=(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash
        }
    }
    # Only the synthetic harness runs. It invokes the original Host entry with
    # synthetic cargo, never the installed Rust toolchain or real Cargo packages.
    $info=[Diagnostics.ProcessStartInfo]::new($harness);$info.UseShellExecute=$false;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
    foreach($arg in @($fetchHost,$fixture,(Join-Path $OutputRoot 'cases'))){$info.ArgumentList.Add($arg)}
    $process=[Diagnostics.Process]::Start($info)
    try {
        $drain=[Threading.Tasks.Task[]]@($process.StandardOutput.BaseStream.CopyToAsync([IO.Stream]::Null),$process.StandardError.BaseStream.CopyToAsync([IO.Stream]::Null))
        if(-not $process.WaitForExit(90000)){
            try{$process.Kill($true);$null=$process.WaitForExit(5000)}finally{throw 'synthetic harness timeout; lifecycle unknown'}
        }
        if(-not [Threading.Tasks.Task]::WaitAll($drain,5000)){throw 'synthetic harness output lifecycle unknown'}
        if($process.ExitCode -ne 0){throw 'synthetic Host suite failed'}
    } finally {$process.Dispose()}
    $result=Get-Content -LiteralPath (Join-Path $OutputRoot 'cases/host-entry-result.json') -Raw | ConvertFrom-Json
    if(-not $result.passed -or $result.case_count -ne 13 -or $result.cases.Count -ne 13){throw 'synthetic Host evidence incomplete'}
    $passed=$true
} finally {
    try {
        Check-Production;Check-Copies
        foreach($p in $runtime.Keys){Plain $p;if((Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash -ne $runtime[$p]){throw 'test runtime identity changed'}}
        foreach($name in $local){if((Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $name) -Algorithm SHA256).Hash -ne $before[$name]){throw 'preparation source changed'}}
    } finally {Pop-Location}
}
if(-not $passed){exit 1}
