# Read-only fixed SDK inventory; no INCLUDE/LIB authorization or product execution.
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$root=Join-Path $env:RUNNER_TEMP 'key-import-compile-evidence'
$out=Join-Path $root 'sdk-metadata.json'
$rows=@();$ok=$false
function Plain([string]$path) {
    if(-not [IO.Path]::IsPathFullyQualified($path) -or [IO.Path]::GetFullPath($path) -ne $path){throw 'SDK path not canonical'}
    $p=$path
    while($p){if(Test-Path -LiteralPath $p){if(((Get-Item -LiteralPath $p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'SDK reparse path'}};$p=[IO.Path]::GetDirectoryName($p)}
}
function PathHash([string]$path){return [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($path.ToLowerInvariant()))).ToLowerInvariant()}
try {
    if(-not $IsWindows -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:RUNNER_ARCH -ne 'X64'){throw 'Hosted x64 Windows required'}
    Plain $root
    if(-not(Test-Path -LiteralPath $root -PathType Container) -or (Test-Path -LiteralPath $out)){throw 'Output ownership'}
    if($env:EXPECTED_CONTROL_SHA -notmatch '^[0-9a-f]{40}$' -or $env:GITHUB_SHA -ne $env:EXPECTED_CONTROL_SHA){throw 'Control identity'}
    # Only these three SDK keys and named values, in the two installation views.
    $specs=@(
      @{id='netfx-4.8';key='SOFTWARE\Microsoft\Microsoft SDKs\NETFXSDK\4.8';value='KitsInstallationFolder';relative='Windows Kits\NETFXSDK\4.8';leaves=@('Include\um','Lib\um\x64')},
      @{id='netfx-4.8.1';key='SOFTWARE\Microsoft\Microsoft SDKs\NETFXSDK\4.8.1';value='KitsInstallationFolder';relative='Windows Kits\NETFXSDK\4.8.1';leaves=@('Include\um','Lib\um\x64')},
      @{id='windows-10.0.26100.0';key='SOFTWARE\Microsoft\Windows Kits\Installed Roots';value='KitsRoot10';relative='Windows Kits\10';leaves=@('Include\10.0.26100.0\um','Include\10.0.26100.0\ucrt','Include\10.0.26100.0\shared','Lib\10.0.26100.0\um\x64','Lib\10.0.26100.0\ucrt\x64')}
    )
    foreach($spec in $specs){
      $expected=Join-Path ${env:ProgramFiles(x86)} $spec.relative
      foreach($view in @([Microsoft.Win32.RegistryView]::Registry32,[Microsoft.Win32.RegistryView]::Registry64)){
        $base=$null;$key=$null
        try{
          $base=[Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine,$view)
          $key=$base.OpenSubKey($spec.key,$false)
          $value=if($null -eq $key){$null}else{$key.GetValue($spec.value,$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)}
          $registered=$value -is [string] -and $value.Length -gt 0
          $match=$registered -and [string]::Equals($value.TrimEnd('\'),$expected,[StringComparison]::OrdinalIgnoreCase)
          $leaves=@();$plain=$false
          if($match){Plain $expected;foreach($leaf in $spec.leaves){$p=Join-Path $expected $leaf;Plain $p;$leaves+=@{relative=$leaf;exists=[IO.Directory]::Exists($p);path_sha256=(PathHash $p)}};$plain=$true}
          $rows+=@{id=$spec.id;view=$view.ToString();registered=$registered;fixed_root_match=[bool]$match;no_reparse=$plain;leaves=$leaves}
        }finally{if($null -ne $key){$key.Dispose()};if($null -ne $base){$base.Dispose()}}
      }
    }
    $ok=$true
}finally{
    # Installation metadata evidence, NOT cryptographic provenance of all headers/libs.
    if([IO.Directory]::Exists($root) -and -not(Test-Path -LiteralPath $out)){
      Plain $root
      $record=@{schema=1;control=$env:EXPECTED_CONTROL_SHA;run=$env:GITHUB_RUN_ID;capture_complete=$ok;product_environment_qualified=$false;all_sdk_files_cryptographically_verified=$false;rows=$rows}
      $f=[IO.File]::Open($out,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
      try{$b=[Text.Encoding]::UTF8.GetBytes(($record|ConvertTo-Json -Depth 8 -Compress));$f.Write($b,0,$b.Length)}finally{$f.Dispose()}
    }
}
