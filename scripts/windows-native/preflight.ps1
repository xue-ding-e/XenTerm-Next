param([Parameter(Mandatory=$true)][string]$EvidenceDir)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
$report=[ordered]@{stage='desktop_preflight';status='blocked';started_utc=[DateTime]::UtcNow.ToString('o');reason=$null;facts=$null;control=$null}
$form=$null; $oldDpi=[IntPtr]::Zero
try {
    . (Join-Path $PSScriptRoot 'common.ps1')
    Assert-HostedDisposableRunner
    # Per-thread awareness only, restored below. Does not change OS/display settings.
    $oldDpi=[XenTermNativeProbe.Desktop]::SetThreadDpiAwarenessContext([IntPtr](-4))
    if ($oldDpi -eq [IntPtr]::Zero) { throw 'Per-monitor screenshot coordinate context unavailable' }
    $facts=[XenTermNativeProbe.Desktop]::Inspect()
    $report.facts=$facts
    $report.os=Get-CimInstance Win32_OperatingSystem -OperationTimeoutSec 10 | Select-Object Caption,Version,BuildNumber,OSArchitecture
    $report.gpu= @(Get-CimInstance Win32_VideoController -OperationTimeoutSec 10 | Select-Object Name,DriverVersion,VideoProcessor,CurrentHorizontalResolution,CurrentVerticalResolution)
    $report.image=[ordered]@{os=$env:ImageOS;version=$env:ImageVersion;runner_arch=$env:RUNNER_ARCH;processor_arch=$env:PROCESSOR_ARCHITECTURE;ps_version=$PSVersionTable.PSVersion.ToString();clr_version=[Environment]::Version.ToString()}
    if (-not (Test-DesktopFacts $facts)) { throw 'No verified active input desktop. No control UI or product build attempted.' }
    $form=New-Object XenTermNativeProbe.ControlForm
    $form.Show()
    $timer=[Diagnostics.Stopwatch]::StartNew()
    $matched=$false; $lastError=$null
    while ($timer.Elapsed.TotalSeconds -lt 12 -and -not $matched) {
        [Windows.Forms.Application]::DoEvents()
        Start-Sleep -Milliseconds 150
        $image=$null
        try {
            $image=[XenTermNativeProbe.Desktop]::CaptureClient($form.Handle)
            $matched=[XenTermNativeProbe.Desktop]::ControlPixelsMatch($image)
            $image.Save((Join-Path $EvidenceDir 'control-client.png'),[Drawing.Imaging.ImageFormat]::Png)
        } catch { $lastError=$_.Exception.Message }
        finally { if ($image) { $image.Dispose() } }
    }
    $report.control=[ordered]@{mapped=[XenTermNativeProbe.Desktop]::IsWindowVisible($form.Handle);dpi=[XenTermNativeProbe.Desktop]::GetDpiForWindow($form.Handle);client_bounds=[XenTermNativeProbe.Desktop]::ClientBounds($form.Handle);known_screen_pixels_match=$matched;capture_method='Graphics.CopyFromScreen (visible client only)';last_capture_error=$lastError}
    if (-not $matched) { throw 'Mapped WinForms control did not produce verified screen pixels. Build remains blocked.' }
    $report.status='passed'
    $report.reason='Active desktop plus visible, unoccluded WinForms control with four matched screen color patches.'
} catch { $report.reason=$_.Exception.Message }
finally {
    if ($form) { $form.Close(); $form.Dispose() }
    if ($oldDpi -ne [IntPtr]::Zero) { [void][XenTermNativeProbe.Desktop]::SetThreadDpiAwarenessContext($oldDpi) }
    $report.finished_utc=[DateTime]::UtcNow.ToString('o')
    $report | ConvertTo-Json -Depth 16 | Set-Content -LiteralPath (Join-Path $EvidenceDir 'preflight.json') -Encoding UTF8
    if ($env:GITHUB_OUTPUT) { "desktop_gate=$($report.status)" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding UTF8 }
}
if ($report.status -ne 'passed') { Write-Warning $report.reason; exit 20 }
exit 0
