# Test CLI/MCP export with synthetic profiles and loopback SSH only.
#
# Purpose: after MCP and CLI were merged onto the shared automation layer,
# confirm the command-line path still works. The fixture applies bounded
# subprocess timeouts and never exports the user's real credentials.
param(
    [string]$Exe = "$PSScriptRoot\..\target\debug\xenterm.exe",
    [string]$Python = "python"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $Exe)) {
    throw "executable not found: $Exe"
}
$fixture = Join-Path $PSScriptRoot '..\tests\config_export_e2e.py'
& $Python $fixture --exe (Resolve-Path -LiteralPath $Exe).Path
if ($LASTEXITCODE -ne 0) {
    throw "synthetic export regression failed (exit $LASTEXITCODE)"
}
