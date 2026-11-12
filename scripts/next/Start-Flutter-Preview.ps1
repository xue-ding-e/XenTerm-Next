# Windows 10/11 x64 prerequisite gate. No download, install, elevation or service.
$ErrorActionPreference = 'Stop'

function Assert-PreviewPlainPath([string]$Path) {
  $candidate = [IO.Path]::GetFullPath($Path)
  while ($candidate) {
    $item = Get-Item -LiteralPath $candidate -Force
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Path links are not permitted: $candidate" }
    $parent = [IO.Directory]::GetParent($candidate)
    if ($null -eq $parent) { break }
    $candidate = $parent.FullName
  }
}

$receipt = Join-Path $PSScriptRoot 'WINDOWS-RUNTIME.json'
Assert-PreviewPlainPath $receipt
$metadata = Get-Content -LiteralPath $receipt -Raw | ConvertFrom-Json
if ($metadata.runtime_mode -eq 'app-local') {
  # A portable app-local bundle must not depend on the machine-wide registry.
  # Verify both the listed CRT and application binaries before starting the GUI.
  $crt = @($metadata.crt_files)
  if ('msvcp140.dll' -cnotin $crt -or 'vcruntime140.dll' -cnotin $crt) { throw 'Missing app-local CRT inventory.' }
  foreach ($name in $crt) {
    if ($name -cnotmatch '^[A-Za-z0-9_.-]+\.dll$' -or
        $null -eq $metadata.binary_sha256.PSObject.Properties[$name]) { throw "Invalid CRT inventory: $name" }
  }
  foreach ($name in @('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll')) {
    if ($null -eq $metadata.binary_sha256.PSObject.Properties[$name]) { throw "Missing binary hash: $name" }
  }
  foreach ($entry in $metadata.binary_sha256.PSObject.Properties) {
    $name = $entry.Name
    if ($name -match '(^/|\\|:|(^|/)\.{1,2}(/|$))') { throw "Invalid binary path: $name" }
    $path = Join-Path $PSScriptRoot $name
    Assert-PreviewPlainPath $path
    if ($entry.Value -cnotmatch '^[0-9a-f]{64}$' -or
        (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $entry.Value) {
      throw "Binary provenance mismatch: $name"
    }
  }
} elseif ($metadata.runtime_mode -eq 'external-prerequisite') {
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
} else {
  throw 'Unknown Windows runtime mode.'
}
Start-Process -FilePath (Join-Path $PSScriptRoot 'xenterm.exe') -WorkingDirectory $PSScriptRoot
