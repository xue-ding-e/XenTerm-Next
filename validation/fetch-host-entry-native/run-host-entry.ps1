# Manual-only synthetic qualification. Never invokes real Cargo or product code.
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$passed=$false;$initial=$false;$final=$false;$created=$false;$copied=$false
$stage='scope';$failureCase=$null;$manifestHash=$null
$primaryFailure=$null;$finalFailure=$null;$checkpoint='SCOPE';$inFinal=$false
$directoryProbeChecks=0;$nugetIsolation=@{migrationMarkerAtExpectedRoot=$null;markerBytes=$null}
$buildOutcomes=[Collections.Generic.List[object]]::new();$copyChecks=[Collections.Generic.List[object]]::new()
$runtime=@{};$runtimeEvidence=@{};$dotnetHash=$null;$cases=@();$result=$null;$harnessStarted=$false
$OutputRoot=$null;$work=$null
$required=@('Host/Program.cs','Host/fetch-owned-process.cs','Host/FetchHost.csproj','Host/NuGet.Config','Host/global.json','Fixture/Program.cs','Fixture/Fixture.csproj','Harness/Program.cs','Harness/Harness.csproj','NuGet.Config','global.json','run-host-entry.ps1','.github/workflows/ci.yml','expected-cases.json','test_host_entry_preparation.py')
$compileInputs=@('Host/Program.cs','Host/fetch-owned-process.cs','Host/FetchHost.csproj','Host/NuGet.Config','Host/global.json','Fixture/Program.cs','Fixture/Fixture.csproj','Harness/Program.cs','Harness/Harness.csproj','NuGet.Config','global.json')
$fixed=@{
 'Program.cs'='0f94cd746e8a04d7bb5e794683a30b7512d406319eccb3e3c4e6fb3d79e29ae3';
 'fetch-owned-process.cs'='d6d9d3b5c31a2f697eb435cdbf99f0fc968e2af912b069923bc6f8c7497331b3';
 'FetchHost.csproj'='fdcf2d82f51d9986e855c388eeafb0a67b186ffbfc31a0bf47fab47ba52defa9';
 'NuGet.Config'='9508cae5f76364f56e80eb97cb16493977243cf38e5cca6284e3d74f83b20a2c';
 'global.json'='0503dacfc3e664e4b7fc0ac7da920e5939da4ee62ab852ce7aec1a0bdbdf38a2'
}
$dotnet='C:\Program Files\dotnet\dotnet.exe'
$sourceMap=@{}
$diagnosticCodes=@('SCOPE','SOURCE_MANIFEST','SOURCE_INVENTORY','SOURCE_HASH','WORKFLOW_HASH','COPY_READ','COPY_SCAN','COPY_MISSING','COPY_HASH','COPY_EXTRA','COPY_INVENTORY','DOTNET_HASH','RUNTIME_HASH','BUILD_ENV','BUILD_START','BUILD_STREAM_INIT','BUILD_WAIT','BUILD_TIMEOUT','BUILD_DRAIN','BUILD_EXIT','BUILD_OUTPUT_LIMIT','CAPTURE_HELPER','COPY_PATH','NUGET_DIRECTORY','NUGET_MIGRATION','HARNESS_START','HARNESS_WAIT','HARNESS_DRAIN','RESULT_VALIDATE','OTHER')
$diagnosticStages=@('scope','source-identity','dotnet-identity','build-host','build-fixture','build-harness','runtime-identity','synthetic-host-entry13')
function Fail-Diagnostic([string]$Code) {
 if($Code -notin $diagnosticCodes){$Code='OTHER'}
 $script:checkpoint=$Code
 throw [InvalidOperationException]::new('fixed diagnostic failure')
}
function Failure-Snapshot($Record) {
 $code=if($script:checkpoint -in $diagnosticCodes){$script:checkpoint}else{'OTHER'}
 $where=if($script:stage -in $diagnosticStages){$script:stage}else{'scope'}
 $kind=$Record.Exception.GetType().Name
 if($kind -notin @('InvalidOperationException','RuntimeException','MethodInvocationException','IOException','UnauthorizedAccessException','ArgumentException','DirectoryNotFoundException','FileNotFoundException')){$kind='OtherException'}
 return @{stage=$where;code=$code;type=$kind}
}
function Extra-Class([string]$Relative) {
 switch([IO.Path]::GetExtension($Relative).ToLowerInvariant()) {
  '.json' {return 'json'} '.props' {return 'props'} '.targets' {return 'targets'}
  '.cs' {return 'cs'} '.dll' {return 'dll'} '.exe' {return 'exe'}
  '.pdb' {return 'pdb'} '.cache' {return 'cache'} '.txt' {return 'txt'}
  default {return 'other'}
 }
}
function Initialize-BoundedCapture {
 Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
public sealed class XenBuildCaptureV1 {
    public const int Limit=65536;
    readonly byte[] retained=new byte[Limit];
    int count=0;
    long seen=0;
    volatile bool exceeded=false;
    public int RetainedBytes { get { return Volatile.Read(ref count); } }
    public long ObservedBytes { get { return Interlocked.Read(ref seen); } }
    public bool Exceeded { get { return exceeded; } }
    public string Text { get { return Encoding.UTF8.GetString(retained,0,RetainedBytes); } }
    public async Task Drain(Stream input) {
        var buffer=new byte[4096];
        int size;
        while((size=await input.ReadAsync(buffer,0,buffer.Length).ConfigureAwait(false))>0) {
            Interlocked.Add(ref seen,size);
            int old=RetainedBytes,copy=Math.Min(size,Limit-old);
            if(copy>0) {Buffer.BlockCopy(buffer,0,retained,old,copy);Volatile.Write(ref count,old+copy);}
            if(copy<size) exceeded=true;
            // Drain excess without retaining it. The parent keeps its original
            // 60-second process bound, and any excess makes the build fail.
        }
    }
}
'@
}
function Safe-BuildDiagnostic($Capture) {
 $lines=[Collections.Generic.List[string]]::new();$codes=@{};$omitted=0
 foreach($line in ($Capture.Text -split "`r?`n")){
  foreach($match in [regex]::Matches($line,'\b(?:CS|MSB|NU|NETSDK)[0-9]{3,5}\b')){
   if($codes.ContainsKey($match.Value)){$codes[$match.Value]=[Math]::Min(1000,$codes[$match.Value]+1)}elseif($codes.Count -lt 32){$codes[$match.Value]=1}
  }
  if($line -notmatch '(?i)error|warning|sdk|msbuild|global\.json|could not|failed|exception|targetframework|^\s*[0-9]+\.[0-9]+\.[0-9]+'){continue}
  if($line -match '(?i)[A-Za-z0-9_]*(?:token|password|secret|authorization|credential|api[_-]?key)[A-Za-z0-9_]*|\b(?:Bearer|Basic)\s+\S+|\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9_-]+|\bAKIA[A-Z0-9]{16}\b' -or $line -cmatch '\b[A-Z_][A-Z0-9_]{2,}\s*='){$omitted++;continue}
  $safe=$line
  # Replace only known runtime roots by logical names before the generic path
  # redactor; no environment dump, raw exception, or arbitrary filename content.
  foreach($pair in @(@($work,'<source>'),@($PSScriptRoot,'<control>'),@($env:GITHUB_WORKSPACE,'<checkout>'),@($OutputRoot,'<output>'),@([IO.Path]::GetDirectoryName($dotnet),'<dotnet>'))){
   if($pair[0]){$safe=[regex]::Replace($safe,([regex]::Escape($pair[0])+'(?=[\\/]|$)'),$pair[1],[Text.RegularExpressions.RegexOptions]::IgnoreCase)}
  }
  $safe=[regex]::Replace($safe,'(?i)https?://\S+','<url>')
  $safe=[regex]::Replace($safe,'(?i)(?:[A-Z]:[\\/]|\\\\)[^\r\n\]"\)]*','<absolute-path>')
  $safe=[regex]::Replace($safe,'(?<![A-Za-z0-9_>])/(?:[^\r\n\]"\)]+)','<absolute-path>')
  $safe=[regex]::Replace($safe,'[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}','<email>')
  $safe=[regex]::Replace($safe,'[\x00-\x08\x0b\x0c\x0e-\x1f]','?')
  if($safe.Length -gt 512){$safe=$safe.Substring(0,512)+'<truncated>'}
  if($lines.Count -lt 32){$lines.Add($safe)}else{$omitted++}
 }
 return @{retainedBytes=$Capture.RetainedBytes;observedBytes=$Capture.ObservedBytes;limitBytes=65536;overflow=$Capture.Exceeded;lines=@($lines.ToArray());codes=$codes;omittedLines=$omitted}
}
function Safe-ExtraEntry([string]$Relative) {
 if([IO.Path]::IsPathRooted($Relative) -or $Relative.Length -gt 256 -or $Relative -match '(^|/)\.\.(/|$)' -or $Relative -match '[:\x00-\x1f]' -or $Relative -notmatch '^[A-Za-z0-9._/+,=@ ()-]+$'){Fail-Diagnostic 'COPY_PATH'}
 $full=[IO.Path]::GetFullPath((Join-Path $work $Relative));$prefix=[IO.Path]::GetFullPath($work)+[IO.Path]::DirectorySeparatorChar
 if(-not $full.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase)){Fail-Diagnostic 'COPY_PATH'}
 Plain $full
 $file=Get-Item -LiteralPath $full -Force
 if($file.PSIsContainer){Fail-Diagnostic 'COPY_PATH'}
 return @{relative=$Relative;kind='regular-file';extensionClass=(Extra-Class $Relative);bytes=$file.Length}
}
function Plain([string]$Path) {
 if(-not [IO.Path]::IsPathFullyQualified($Path) -or [IO.Path]::GetFullPath($Path) -ne $Path){throw 'canonical path required'}
 $p=$Path
 while($p){if(Test-Path -LiteralPath $p){if(((Get-Item -LiteralPath $p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'reparse input'}};$p=[IO.Path]::GetDirectoryName($p)}
}
function Check-Identity {
 $script:checkpoint='SOURCE_MANIFEST'
 $manifest=Join-Path $PSScriptRoot 'source-hashes.json';Plain $manifest
 if((Get-FileHash -LiteralPath $manifest -Algorithm SHA256).Hash -ne $script:manifestHash){throw 'manifest changed'}
 $map=Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json -AsHashtable
 if(@(Compare-Object ($required|Sort-Object) (@($map.Keys)|Sort-Object)).Count -ne 0){throw 'fixed inventory mismatch'}
 $script:checkpoint='SOURCE_INVENTORY'
 $actual=@(Get-ChildItem -LiteralPath $PSScriptRoot -Recurse -Force -File | ForEach-Object {[IO.Path]::GetRelativePath($PSScriptRoot,$_.FullName).Replace('\','/')} | Where-Object {$_ -ne 'source-hashes.json'})
 if(@(Compare-Object ($required|Sort-Object) ($actual|Sort-Object)).Count -ne 0){throw 'unexpected source input'}
 $script:checkpoint='SOURCE_HASH'
 foreach($name in $required){$path=Join-Path $PSScriptRoot $name;Plain $path;if((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $map[$name]){throw 'source changed'}}
 foreach($name in $fixed.Keys){if($map['Host/'+$name] -ne $fixed[$name]){throw 'production Host changed'}}
 $script:checkpoint='WORKFLOW_HASH'
 $workflow=Join-Path $env:GITHUB_WORKSPACE '.github/workflows/ci.yml';Plain $workflow
 if((Get-FileHash -LiteralPath $workflow -Algorithm SHA256).Hash.ToLowerInvariant() -ne $map['.github/workflows/ci.yml']){throw 'workflow source mismatch'}
 $script:sourceMap=$map
}
function Check-Copies {
 $script:checkpoint='COPY_READ'
 $phase=if($script:inFinal){'final'}else{'primary'}
 $check=@{ordinal=$copyChecks.Count;phase=$phase;missingIndices=@();changedIndices=@();extraCount=0;extraClasses=@{};extraEntries=@();extraEntriesTruncated=$false;scanComplete=$false}
 $copyChecks.Add($check)
 for($i=0;$i -lt $compileInputs.Count;$i++){
  $name=$compileInputs[$i];$path=Join-Path $work $name;Plain $path
  if(-not(Test-Path -LiteralPath $path -PathType Leaf)){$check.missingIndices+= $i;continue}
  if((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sourceMap[$name]){$check.changedIndices+= $i}
 }
 $script:checkpoint='COPY_SCAN'
 $actual=@(Get-ChildItem -LiteralPath $work -Recurse -Force -File | ForEach-Object {[IO.Path]::GetRelativePath($work,$_.FullName).Replace('\','/')})
 foreach($relative in $actual){if($relative -notin $compileInputs){$check.extraCount++;$class=Extra-Class $relative;if(-not $check.extraClasses.ContainsKey($class)){$check.extraClasses[$class]=0};$check.extraClasses[$class]++;$entry=Safe-ExtraEntry $relative;if($check.extraEntries.Count -lt 16){$check.extraEntries+=$entry}else{$check.extraEntriesTruncated=$true}}}
 $check.scanComplete=$true
 if($check.missingIndices.Count -gt 0){Fail-Diagnostic 'COPY_MISSING'}
 if($check.changedIndices.Count -gt 0){Fail-Diagnostic 'COPY_HASH'}
 if($check.extraCount -gt 0){Fail-Diagnostic 'COPY_EXTRA'}
 if(@(Compare-Object ($compileInputs|Sort-Object) ($actual|Sort-Object)).Count -ne 0){Fail-Diagnostic 'COPY_INVENTORY'}
}
function Verify-IsolatedDirectory([string]$Path) {
 $script:checkpoint='NUGET_DIRECTORY'
 $isolated=[IO.Path]::GetFullPath((Join-Path $OutputRoot 'isolated'))+[IO.Path]::DirectorySeparatorChar
 $full=[IO.Path]::GetFullPath($Path)
 if(-not $full.StartsWith($isolated,[StringComparison]::OrdinalIgnoreCase)){Fail-Diagnostic 'NUGET_DIRECTORY'}
 Plain $full
 [IO.Directory]::CreateDirectory($full)|Out-Null
 Plain $full
 if(-not(Test-Path -LiteralPath $full -PathType Container)){Fail-Diagnostic 'NUGET_DIRECTORY'}
 # Test only this freshly owned scratch directory, never change ACLs or inspect
 # machine/user config. CreateNew prevents deleting a pre-existing probe.
 $probe=Join-Path $full '.xenterm-write-probe';Plain $probe;$owned=$false;$stream=$null
 try {
  $stream=[IO.File]::Open($probe,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None);$owned=$true
  $stream.WriteByte(1);$stream.Flush()
 } finally {if($stream){$stream.Dispose()};if($owned){[IO.File]::Delete($probe)}}
 if(Test-Path -LiteralPath $probe){Fail-Diagnostic 'NUGET_DIRECTORY'}
 $script:directoryProbeChecks++
}
function Check-NuGetMigration {
 $script:checkpoint='NUGET_MIGRATION'
 $marker=Join-Path $OutputRoot 'isolated/home/AppData/Local/NuGet/Migrations/1';Plain $marker
 $script:nugetIsolation.migrationMarkerAtExpectedRoot=Test-Path -LiteralPath $marker -PathType Leaf
 if(-not $script:nugetIsolation.migrationMarkerAtExpectedRoot){Fail-Diagnostic 'NUGET_MIGRATION'}
 $script:nugetIsolation.markerBytes=(Get-Item -LiteralPath $marker).Length
 if($script:nugetIsolation.markerBytes -ne 0){Fail-Diagnostic 'NUGET_MIGRATION'}
}
function Set-ChildEnvironment([Diagnostics.ProcessStartInfo]$Info) {
 $Info.Environment.Clear()
 $windows=[Environment]::GetFolderPath([Environment+SpecialFolder]::Windows);Plain $windows
 $values=@{SYSTEMROOT=$windows;WINDIR=$windows;PATH=([IO.Path]::GetDirectoryName($dotnet)+';'+(Join-Path $windows 'System32'));DOTNET_CLI_TELEMETRY_OPTOUT='1';DOTNET_SKIP_FIRST_TIME_EXPERIENCE='1';DOTNET_NOLOGO='1';DOTNET_MULTILEVEL_LOOKUP='0'}
 foreach($key in @('HOME','USERPROFILE','DOTNET_CLI_HOME')){$values[$key]=Join-Path $OutputRoot 'isolated/home'}
 foreach($key in @('TEMP','TMP')){$values[$key]=Join-Path $OutputRoot 'isolated/tmp'}
 # NuGet CoreCLR uses PROGRAMFILES(X86), then PROGRAMFILES, for its machine-wide
 # defaults path. Both must be explicit empty scratch roots after Environment.Clear.
 # Source: NuGet.Client release-6.11.x, NuGet.Common/PathUtil/NuGetEnvironment.cs.
 $values['PROGRAMFILES']=Join-Path $OutputRoot 'isolated/programfiles'
 $values['PROGRAMFILES(X86)']=Join-Path $OutputRoot 'isolated/programfiles-x86'
 # MigrationRunner uses .NET's Windows KnownFolder API, not only LOCALAPPDATA.
 # Materialize the standard profile layout and verify the actual migration marker
 # after restore; these assignments alone are not claimed to prove resolution.
 $values['APPDATA']=Join-Path $OutputRoot 'isolated/home/AppData/Roaming'
 $values['LOCALAPPDATA']=Join-Path $OutputRoot 'isolated/home/AppData/Local'
 $values['NUGET_PACKAGES']=Join-Path $OutputRoot 'isolated/nuget'
 foreach($key in @('HOME','TEMP','APPDATA','LOCALAPPDATA','NUGET_PACKAGES','PROGRAMFILES','PROGRAMFILES(X86)')){
  if(-not $values.ContainsKey($key) -or [string]::IsNullOrEmpty($values[$key])){Fail-Diagnostic 'NUGET_DIRECTORY'}
  Verify-IsolatedDirectory $values[$key]
 }
 foreach($key in $values.Keys){$Info.Environment.Add($key,$values[$key])}
}
function Build([string]$Project,[string]$Name,[string]$Exe) {
    $script:stage='build-'+$Name
    $outcome=@{stage=$script:stage;started=$false;exited=$null;exitCode=$null;streamsComplete=$null;timedOut=$null;outputExceeded=$null;stdout=$null;stderr=$null;diagnosticFailure=$null}
    $buildOutcomes.Add($outcome);$script:checkpoint='BUILD_ENV'
    $bin=Join-Path $OutputRoot ($Name+'/bin/');$obj=Join-Path $OutputRoot ($Name+'/obj/')
    $info=[Diagnostics.ProcessStartInfo]::new($dotnet)
    Set-ChildEnvironment $info
    $info.WorkingDirectory=$work
    $info.UseShellExecute=$false;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
    foreach($arg in @('build',$Project,'--disable-build-servers','-c','Release',"-p:RestoreConfigFile=$work/NuGet.Config",'-p:NuGetAudit=false','-p:ImportDirectoryBuildProps=false','-p:ImportDirectoryBuildTargets=false',"-p:BaseOutputPath=$bin","-p:BaseIntermediateOutputPath=$obj",'-v:q')){$info.ArgumentList.Add($arg)}
    $process=$null;$stdoutCapture=$null;$stderrCapture=$null
    try {
        $script:checkpoint='BUILD_START'
        $process=[Diagnostics.Process]::Start($info);$outcome.started=$true
        $script:checkpoint='BUILD_STREAM_INIT'
        $stdoutCapture=[XenBuildCaptureV1]::new();$stderrCapture=[XenBuildCaptureV1]::new()
        $copies=[Threading.Tasks.Task[]]@($stdoutCapture.Drain($process.StandardOutput.BaseStream),$stderrCapture.Drain($process.StandardError.BaseStream))
        $script:checkpoint='BUILD_WAIT'
        if(-not $process.WaitForExit(60000)){
            $outcome.exited=$false;$outcome.timedOut=$true;$script:checkpoint='BUILD_TIMEOUT'
            try{$process.Kill($true);$null=$process.WaitForExit(5000)}finally{throw 'synthetic build timeout; lifecycle unknown'}
        }
        $outcome.exited=$true;$outcome.timedOut=$false;$outcome.exitCode=$process.ExitCode
        $script:checkpoint='BUILD_DRAIN'
        $outcome.streamsComplete=[Threading.Tasks.Task]::WaitAll($copies,5000)
        if(-not $outcome.streamsComplete){Fail-Diagnostic 'BUILD_DRAIN'}
        $outcome.outputExceeded=$stdoutCapture.Exceeded -or $stderrCapture.Exceeded
        if($outcome.outputExceeded){Fail-Diagnostic 'BUILD_OUTPUT_LIMIT'}
        if($outcome.exitCode -ne 0){Fail-Diagnostic 'BUILD_EXIT'}
    } finally {
        try {
            if($stdoutCapture){$outcome.stdout=Safe-BuildDiagnostic $stdoutCapture}
            if($stderrCapture){$outcome.stderr=Safe-BuildDiagnostic $stderrCapture}
        } catch {$outcome.diagnosticFailure='SANITIZE_FAILED';$outcome.stdout=$null;$outcome.stderr=$null}
        if($process){$process.Dispose()}
    }
    return (Join-Path $bin ('Release/net8.0/'+$Exe))
}

try {
 if(-not $IsWindows -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:RUNNER_ARCH -ne 'X64' -or $env:GITHUB_REPOSITORY -ne 'xue-ding-e/XenTerm-Next' -or $env:GITHUB_REF -ne 'refs/heads/validation/xenterm-fetch-host-entry-20261031' -or $env:GITHUB_EVENT_NAME -ne 'workflow_dispatch'){throw 'fixed hosted scope required'}
 if($env:GITHUB_SHA -notmatch '^[0-9a-f]{40}$' -or $env:EXPECTED_CONTROL_SHA -ne $env:GITHUB_SHA -or $env:GITHUB_RUN_ID -notmatch '^[0-9]+$'){throw 'control run identity'}
 Plain $PSScriptRoot
 if([IO.Path]::GetFullPath($PSScriptRoot) -ne [IO.Path]::GetFullPath((Join-Path $env:GITHUB_WORKSPACE 'validation/fetch-host-entry-native'))){throw 'fixed source location'}
 $OutputRoot=Join-Path $env:RUNNER_TEMP ('xenterm-host-entry-'+$env:GITHUB_SHA);Plain $OutputRoot
 if(Test-Path -LiteralPath $OutputRoot){throw 'single-use output already exists'}
 [IO.Directory]::CreateDirectory($OutputRoot)|Out-Null;$created=$true
 $stage='source-identity';$manifestHash=(Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'source-hashes.json') -Algorithm SHA256).Hash
 Check-Identity;$initial=$true
 $expected=@(Get-Content -LiteralPath (Join-Path $PSScriptRoot 'expected-cases.json') -Raw | ConvertFrom-Json)
 if($expected.Count -ne 13){throw 'expected cases inventory'}
 $stage='dotnet-identity';$checkpoint='DOTNET_HASH';Plain $dotnet
 $dotnetHash=(Get-FileHash -LiteralPath $dotnet -Algorithm SHA256).Hash
 $work=Join-Path $OutputRoot 'build';[IO.Directory]::CreateDirectory($work)|Out-Null
 foreach($name in $compileInputs){$destination=Join-Path $work $name;[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))|Out-Null;Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $destination}
 $copied=$true;Check-Copies
 $checkpoint='CAPTURE_HELPER';Initialize-BoundedCapture
 $fetchHost=Build 'Host/FetchHost.csproj' 'host' 'FetchHost.exe'
 Check-NuGetMigration
 $fixture=Build 'Fixture/Fixture.csproj' 'fixture' 'cargo.exe'
 $harness=Build 'Harness/Harness.csproj' 'harness' 'FetchHostEntryHarness.exe'
 Check-Copies
 $stage='runtime-identity';$checkpoint='RUNTIME_HASH'
 $executables=@{Host=$fetchHost;Fixture=$fixture;Harness=$harness}
 foreach($label in $executables.Keys) {
  $exe=$executables[$label];$base=[IO.Path]::GetFileNameWithoutExtension($exe);$dir=[IO.Path]::GetDirectoryName($exe)
  foreach($suffix in @('.exe','.dll','.deps.json','.runtimeconfig.json')){
   $path=Join-Path $dir ($base+$suffix);Plain $path;$hash=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
   $runtime[$path]=$hash;$runtimeEvidence[$label+'/'+$base+$suffix]=$hash
  }
 }
 if($runtime.Count -ne 12){throw 'runtime inventory'}
 $stage='synthetic-host-entry13'
 $info=[Diagnostics.ProcessStartInfo]::new($harness);$info.UseShellExecute=$false;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true;$info.WorkingDirectory=$work
 Set-ChildEnvironment $info
 foreach($arg in @($fetchHost,$fixture,(Join-Path $OutputRoot 'cases'))){$info.ArgumentList.Add($arg)}
 $checkpoint='HARNESS_START'
 $process=[Diagnostics.Process]::Start($info);$harnessStarted=$true
 try {
  $drain=[Threading.Tasks.Task[]]@($process.StandardOutput.BaseStream.CopyToAsync([IO.Stream]::Null),$process.StandardError.BaseStream.CopyToAsync([IO.Stream]::Null))
  $checkpoint='HARNESS_WAIT'
  if(-not $process.WaitForExit(90000)){try{$process.Kill($true);$null=$process.WaitForExit(5000)}finally{throw 'synthetic harness timeout; lifecycle unknown'}}
  $checkpoint='HARNESS_DRAIN'
  if(-not [Threading.Tasks.Task]::WaitAll($drain,5000)){throw 'harness stream completion unknown'}
  $code=$process.ExitCode
 } finally {$process.Dispose()}
 $checkpoint='RESULT_VALIDATE'
 $receipt=Join-Path $OutputRoot 'cases/host-entry-result.json';Plain $receipt
 if(-not(Test-Path -LiteralPath $receipt -PathType Leaf) -or (Get-Item -LiteralPath $receipt).Length -gt 65536){throw 'Host entry receipt bound'}
 $result=Get-Content -LiteralPath $receipt -Raw | ConvertFrom-Json
 $allowedFailureCases=@($expected.name)+@('initial','runtime-identity')
 if($result.failureCase -in $allowedFailureCases){$failureCase=$result.failureCase}elseif($null -ne $result.failureCase){$failureCase='unrecognized-case'}
 if($code -ne 0 -or $result.passed -ne $true -or $result.case_count -ne 13 -or $result.cases.Count -ne 13 -or $null -ne $result.failureCase){throw 'Host entry suite failed'}
 if($result.hostEntryExecuted -ne $true -or $result.hostOsProcessInvocations -ne 13 -or $result.outerTimeoutObserved -ne $false){throw 'Host invocation evidence'}
 foreach($flag in @('outerJobObjectQualification','realCargoExecuted','dependencyFetchAttempted','fixtureNetworkAttempted','productCompileAttempted','productRuntimeAttempted')){if($result.$flag -isnot [bool] -or $result.$flag -ne $false){throw 'synthetic scope mismatch'}}
 for($i=0;$i -lt 13;$i++){
  $actual=$result.cases[$i];$want=$expected[$i]
  if($actual.name -ne $want.name -or $actual.passed -ne $true -or $actual.syntheticCargoExecutions -ne $want.syntheticCargoExecutions -or $actual.expectedRejection -ne $want.expectedRejection){throw 'case evidence mismatch'}
  $retained=@{name=$want.name;passed=$true;syntheticCargoExecutions=$want.syntheticCargoExecutions;expectedRejection=$want.expectedRejection}
  if($want.syntheticCargoExecutions -eq 1){
   $proof=$actual.hostReceipt
   $numeric=@('exitCode','boundaryRootExitCode','boundaryActiveProcessCount','cleanupMilliseconds','outputBytes','outputBudget')
   $boolean=@('treeEmpty','safeToStart','succeeded','forcedOwnedCleanup','timedOut','outputExceeded','captureFailed')
   $proofKeys=@($proof.PSObject.Properties.Name)
   if(@(Compare-Object (($numeric+$boolean)|Sort-Object) ($proofKeys|Sort-Object)).Count -ne 0){throw 'Host lifecycle inventory'}
   foreach($key in $numeric){if($proof.$key -isnot [long] -and $proof.$key -isnot [int]){throw 'Host lifecycle integer required'}}
   foreach($key in $boolean){if($proof.$key -isnot [bool]){throw 'Host lifecycle boolean required'}}
   $expectedExit=if($want.name -eq 'fixed-success'){0}elseif($want.name -eq 'nonzero-child'){7}else{throw 'unapproved executing case'}
   if($proof.exitCode -ne $expectedExit -or $proof.boundaryRootExitCode -ne $expectedExit -or $proof.boundaryActiveProcessCount -ne 0){throw 'Host exit boundary evidence'}
   if(-not $proof.treeEmpty -or -not $proof.safeToStart -or $proof.succeeded -ne ($expectedExit -eq 0)){throw 'Host success state evidence'}
   foreach($key in @('forcedOwnedCleanup','timedOut','outputExceeded','captureFailed')){if($proof.$key){throw 'Host lifecycle failure'}}
   if($proof.cleanupMilliseconds -lt 0 -or $proof.cleanupMilliseconds -gt 5000 -or $proof.outputBytes -le 0 -or $proof.outputBytes -gt 1048576 -or $proof.outputBudget -ne 1048576){throw 'Host cleanup/output bounds'}
   $safeProof=@{};foreach($key in $numeric+$boolean){$safeProof[$key]=$proof.$key};$retained['hostReceipt']=$safeProof
  }elseif('hostReceipt' -in @($actual.PSObject.Properties.Name)){throw 'rejected case cannot claim a Host receipt'}
  $cases+=$retained
 }
 $passed=$true
} catch {$primaryFailure=Failure-Snapshot $_} finally {
 $inFinal=$true
 try {
  if($initial){
   Check-Identity
   if($copied){Check-Copies}
   $checkpoint='DOTNET_HASH';Plain $dotnet
   if((Get-FileHash -LiteralPath $dotnet -Algorithm SHA256).Hash -ne $dotnetHash){Fail-Diagnostic 'DOTNET_HASH'}
   $checkpoint='RUNTIME_HASH'
   foreach($path in $runtime.Keys){Plain $path;if((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $runtime[$path]){Fail-Diagnostic 'RUNTIME_HASH'}}
   $final=$true
  }
 } catch {$passed=$false;$finalFailure=Failure-Snapshot $_}
 if(-not($initial -and $final)){$passed=$false;$cases=@()}
 if($created){
  $entry=$false;if($harnessStarted){$entry=$null};$invocations=$null;$outerTimeout=$null
  if($passed){$entry=$true;$invocations=13;$outerTimeout=$false}
  @{passed=$passed;stage=$stage;primaryFailure=$primaryFailure;finalFailure=$finalFailure;buildOutcomes=@($buildOutcomes.ToArray());nugetIsolation=$nugetIsolation;directoryProbeChecks=$directoryProbeChecks;copyChecks=@($copyChecks.ToArray());failureCase=$failureCase;initialIdentity=$initial;finalIdentity=$final;controlSha=$env:GITHUB_SHA;runId=$env:GITHUB_RUN_ID;sourceManifestSha256=$manifestHash;case_count=$cases.Count;cases=$cases;runtimeFiles=$runtimeEvidence;dotnetHostSha256=$dotnetHash;hostEntryExecuted=$entry;hostOsProcessInvocations=$invocations;outerTimeoutObserved=$outerTimeout;outerJobObjectQualification=$false;realCargoExecuted=$false;dependencyFetchAttempted=$false;fixtureNetworkAttempted=$false;productCompileAttempted=$false;productRuntimeAttempted=$false;fullTenMinuteElapsedValidation=$false} | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $OutputRoot 'metadata.json') -Encoding utf8NoBOM
 }
}
if(-not $passed){exit 1}
