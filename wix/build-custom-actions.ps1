# Requires an x64 Visual Studio developer shell (VS C++ tools / Windows SDK).
[CmdletBinding()]
param([string]$OutputDirectory = (Join-Path (Split-Path $PSScriptRoot -Parent) 'target/wix'))
$ErrorActionPreference = 'Stop'
if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    throw 'Run this script from an x64 Visual Studio developer shell (cl.exe required).'
}
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$output = Join-Path $OutputDirectory 'path_ownership.dll'
& cl.exe /nologo /LD /O2 /W4 /WX /MT /DUNICODE /D_UNICODE /std:c11 `
    (Join-Path $PSScriptRoot 'path_ownership.c') "/Fo$(Join-Path $OutputDirectory 'path_ownership.obj')" `
    "/Fe$output" /link msi.lib advapi32.lib "/IMPLIB:$(Join-Path $OutputDirectory 'path_ownership.lib')"
if ($LASTEXITCODE -ne 0) { throw 'Failed to compile MSI PATH ownership action.' }
Write-Host "Built $output"
