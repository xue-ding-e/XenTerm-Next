# Windows 10/11 x64 prerequisite gate. No download, install, elevation or service.
$ErrorActionPreference = 'Stop'
$metadata = Get-Content (Join-Path $PSScriptRoot 'WINDOWS-RUNTIME.json') -Raw | ConvertFrom-Json
$minimum = [version]$metadata.minimum_vc_runtime
$installed = @(
  'HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64',
  'HKLM:\SOFTWARE\WOW6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\x64'
) | ForEach-Object {
  $entry = Get-ItemProperty $_ -ErrorAction SilentlyContinue
  if ($entry -and $entry.Installed -eq 1) { [version]($entry.Version -replace '^v', '') }
} | Sort-Object -Descending | Select-Object -First 1
if (-not $installed -or $installed -lt $minimum) {
  throw "Microsoft Visual C++ x64 runtime $minimum or newer is required. Install it manually from https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist then retry. This preview does not install software or request administrator access."
}
Start-Process -FilePath (Join-Path $PSScriptRoot 'xenterm.exe') -WorkingDirectory $PSScriptRoot
