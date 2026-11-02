# Disposable installer fixture initialization through the pinned application's public CLI.
# Does not connect to a server, import credentials, or fabricate a SQLite schema.
function Invoke-GpuiFixtureCli([string]$Exe,[string[]]$Arguments) {
 $start=[Diagnostics.ProcessStartInfo]::new();$start.FileName=$Exe;$start.UseShellExecute=$false
 $start.WorkingDirectory=Split-Path $Exe;$start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
 foreach($argument in $Arguments){$start.ArgumentList.Add($argument)}
 $process=[Diagnostics.Process]::new();$process.StartInfo=$start
 try {
  if(-not $process.Start()){throw 'Fixture CLI did not start'}
  $stdout=$process.StandardOutput.ReadToEndAsync();$stderr=$process.StandardError.ReadToEndAsync()
  if(-not $process.WaitForExit(30000)){$process.Kill();if(-not $process.WaitForExit(5000)){throw 'Fixture CLI cleanup timed out'};throw 'Fixture CLI timed out'}
  $output=$stdout.GetAwaiter().GetResult();$errorOutput=$stderr.GetAwaiter().GetResult()
  if($output.Length -gt 16384 -or $errorOutput.Length -gt 16384){throw 'Fixture CLI output exceeds evidence bound'}
  return @{exit_code=$process.ExitCode;stdout=$output;stderr=$errorOutput}
 } finally {$process.Dispose()}
}
function Normalize-GpuiLocalProfilePath([string]$Value) {
 # Rust canonicalize returns a verbatim drive path on Windows. Accept only that
 # exact local-drive form or an ordinary absolute drive path, never UNC/devices.
 if($Value -cmatch '^\\\\\?\\[A-Za-z]:\\'){$Value=$Value.Substring(4)}
 if($Value -cnotmatch '^[A-Za-z]:\\'){throw 'Profile path must be an absolute local drive path'}
 if($Value.EndsWith('\')){$Value=$Value.Substring(0,$Value.Length-1)}
 $components=$Value.Substring(3).Split('\')
 foreach($part in $components){
  if($part.Length -eq 0 -or $part -in @('.','..') -or $part -match '[<>:"/|?*\x00-\x1f]' -or $part -match '[. ]$' -or $part -match '^(CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(\.|$)'){
   throw 'Unsupported or ambiguous local profile path component'
  }
 }
 return $Value
}
function Assert-GpuiInitializationProof($Version,$Permission,[string]$Profile,[string]$ExpectedVersion) {
 if($Version.exit_code -ne 0 -or $Version.stdout.Trim() -cne ('xenterm '+$ExpectedVersion)){throw 'Pinned old runtime version mismatch'}
 if($Permission.exit_code -ne 0){throw 'Official fixture initialization failed'}
 $policy=$Permission.stdout|ConvertFrom-Json
 if($policy.mode -cne 'restricted' -or (Normalize-GpuiLocalProfilePath $policy.data_dir) -ine (Normalize-GpuiLocalProfilePath $Profile)){throw 'Official CLI initialized a different profile or access mode'}
}
function Initialize-GpuiTestProfile([string]$Exe,[string]$Profile,[string]$ExpectedVersion,[string]$Output) {
 if($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted'){throw 'Disposable hosted runner required'}
 if(Test-Path -LiteralPath $Output){throw 'Refusing to overwrite initialization evidence'}
 $Exe=(Resolve-Path -LiteralPath $Exe).Path;$Profile=(Resolve-Path -LiteralPath $Profile).Path
 if($Profile.TrimEnd('\') -ine (Join-Path (Split-Path $Exe) 'config').TrimEnd('\')){throw 'Fixture profile must be the installed executable sibling'}
 foreach($path in @($Exe,$Profile)){if((Get-Item -LiteralPath $path).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Refusing fixture reparse point'}}
 if((Get-Content -LiteralPath (Join-Path $Profile 'installer-preserve-test.json') -Raw).Trim() -cne '{"fixture":"preserve"}'){throw 'Missing owned synthetic marker'}
 if(@(Get-Process xenterm -ErrorAction SilentlyContinue|Where-Object {$_.Path -ieq $Exe}).Count){throw 'Fixture GUI must close before CLI initialization'}
 $evidence=@{profile=$Profile;expected_version=$ExpectedVersion;method='public cli permissions restricted';schema_fabricated=$false}
 try {
  $version=Invoke-GpuiFixtureCli $Exe @('--data-dir',$Profile,'--version');$evidence.version=$version
  if($version.exit_code -ne 0 -or $version.stdout.Trim() -cne ('xenterm '+$ExpectedVersion)){throw 'Pinned old runtime version mismatch'}
  $permission=Invoke-GpuiFixtureCli $Exe @('--data-dir',$Profile,'cli','permissions','restricted','--json');$evidence.permissions=$permission
  Assert-GpuiInitializationProof $version $permission $Profile $ExpectedVersion
  if(-not(Test-Path -LiteralPath (Join-Path $Profile 'sessions.db'))){throw 'Official CLI did not create native database'}
 } catch {$evidence.error=$_.Exception.Message;throw}
 finally {
  $bytes=[Text.UTF8Encoding]::new($false).GetBytes(($evidence|ConvertTo-Json -Depth 6))
  $stream=[IO.File]::Open($Output,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
  try{$stream.Write($bytes,0,$bytes.Length)}finally{$stream.Dispose()}
 }
}
