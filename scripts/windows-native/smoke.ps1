param([Parameter(Mandatory=$true)][string]$SourceDir,[Parameter(Mandatory=$true)][string]$EvidenceDir,[Parameter(Mandatory=$true)][string]$VerifiedDir,[Parameter(Mandatory=$true)][string]$PreflightDir,[switch]$RunLimitedInteractions)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-HostedDisposableRunner
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
$report=[ordered]@{stage='visible_application_startup';capture_scope='visible_client_intersection_only';full_window_verified=$false;status='blocked';startup='not_run';map='not_run';render='not_run';interaction='not_run';reason=$null;screenshots=@()}
$process=$null; $oldDpi=[IntPtr]::Zero
# Keep the synthetic profile outside uploaded evidence. Never inspect its key/database.
$sandboxRoot=Join-Path $env:RUNNER_TEMP ('xenterm-native-empty-'+[Guid]::NewGuid().ToString('N'))
$profile=Join-Path $sandboxRoot 'profile'
try {
    $gate=Get-Content -Raw -LiteralPath (Join-Path $PreflightDir 'preflight.json') | ConvertFrom-Json
    if ($gate.status -ne 'passed' -or ([DateTime]::UtcNow-[DateTime]::Parse($gate.finished_utc)).TotalMinutes -gt 3) { throw 'Immediate prelaunch desktop control is missing, failed, or stale' }
    $manifest=Get-Content -Raw -LiteralPath (Join-Path $VerifiedDir 'manifest.json') | ConvertFrom-Json
    $identity=Get-Content -Raw -LiteralPath "$env:RUNNER_TEMP/native-evidence/reuse-identity.json" | ConvertFrom-Json
    $harnessSha=(& git -C $SourceDir rev-parse HEAD | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $harnessSha -ne $env:GITHUB_SHA -or $identity.harness_sha -ne $harnessSha -or $identity.status -ne 'identity_verified_not_execution_pass') { throw 'Harness or prior identity gate mismatch' }
    $src=(& git -C $SourceDir rev-parse HEAD:src | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $src -ne 'a8f4de26c9c0b0782b5cce12e629133664ce5a5d' -or $manifest.src_tree -ne $src -or $manifest.sha -ne 'ad095a64a4054caa27a3f0fe7b556d901f252bf6' -or $identity.build_sha -ne $manifest.sha) { throw 'Source or producer identity changed before launch' }
    $exe=Join-Path $VerifiedDir 'xenterm.exe'
    $binaryHash=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($binaryHash -ne 'f6dfc60bb334247f788012ad52c9ad2faa126b48b8df11d2ec6313dc141325e0' -or $binaryHash -ne $manifest.binary_sha256 -or (Get-Item -LiteralPath $exe).Length -ne 102243840) { throw 'Binary changed after identity verification' }
    $report.build_sha=$manifest.sha; $report.harness_sha=$harnessSha; $report.src_tree=$src; $report.binary_sha256=$binaryHash
    $report.producer_run=39415201204; $report.producer_conclusion='failure'; $report.reused_artifact=12261619305
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
    if ($RunLimitedInteractions) {
        . (Join-Path $PSScriptRoot 'limited-interactions.ps1')
        $interaction=Invoke-LimitedInteractions $process $EvidenceDir
        $report.stage='visible_startup_and_limited_native_input'
        $report.status=$interaction.status; $report.reason=$interaction.reason
        $report.interaction=$interaction.status
        $report.screenshots=$interaction.screenshots
        if ($interaction.screenshots.Count -gt 0) { $report.map='visible_top_level_window_observed'; $report.render='trusted_home_roi_match_observed' }
        if ($interaction.status -ne 'needs_visual_review') { throw $interaction.reason }
    } else {
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
            $capture=$null
            try {
                $capture=[XenTermNativeProbe.Desktop]::CaptureVisibleClient($handle,$process.Pid)
                $image=$capture.Image
                $metrics=[XenTermNativeProbe.Desktop]::PixelMetrics($image)
                $name='app-visible-client-'+($report.screenshots.Count+1).ToString('00')+'.png'
                $image.Save((Join-Path $EvidenceDir $name),[Drawing.Imaging.ImageFormat]::Png)
                $report.screenshots+=@([ordered]@{file=$name;elapsed_seconds=[Math]::Round($watch.Elapsed.TotalSeconds,2);window_handle=$window;window_class=$class;window_dpi=[XenTermNativeProbe.Desktop]::GetDpiForWindow($handle);original_client_bounds=$capture.OriginalBounds;visible_client_bounds=$capture.VisibleBounds;desktop_bounds=$capture.DesktopBounds;work_area_bounds=$capture.WorkAreaBounds;partial=$capture.Partial;full_window_verified=$false;metrics=$metrics})
                $lastCapture=$watch.Elapsed.TotalSeconds
                if ($metrics.nonblank_heuristic) { $richFrames++ }
            } catch { $lastError=$_.Exception.Message }
            finally { if ($capture) { $capture.Dispose() } }
        }
        if ($richFrames -ge 3) { break }
        Start-Sleep -Milliseconds 250
    }
    if ($process.HasExited) { $report.exit_code=$process.ExitCode; throw 'Application exited before observation completed' }
    $report.last_capture_error=$lastError
    if ($report.screenshots.Count -eq 0) { throw 'No unoccluded visible application client intersection capture; cannot attribute failure to renderer' }
    if ($richFrames -eq 0) { $report.status='failed'; $report.render='visible_region_blank_or_insufficient_pixel_variation'; $report.reason='Control passed but captured visible app regions did not satisfy nonblank heuristic. Inspect screenshots; this is not an interaction result.' }
    else { $report.status='needs_visual_review'; $report.render='visible_region_nonblank_semantics_unverified'; $report.reason='Inspect visible-client PNGs for XenTerm text/layout. Off-screen client content and full-window layout are unverified even if this crop is legible. Pixel variation is not proof of correct rendering; no keyboard/mouse flow was exercised.' }
    }
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
