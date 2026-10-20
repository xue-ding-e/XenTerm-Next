Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Windows required. No emulation fallback.' }
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
if (-not ('XenTermNativeProbe.Desktop' -as [type])) {
    Add-Type -Path (Join-Path $PSScriptRoot 'NativeDesktop.cs') -ReferencedAssemblies System.dll,System.Core.dll,System.Drawing.dll,System.Windows.Forms.dll
}
function Write-Report([object] $Value, [string] $Path) {
    $Value | ConvertTo-Json -Depth 16 | Set-Content -LiteralPath $Path -Encoding UTF8
}
function Set-GateOutput([string] $Name, [string] $Value) {
    if ($env:GITHUB_OUTPUT) { "$Name=$Value" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding UTF8 }
}
function Test-DesktopFacts([object] $Facts) {
    return ($Facts.user_interactive -and $Facts.process_session_id -ne 0 -and
        $Facts.wts_connect_state -eq 0 -and $Facts.open_input_desktop -and
        $Facts.current_desktop_receives_input -and
        $Facts.current_desktop -eq $Facts.input_desktop -and
        $Facts.window_station -eq 'WinSta0' -and
        $Facts.primary_width -ge 800 -and $Facts.primary_height -ge 600 -and
        $Facts.virtual_width -le 8192 -and $Facts.virtual_height -le 8192)
}
function Assert-HostedDisposableRunner {
    # This is not a desktop/self-hosted test harness. No user machine fallback.
    if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
        throw 'Only a disposable GitHub-hosted Windows runner is allowed.'
    }
}
