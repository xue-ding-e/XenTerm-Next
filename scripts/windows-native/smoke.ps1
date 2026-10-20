param([Parameter(Mandatory=$true)][string]$SourceDir,[Parameter(Mandatory=$true)][string]$EvidenceDir,[Parameter(Mandatory=$true)][string]$BuildEvidenceDir,[Parameter(Mandatory=$true)][string]$PreflightDir)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-HostedDisposableRunner
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
$report=[ordered]@{stage='application_startup_render';status='blocked';startup='not_run';map='not_run';render='not_run';interaction='not_run';reason=$null;screenshots=@()}
$process=$null; $oldDpi=[IntPtr]::Zero
# Keep the synthetic profile outside uploaded evidence. Never inspect its key/database.
$sandboxRoot=Join-Path $env:RUNNER_TEMP ('xenterm-native-empty-'+[Guid]::NewGuid().ToString('N'))
$profile=Join-Path $sandboxRoot 'profile'
try {
    $gate=Get-Content -Raw -LiteralPath (Join-Path $PreflightDir 'preflight.json') | ConvertFrom-Json
    if ($gate.status -ne 'passed' -or ([DateTime]::UtcNow-[DateTime]::Parse($gate.finished_utc)).TotalMinutes -gt 3) { throw 'Immediate prelaunch desktop control is missing, failed, or stale' }
    $build=Get-Content -Raw -LiteralPath (Join-Path $BuildEvidenceDir 'build-metadata.json') | ConvertFrom-Json
    if ($build.status -ne 'passed' -or $build.sha -ne $env:GITHUB_SHA -or $build.src_tree -ne 'c068475846a19b8289e1eead56ac50d2a0cdaa74') { throw 'No validated candidate build' }
    $exe=Join-Path $SourceDir 'target\x86_64-pc-windows-msvc\debug\xenterm.exe'
    $binaryHash=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($binaryHash -ne $build.binary_sha256) { throw 'Binary changed after build' }
    $report.sha=$build.sha; $report.src_tree=$build.src_tree; $report.binary_sha256=$binaryHash
    $oldDpi=[XenTermNativeProbe.Desktop]::SetThreadDpiAwarenessContext([IntPtr](-4))
    if ($oldDpi -eq [IntPtr]::Zero) { throw 'Cannot establish screenshot DPI context' }
    $report.desktop=[XenTermNativeProbe.Desktop]::Inspect()
    if (-not (Test-DesktopFacts $report.desktop)) { throw 'Desktop changed after preflight' }
    New-Item -ItemType Directory -Path $profile -Force | Out-Null
    $report.profile='new, empty, explicit --data-dir; contents not collected'
    $arguments='--data-dir "'+$profile+'" gpui'
    # Process-local overrides only, inherited by our child and immediately restored.
    # Windows Known Folder / Shell COM may ignore these. This is defense in depth,
    # not an OS sandbox; the hosted-runner-only gate is mandatory.
    $savedEnvironment=@{}
    $isolatedNames=@('APPDATA','LOCALAPPDATA','USERPROFILE','HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','TEMP','TMP')
    try {
        foreach ($name in $isolatedNames) {
            $savedEnvironment[$name]=[Environment]::GetEnvironmentVariable($name,'Process')
            $isolated=Join-Path $sandboxRoot $name
            New-Item -ItemType Directory -Path $isolated -Force | Out-Null
            [Environment]::SetEnvironmentVariable($name,$isolated,'Process')
        }
        $process=[XenTermNativeProbe.OwnedProcess]::new($exe,$arguments,$SourceDir,70)
    } finally {
        foreach ($name in $savedEnvironment.Keys) { [Environment]::SetEnvironmentVariable($name,$savedEnvironment[$name],'Process') }
    }
    $report.pid=$process.Pid; $report.startup='process_created'
    $report.isolated_environment_names=$isolatedNames
    $report.isolation_limit='Windows Known Folder and startup Shell COM jump-list access are not intercepted. Disposable hosted runner required.'
    $sessionProcess=[Diagnostics.Process]::GetProcessById($process.Pid)
    try { $report.process_session_id=$sessionProcess.SessionId } finally { $sessionProcess.Dispose() }
    if ($report.process_session_id -ne $report.desktop.process_session_id) { throw 'Application started in a different session' }
    $report.pid=$process.Pid; $report.startup='process_created'; $report.map='not_observed'; $report.render='not_observed'
    $watch=[Diagnostics.Stopwatch]::StartNew(); $lastCapture=-100.0; $lastError=$null; $richFrames=0
    while ($watch.Elapsed.TotalSeconds -lt 55) {
        if ($process.HasExited) { $report.exit_code=$process.ExitCode; throw 'Application exited during startup observation' }
        $windows=@([XenTermNativeProbe.Desktop]::WindowsForProcess($process.Pid))
        foreach ($window in $windows) {
            $handle=[IntPtr]$window
            # Debug console windows are not XenTerm client content.
            $class=[XenTermNativeProbe.Desktop]::WindowClass($handle)
            if ($class -eq 'ConsoleWindowClass') { continue }
            $bounds=[XenTermNativeProbe.Desktop]::ClientBounds($handle)
            if ($bounds.Width -lt 400 -or $bounds.Height -lt 250) { continue }
            $report.map='visible_top_level_window_observed'
            if ($watch.Elapsed.TotalSeconds-$lastCapture -lt 5) { continue }
            $image=$null
            try {
                $image=[XenTermNativeProbe.Desktop]::CaptureClient($handle)
                $metrics=[XenTermNativeProbe.Desktop]::PixelMetrics($image)
                $name='app-client-'+($report.screenshots.Count+1).ToString('00')+'.png'
                $image.Save((Join-Path $EvidenceDir $name),[Drawing.Imaging.ImageFormat]::Png)
                $report.screenshots+=@([ordered]@{file=$name;elapsed_seconds=[Math]::Round($watch.Elapsed.TotalSeconds,2);window_handle=$window;window_class=$class;window_dpi=[XenTermNativeProbe.Desktop]::GetDpiForWindow($handle);bounds=$bounds;metrics=$metrics})
                $lastCapture=$watch.Elapsed.TotalSeconds
                if ($metrics.nonblank_heuristic) { $richFrames++ }
            } catch { $lastError=$_.Exception.Message }
            finally { if ($image) { $image.Dispose() } }
        }
        if ($richFrames -ge 3) { break }
        Start-Sleep -Milliseconds 250
    }
    if ($process.HasExited) { $report.exit_code=$process.ExitCode; throw 'Application exited before observation completed' }
    $report.last_capture_error=$lastError
    if ($report.screenshots.Count -eq 0) { throw 'No unoccluded on-screen application client capture; cannot attribute failure to renderer' }
    if ($richFrames -eq 0) { $report.status='failed'; $report.render='blank_or_insufficient_pixel_variation'; $report.reason='Control passed but captured app clients did not satisfy nonblank heuristic. Inspect screenshots; this is not an interaction result.' }
    else { $report.status='needs_visual_review'; $report.render='nonblank_pixels_captured_semantics_unverified'; $report.reason='Inspect all client PNGs for actual XenTerm shell text/layout. Pixel variation is not proof of correct rendering; no keyboard/mouse flow was exercised.' }
} catch { $report.reason=$_.Exception.Message; if ($report.startup -eq 'process_created' -and $process -and $process.HasExited) { $report.status='failed' } }
finally {
    if ($process) { $process.Dispose(); $report.cleanup_confirmed=$process.CleanupConfirmed; if (-not $process.CleanupConfirmed) { $report.status='blocked'; $report.reason='Application job cleanup could not be confirmed' } }
    if ($oldDpi -ne [IntPtr]::Zero) { [void][XenTermNativeProbe.Desktop]::SetThreadDpiAwarenessContext($oldDpi) }
    # The profile was created by this run, never a user profile or uploaded artifact.
    try {
        if (Test-Path -LiteralPath $sandboxRoot) { Remove-Item -LiteralPath $sandboxRoot -Recurse -Force }
        $report.profile_cleanup='confirmed'
    } catch { $report.profile_cleanup='failed'; $report.status='blocked'; $report.reason='Synthetic profile cleanup failed: '+$_.Exception.Message }
    Write-Report $report (Join-Path $EvidenceDir 'smoke.json')
    Set-GateOutput 'smoke_gate' $report.status
}
if ($report.status -eq 'failed' -or $report.status -eq 'blocked') { throw $report.reason }
exit 0
