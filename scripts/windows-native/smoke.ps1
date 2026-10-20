param([Parameter(Mandatory=$true)][string]$SourceDir,[Parameter(Mandatory=$true)][string]$EvidenceDir,[Parameter(Mandatory=$true)][string]$VerifiedDir,[Parameter(Mandatory=$true)][string]$PreflightDir)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-HostedDisposableRunner
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
$report=[ordered]@{stage='visible_application_startup';capture_scope='visible_client_intersection_only';full_window_verified=$false;status='blocked';startup='not_run';map='not_run';render='not_run';interaction='not_run';reason=$null;screenshots=@()}
$report.accepted_input_events=0; $report.input_policy='three reviewed navigation clicks, Ctrl+K and guarded Escape; fifteen events maximum; no text/Enter/selection or retry'
$report.stderr_capture='child stderr only; retain first 65536 bytes while draining; stdin/stdout NUL'
$process=$null; $oldDpi=[IntPtr]::Zero
# Keep the synthetic profile outside uploaded evidence. Never inspect its key/database.
$sandboxRoot=Join-Path $env:RUNNER_TEMP ('xenterm-native-empty-'+[Guid]::NewGuid().ToString('N'))
$profile=Join-Path $sandboxRoot 'profile'
try {
    $gate=Get-Content -Raw -LiteralPath (Join-Path $PreflightDir 'preflight.json') | ConvertFrom-Json
    if ($gate.status -ne 'passed' -or ([DateTime]::UtcNow-[DateTime]::Parse($gate.finished_utc)).TotalMinutes -gt 3) { throw 'Immediate prelaunch desktop control is missing, failed, or stale' }
    $identity=Get-Content -Raw -LiteralPath "$env:RUNNER_TEMP/native-evidence/release-identity.json" | ConvertFrom-Json
    $harnessSha=(& git -C $SourceDir rev-parse HEAD | Out-String).Trim()
    $expectedPackage=Join-Path $env:RUNNER_TEMP 'xenterm-published-77ce731/XenTerm-Next-beta-77ce7313b71d176c26f62ec199b34aaad91c2251-windows-x86_64'
    if ($LASTEXITCODE -ne 0 -or $harnessSha -ne $env:GITHUB_SHA -or $identity.harness_sha -ne $harnessSha -or $identity.status -ne 'published_release_identity_verified_not_execution_pass' -or [IO.Path]::GetFullPath($VerifiedDir) -ne [IO.Path]::GetFullPath($expectedPackage) -or $identity.package_dir -ne $VerifiedDir) { throw 'Published package or harness identity mismatch' }
    if ($identity.build_sha -ne '77ce7313b71d176c26f62ec199b34aaad91c2251' -or $identity.zip_sha256 -ne '2739215d5555a7819ac390ce08372fcaff6fe44ca2e6830e8df2b1cbf87f86c6' -or $identity.runtime_version -ne 'xt-beta-77ce731-T1792516261') { throw 'Pinned published source/version mismatch' }
    $exe=Join-Path $VerifiedDir 'xenterm.exe'
    $binaryHash=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($binaryHash -ne $identity.binary_sha256 -or (Get-Item -LiteralPath $exe).Length -ne $identity.binary_bytes -or ((Get-Item -LiteralPath $exe).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Published executable changed after byte verification' }
    $report.build_sha=$identity.build_sha; $report.harness_sha=$harnessSha; $report.binary_sha256=$binaryHash
    $report.runtime_version=$identity.runtime_version; $report.ci_run=$identity.ci_run; $report.build_run=$identity.build_run
    $report.harness_source_is_not_application_source=$true
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
        $savedEnvironment['RUST_BACKTRACE']=[Environment]::GetEnvironmentVariable('RUST_BACKTRACE','Process')
        [Environment]::SetEnvironmentVariable('RUST_BACKTRACE','1','Process')
        $process=[XenTermNativeProbe.OwnedProcess]::new($exe,$arguments,$SourceDir,70,$true)
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
    # Exact release reference and fresh-frame/foreground guards are checked before input.
    . (Join-Path $PSScriptRoot 'limited-interactions.ps1')
    $interaction=Invoke-LimitedInteractions $process $EvidenceDir
    $report.interaction=$interaction.status
    if ($interaction.Contains('accepted_input_events')) { $report.accepted_input_events=$interaction.accepted_input_events }
    $report.status=$interaction.status
    $report.reason=$interaction.reason
    $report.map='see_interactions_evidence'; $report.render='semantic_page_requires_visual_review'

} catch {
    $report.reason=$_.Exception.Message
    if ($report.startup -eq 'process_created' -and $process -and $process.HasExited) {
        $report.status='failed'; $report.exit_code=$process.ExitCode
        $unsigned=[BitConverter]::ToUInt32([BitConverter]::GetBytes([int]$report.exit_code),0)
        $report.exit_code_hex=('0x{0:X8}' -f $unsigned)
    }
}
finally {
    if ($process) { $process.Dispose(); $report.stderr_text=$process.StderrText; $report.stderr_truncated=$process.StderrTruncated; $report.stderr_reader_stopped=$process.StderrReaderStopped; $report.stderr_error=$process.StderrError; if (-not $process.StderrReaderStopped) { $report.status='blocked'; $report.reason='Stderr reader cleanup was not confirmed' }; $report.cleanup_confirmed=$process.CleanupConfirmed; if (-not $process.CleanupConfirmed) { $report.status='blocked'; $report.reason='Application job cleanup could not be confirmed' } }
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
