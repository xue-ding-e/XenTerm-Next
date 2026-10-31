# Manual-only synthetic qualification. Never invokes real Cargo or product code.
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$passed=$false;$initial=$false;$final=$false;$created=$false;$copied=$false
$stage='scope';$failureType=$null;$failureCase=$null;$manifestHash=$null
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
function Plain([string]$Path) {
 if(-not [IO.Path]::IsPathFullyQualified($Path) -or [IO.Path]::GetFullPath($Path) -ne $Path){throw 'canonical path required'}
 $p=$Path
 while($p){if(Test-Path -LiteralPath $p){if(((Get-Item -LiteralPath $p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'reparse input'}};$p=[IO.Path]::GetDirectoryName($p)}
}
function Check-Identity {
 $manifest=Join-Path $PSScriptRoot 'source-hashes.json';Plain $manifest
 if((Get-FileHash -LiteralPath $manifest -Algorithm SHA256).Hash -ne $script:manifestHash){throw 'manifest changed'}
 $map=Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json -AsHashtable
 if(@(Compare-Object ($required|Sort-Object) (@($map.Keys)|Sort-Object)).Count -ne 0){throw 'fixed inventory mismatch'}
 $actual=@(Get-ChildItem -LiteralPath $PSScriptRoot -Recurse -Force -File | ForEach-Object {[IO.Path]::GetRelativePath($PSScriptRoot,$_.FullName).Replace('\','/')} | Where-Object {$_ -ne 'source-hashes.json'})
 if(@(Compare-Object ($required|Sort-Object) ($actual|Sort-Object)).Count -ne 0){throw 'unexpected source input'}
 foreach($name in $required){$path=Join-Path $PSScriptRoot $name;Plain $path;if((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $map[$name]){throw 'source changed'}}
 foreach($name in $fixed.Keys){if($map['Host/'+$name] -ne $fixed[$name]){throw 'production Host changed'}}
 $workflow=Join-Path $env:GITHUB_WORKSPACE '.github/workflows/ci.yml';Plain $workflow
 if((Get-FileHash -LiteralPath $workflow -Algorithm SHA256).Hash.ToLowerInvariant() -ne $map['.github/workflows/ci.yml']){throw 'workflow source mismatch'}
 $script:sourceMap=$map
}
function Check-Copies {
 foreach($name in $compileInputs){$path=Join-Path $work $name;Plain $path;if((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sourceMap[$name]){throw 'build input copy changed'}}
 $actual=@(Get-ChildItem -LiteralPath $work -Recurse -Force -File | ForEach-Object {[IO.Path]::GetRelativePath($work,$_.FullName).Replace('\','/')})
 if(@(Compare-Object ($compileInputs|Sort-Object) ($actual|Sort-Object)).Count -ne 0){throw 'unexpected compiled input'}
}
function Set-ChildEnvironment([Diagnostics.ProcessStartInfo]$Info) {
 $Info.Environment.Clear()
 $windows=[Environment]::GetFolderPath([Environment+SpecialFolder]::Windows);Plain $windows
 $values=@{SYSTEMROOT=$windows;WINDIR=$windows;PATH=([IO.Path]::GetDirectoryName($dotnet)+';'+(Join-Path $windows 'System32'));DOTNET_CLI_TELEMETRY_OPTOUT='1';DOTNET_SKIP_FIRST_TIME_EXPERIENCE='1';DOTNET_NOLOGO='1';DOTNET_MULTILEVEL_LOOKUP='0'}
 foreach($key in @('HOME','USERPROFILE','DOTNET_CLI_HOME')){$values[$key]=Join-Path $OutputRoot 'isolated/home'}
 foreach($key in @('TEMP','TMP')){$values[$key]=Join-Path $OutputRoot 'isolated/tmp'}
 $values['APPDATA']=Join-Path $OutputRoot 'isolated/appdata';$values['LOCALAPPDATA']=Join-Path $OutputRoot 'isolated/localappdata';$values['NUGET_PACKAGES']=Join-Path $OutputRoot 'isolated/nuget'
 foreach($key in @('HOME','TEMP','APPDATA','LOCALAPPDATA','NUGET_PACKAGES')){$path=$values[$key];Plain $path;[IO.Directory]::CreateDirectory($path)|Out-Null}
 foreach($key in $values.Keys){$Info.Environment.Add($key,$values[$key])}
}
function Build([string]$Project,[string]$Name,[string]$Exe) {
    $script:stage='build-'+$Name
    $bin=Join-Path $OutputRoot ($Name+'/bin/');$obj=Join-Path $OutputRoot ($Name+'/obj/')
    $info=[Diagnostics.ProcessStartInfo]::new($dotnet)
    Set-ChildEnvironment $info
    $info.WorkingDirectory=$work
    $info.UseShellExecute=$false;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
    foreach($arg in @('build',$Project,'--disable-build-servers','-c','Release',"-p:RestoreConfigFile=$work/NuGet.Config",'-p:NuGetAudit=false','-p:ImportDirectoryBuildProps=false','-p:ImportDirectoryBuildTargets=false',"-p:BaseOutputPath=$bin","-p:BaseIntermediateOutputPath=$obj",'-v:q')){$info.ArgumentList.Add($arg)}
    $process=$null
    try {
        $process=[Diagnostics.Process]::Start($info)
        $copies=[Threading.Tasks.Task[]]@($process.StandardOutput.BaseStream.CopyToAsync([IO.Stream]::Null),$process.StandardError.BaseStream.CopyToAsync([IO.Stream]::Null))
        if(-not $process.WaitForExit(60000)){
            try{$process.Kill($true);$null=$process.WaitForExit(5000)}finally{throw 'synthetic build timeout; lifecycle unknown'}
        }
        if(-not [Threading.Tasks.Task]::WaitAll($copies,5000)){throw 'synthetic build log completion unknown'}
        if($process.ExitCode -ne 0){throw 'test-only build failed'}
    } finally {if($process){$process.Dispose()}}
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
 $stage='dotnet-identity';Plain $dotnet
 $dotnetHash=(Get-FileHash -LiteralPath $dotnet -Algorithm SHA256).Hash
 $work=Join-Path $OutputRoot 'build';[IO.Directory]::CreateDirectory($work)|Out-Null
 foreach($name in $compileInputs){$destination=Join-Path $work $name;[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination))|Out-Null;Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $destination}
 $copied=$true;Check-Copies
 $fetchHost=Build 'Host/FetchHost.csproj' 'host' 'FetchHost.exe'
 $fixture=Build 'Fixture/Fixture.csproj' 'fixture' 'cargo.exe'
 $harness=Build 'Harness/Harness.csproj' 'harness' 'FetchHostEntryHarness.exe'
 Check-Copies
 $stage='runtime-identity'
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
 $process=[Diagnostics.Process]::Start($info);$harnessStarted=$true
 try {
  $drain=[Threading.Tasks.Task[]]@($process.StandardOutput.BaseStream.CopyToAsync([IO.Stream]::Null),$process.StandardError.BaseStream.CopyToAsync([IO.Stream]::Null))
  if(-not $process.WaitForExit(90000)){try{$process.Kill($true);$null=$process.WaitForExit(5000)}finally{throw 'synthetic harness timeout; lifecycle unknown'}}
  if(-not [Threading.Tasks.Task]::WaitAll($drain,5000)){throw 'harness stream completion unknown'}
  $code=$process.ExitCode
 } finally {$process.Dispose()}
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
} catch {$failureType=$_.Exception.GetType().Name} finally {
 try {
  if($initial){Check-Identity;if($copied){Check-Copies};Plain $dotnet;if((Get-FileHash -LiteralPath $dotnet -Algorithm SHA256).Hash -ne $dotnetHash){throw 'dotnet host changed'};foreach($path in $runtime.Keys){Plain $path;if((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $runtime[$path]){throw 'runtime changed'}};$final=$true}
 } catch {$passed=$false;$failureType=$_.Exception.GetType().Name}
 if(-not($initial -and $final)){$passed=$false;$cases=@()}
 if($created){
  $entry=$false;if($harnessStarted){$entry=$null};$invocations=$null;$outerTimeout=$null
  if($passed){$entry=$true;$invocations=13;$outerTimeout=$false}
  @{passed=$passed;stage=$stage;failureType=$failureType;failureCase=$failureCase;initialIdentity=$initial;finalIdentity=$final;controlSha=$env:GITHUB_SHA;runId=$env:GITHUB_RUN_ID;sourceManifestSha256=$manifestHash;case_count=$cases.Count;cases=$cases;runtimeFiles=$runtimeEvidence;dotnetHostSha256=$dotnetHash;hostEntryExecuted=$entry;hostOsProcessInvocations=$invocations;outerTimeoutObserved=$outerTimeout;outerJobObjectQualification=$false;realCargoExecuted=$false;dependencyFetchAttempted=$false;fixtureNetworkAttempted=$false;productCompileAttempted=$false;productRuntimeAttempted=$false;fullTenMinuteElapsedValidation=$false} | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $OutputRoot 'metadata.json') -Encoding utf8NoBOM
 }
}
if(-not $passed){exit 1}
