# Hosted-runner-only causal boundary using the exact retained old binary.
param([Parameter(Mandatory=$true)][string]$Exe,[Parameter(Mandatory=$true)][string]$PackagingScript,[Parameter(Mandatory=$true)][string]$Output)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
if($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted'){throw 'Disposable hosted runner required'}
if(Test-Path -LiteralPath Env:MEATSHELL_DATA_DIR){throw 'Refusing present MEATSHELL_DATA_DIR fallback; value not read'}
$expectedHash='7cfd020be7a57edcfe27224994a263fddab42367ba6c7c6842ba3ceed13e30a1'
$expectedVersion='xenterm xt-beta-8819e2e-T1793303184'
$Exe=(Resolve-Path -LiteralPath $Exe).Path
if((Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash -ine $expectedHash){throw 'Wrong original binary for environment boundary'}
if(Test-Path -LiteralPath $Output){throw 'Refusing existing native environment evidence'}
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $PackagingScript).Path,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Packaging script parse errors'}
$functions=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Restore-GpuiProfileEnvironment'},$true))
if($functions.Count -ne 1){throw 'Expected unique environment restore helper'}
. ([scriptblock]::Create($functions[0].Extent.Text))
$present=Test-Path -LiteralPath Env:XENTERM_DATA_DIR
$value=[Environment]::GetEnvironmentVariable('XENTERM_DATA_DIR','Process')
$root=Join-Path $env:RUNNER_TEMP ('gpui-native-env-'+[guid]::NewGuid())
if(Test-Path -LiteralPath $root){throw 'Refusing existing native fixture directory'}
[void][IO.Directory]::CreateDirectory($root)
$copy=Join-Path $root 'xenterm.exe';Copy-Item -LiteralPath $Exe -Destination $copy
if((Get-FileHash -LiteralPath $copy).Hash -ine $expectedHash){throw 'Copied binary hash mismatch'}
$result=@{meatshell_fallback_present=$false;binary_sha256=$expectedHash;expected_version=$expectedVersion;arguments=@('--version');cases=@();diagnostic_only=$true;restoration_verified=$false}
$mainError=$null
try {
 foreach($mode in @('empty','restored-absent')) {
  if($mode -eq 'empty'){Set-Item -LiteralPath Env:XENTERM_DATA_DIR -Value ''}
  else {Restore-GpuiProfileEnvironment $false $null}
  $entry=@{mode=$mode;environment_present=(Test-Path -LiteralPath Env:XENTERM_DATA_DIR);environment_length=$null}
  if($entry.environment_present){$entry.environment_length=([Environment]::GetEnvironmentVariable('XENTERM_DATA_DIR','Process')).Length}
  # Null length must not be used to determine presence; preserve the boolean above.
  if(!$entry.environment_present){$entry.environment_length=$null}
  $result.cases+=,$entry
  $p=[Diagnostics.Process]::new();$p.StartInfo=[Diagnostics.ProcessStartInfo]::new()
  $p.StartInfo.FileName=$copy;$p.StartInfo.WorkingDirectory=$root;$p.StartInfo.UseShellExecute=$false
  $p.StartInfo.Environment['HOME']=$root;$p.StartInfo.Environment['USERPROFILE']=$root
  $p.StartInfo.ArgumentList.Add('--version');$p.StartInfo.RedirectStandardOutput=$true;$p.StartInfo.RedirectStandardError=$true
  try {
   if(!$p.Start()){throw 'Native version process did not start'}
   $stdout=$p.StandardOutput.ReadToEndAsync();$stderr=$p.StandardError.ReadToEndAsync()
   if(!$p.WaitForExit(15000)){$p.Kill();if(!$p.WaitForExit(5000)){throw 'Native version cleanup timeout'};throw 'Native version timeout'}
   $entry.exit_code=$p.ExitCode;$out=$stdout.GetAwaiter().GetResult().Trim();$err=$stderr.GetAwaiter().GetResult().Trim()
   if([Text.Encoding]::UTF8.GetByteCount($out)+[Text.Encoding]::UTF8.GetByteCount($err) -gt 4096){throw 'Unexpected native version output size'}
   # Only known literal outputs are published; never print arbitrary profile data.
   $entry.stdout_expected=($out -ceq $expectedVersion);$entry.stdout_empty=($out -ceq '')
   $entry.stderr_empty=($err -ceq '')
   $entry.stderr_expected=($err -cin @('data directory environment variable must contain a valid path','Error: data directory environment variable must contain a valid path'))
   if($mode -eq 'empty') {
    if(!$entry.environment_present -or $entry.environment_length -ne 0 -or $entry.exit_code -eq 0 -or !$entry.stdout_empty -or !$entry.stderr_expected){throw 'Exact old binary did not strictly reject empty profile environment'}
   } else {
    if($entry.environment_present -or $entry.exit_code -ne 0 -or !$entry.stdout_expected -or !$entry.stderr_empty){throw 'Exact old binary version failed after absent restoration'}
   }
  }finally{$p.Dispose()}
 }
 $inventory=@(Get-ChildItem -LiteralPath $root -Force -Recurse)
 if($inventory.Count -ne 1 -or $inventory[0].FullName -ine $copy -or (Get-FileHash -LiteralPath $copy).Hash -ine $expectedHash){throw 'Version boundary created unexpected profile/database or changed binary'}
 $result.no_profile_or_database_created=$true
 Write-Output 'Real original binary: empty profile environment rejected; restored absence exact version succeeded'
}catch{$mainError=$_;$result.error=$_.Exception.Message;throw}
finally {
 try {Restore-GpuiProfileEnvironment $present $value
  if((Test-Path -LiteralPath Env:XENTERM_DATA_DIR) -ne $present -or [Environment]::GetEnvironmentVariable('XENTERM_DATA_DIR','Process') -cne $value){throw 'Parent profile environment restoration failed'}
  $result.restoration_verified=$true
 }catch{$result.restoration_error=$_.Exception.Message;if($null -eq $mainError){$mainError=$_}}
 try {
  $bytes=[Text.UTF8Encoding]::new($false).GetBytes(($result|ConvertTo-Json -Depth 8))
  $file=[IO.File]::Open($Output,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
  try{$file.Write($bytes,0,$bytes.Length)}finally{$file.Dispose()}
 }catch{if($null -eq $mainError){$mainError=$_}else{Write-Warning 'Native environment evidence write failed; original failure retained'}}
 # Only this newly created, fixed binary is removable; unexpected files are kept.
 try {
  if(Test-Path -LiteralPath $copy){Remove-Item -LiteralPath $copy}
  if(@(Get-ChildItem -LiteralPath $root -Force).Count -eq 0){[IO.Directory]::Delete($root)}
 }catch{if($null -eq $mainError){$mainError=$_}else{Write-Warning 'Native fixture cleanup failed; original failure retained'}}
 if($null -ne $mainError){throw $mainError}
}
