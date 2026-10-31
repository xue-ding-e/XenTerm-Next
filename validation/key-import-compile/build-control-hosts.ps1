# Two control Hosts only, no fixture execution or product compilation.
# Bounded compiler capture/environment/migration logic reused from qualified 0ac5255.
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$OutputRoot=Join-Path $env:RUNNER_TEMP 'key-import-compile-evidence'
$control=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$work=Join-Path $OutputRoot 'control-build-inputs'
$dotnet='C:\Program Files\dotnet\dotnet.exe'
$directoryProbeChecks=0;$nugetIsolation=@{migrationMarkerAtExpectedRoot=$null;markerBytes=$null}
$buildOutcomes=[Collections.Generic.List[object]]::new();$checkpoint='BUILD_ENV';$stage='initial'
function Fail-Diagnostic([string]$Code){$script:checkpoint=$Code;throw [InvalidOperationException]::new($Code)}
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

function Plain([string]$Path) {
 if(-not [IO.Path]::IsPathFullyQualified($Path) -or [IO.Path]::GetFullPath($Path) -ne $Path){throw 'canonical path required'}
 $p=$Path
 while($p){if(Test-Path -LiteralPath $p){if(((Get-Item -LiteralPath $p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'reparse input'}};$p=[IO.Path]::GetDirectoryName($p)}
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

function Check-Control {
 if(-not $IsWindows -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:GITHUB_REPOSITORY -ne 'xue-ding-e/XenTerm-Next' -or $env:GITHUB_REF -ne 'refs/heads/validation/key-import-compile-only-20261030' -or $env:GITHUB_EVENT_NAME -ne 'workflow_dispatch'){throw 'fixed hosted scope'}
 if($env:EXPECTED_CONTROL_SHA -notmatch '^[0-9a-f]{40}$' -or $env:GITHUB_SHA -ne $env:EXPECTED_CONTROL_SHA){throw 'control SHA'}
 $head=& git -c core.fsmonitor=false -C $control rev-parse HEAD
 if($LASTEXITCODE -ne 0 -or $head -ne $env:EXPECTED_CONTROL_SHA){throw 'control head'}
 $status=& git -c core.fsmonitor=false -C $control status --porcelain --untracked-files=all
 if($LASTEXITCODE -ne 0 -or $status){throw 'dirty control'}
}
function Check-CopiedInputs {
 $actual=@(Get-ChildItem -LiteralPath $work -Recurse -Force -File|ForEach-Object{[IO.Path]::GetRelativePath($work,$_.FullName).Replace('\','/')})
 if(@(Compare-Object ($sourceMap.Keys|Sort-Object) ($actual|Sort-Object)).Count -ne 0){throw 'copy inventory changed'}
 foreach($key in $sourceMap.Keys){$path=Join-Path $work $key;Plain $path;if((Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -ne $sourceMap[$key]){throw 'copy input changed'}}
 foreach($path in $originals.Keys){Plain $path;if((Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -ne $originals[$path]){throw 'source input changed'}}
}
$sourceMap=@{};$originals=@{};$passed=$false;$initial=$false;$records=@{};$dotnetHash=$null;$failureCode=$null
try {
 Check-Control;Plain $OutputRoot;Plain $dotnet
 if(-not(Test-Path $OutputRoot -PathType Container) -or (Test-Path $work)){throw 'fresh build scope'}
 $dotnetHash=(Get-FileHash -LiteralPath $dotnet).Hash
 [IO.Directory]::CreateDirectory($work)|Out-Null
 $groups=@(
  @{folder='Compile';root=(Join-Path $PSScriptRoot 'adapter/Host');project='Host.csproj';assembly='CompileOnlyHost';name='host-build';prefix='Host/'},
  @{folder='Fetch';root=(Join-Path $control 'validation/fetch-host-entry-native/Host');project='FetchHost.csproj';assembly='FetchHost';name='fetch-host-build';prefix=''}
 )
 foreach($g in $groups){
  $names=@($g.project,'Program.cs','NuGet.Config','global.json',$(if($g.folder -eq 'Compile'){'windows-owned-process.cs'}else{'fetch-owned-process.cs'}))
  $actual=@(Get-ChildItem -LiteralPath $g.root -Recurse -Force -File|ForEach-Object{[IO.Path]::GetRelativePath($g.root,$_.FullName).Replace('\','/')})
  if(@(Compare-Object ($names|Sort-Object) ($actual|Sort-Object)).Count -ne 0){throw 'Host source inventory'}
  $target=Join-Path $work $g.folder;[IO.Directory]::CreateDirectory($target)|Out-Null;$sources=@{}
  foreach($name in $names){$path=Join-Path $g.root $name;Plain $path;$hash=(Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant();$originals[$path]=$hash;$sources[$g.prefix+$name]=$hash;$sourceMap[$g.folder+'/'+$name]=$hash;Copy-Item -LiteralPath $path -Destination (Join-Path $target $name)}
  $records[$g.name]=@{state='not-started';compile_exit=$null;initial_identity=$false;final_identity=$false;compiled_copy_identity=$false;control_sha=$env:GITHUB_SHA;run_id=$env:GITHUB_RUN_ID;host_sources=$sources;files=@{};product_compile_attempted=$false;product_runtime_attempted=$false}
 }
 # Explicit shared SDK selector and feed-free restore inputs, never ancestor config.
 foreach($shared in @('NuGet.Config','global.json')){
  $config=Join-Path $control ('validation/fetch-host-entry-native/'+$shared);Plain $config
  $hash=(Get-FileHash -LiteralPath $config).Hash.ToLowerInvariant();$originals[$config]=$hash;$sourceMap[$shared]=$hash
  Copy-Item -LiteralPath $config -Destination (Join-Path $work $shared)
 }
 Check-CopiedInputs;$initial=$true
 Initialize-BoundedCapture
 foreach($g in $groups){
  $record=$records[$g.name];$record.initial_identity=$true
  $exe=Build ($g.folder+'/'+$g.project) $g.name ($g.assembly+'.exe')
  $record.compile_exit=0
  Check-NuGetMigration;Check-CopiedInputs
  foreach($suffix in @('.exe','.dll','.deps.json','.runtimeconfig.json')){$name=$g.assembly+$suffix;$path=Join-Path (Split-Path $exe -Parent) $name;Plain $path;$record.files[$name]=(Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant()}
  $record.state='compiled'
 }
 Check-Control;Check-CopiedInputs
 if((Get-FileHash -LiteralPath $dotnet).Hash -ne $dotnetHash){throw 'dotnet changed'}
 foreach($record in $records.Values){$record.final_identity=$true;$record.compiled_copy_identity=$true}
 $passed=$true
} catch {$failureCode=$checkpoint} finally {
 foreach($name in $records.Keys){if(-not $passed){$records[$name].state='failed'};$records[$name]|ConvertTo-Json -Depth 8|Set-Content -LiteralPath (Join-Path $OutputRoot ($name+'.json')) -Encoding utf8NoBOM}
 @{passed=$passed;failureCode=$failureCode;buildOutcomes=@($buildOutcomes.ToArray());nugetIsolation=$nugetIsolation;directoryProbeChecks=$directoryProbeChecks;realCargoExecuted=$false;productCompileAttempted=$false;productRuntimeAttempted=$false}|ConvertTo-Json -Depth 10|Set-Content -LiteralPath (Join-Path $OutputRoot 'control-host-build.json') -Encoding utf8NoBOM
}
if(-not $passed){exit 1}
