Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-HostedDisposableRunner
$evidence="$env:RUNNER_TEMP/native-evidence"
$staging=Join-Path $env:RUNNER_TEMP ('xenterm-shader-source-'+[Guid]::NewGuid().ToString('N'))
$report=[ordered]@{status='blocked';input_events=0;reason=$null;cleanup='not_run'}
try {
    New-Item -ItemType Directory -Path $staging | Out-Null
    & python (Join-Path $PSScriptRoot 'shader-source.py') probe $evidence
    if ($LASTEXITCODE -ne 0) { throw 'Fixed shader paths are not all absent; stop hypothesis without changes' }
    try { & (Join-Path $PSScriptRoot 'smoke.ps1') -SourceDir $env:GITHUB_WORKSPACE -EvidenceDir "$evidence/before" -VerifiedDir "$env:RUNNER_TEMP/xenterm-exact-reuse" -PreflightDir "$evidence/prelaunch" } catch { $report.baseline_exception=$_.Exception.Message }
    $before=Get-Content -Raw -LiteralPath "$evidence/before/smoke.json" | ConvertFrom-Json
    if ($before.exit_code -ne 101 -or $before.exit_code_hex -ne '0x00000065' -or $before.accepted_input_events -ne 0 -or -not $before.cleanup_confirmed -or $before.profile_cleanup -ne 'confirmed' -or -not $before.stderr_reader_stopped -or $before.stderr_truncated -or $before.stderr_error -or $before.stderr_text -notmatch 'failed to initialize DirectX:.*Recreating GPU state for DirectWrite:.*path specified\. \(os error 3\)') { throw 'Baseline did not reproduce exact known clean failure; no restoration' }
    & python (Join-Path $PSScriptRoot 'shader-source.py') restore $evidence $staging
    if ($LASTEXITCODE -ne 0) { throw 'Locked source restoration failed; do not launch after state' }
    & (Join-Path $PSScriptRoot 'preflight.ps1') -EvidenceDir "$evidence/after-prelaunch"
    if ($LASTEXITCODE -ne 0) { throw 'Immediate after desktop preflight failed' }
    & (Join-Path $PSScriptRoot 'smoke.ps1') -SourceDir $env:GITHUB_WORKSPACE -EvidenceDir "$evidence/after" -VerifiedDir "$env:RUNNER_TEMP/xenterm-exact-reuse" -PreflightDir "$evidence/after-prelaunch"
    $after=Get-Content -Raw -LiteralPath "$evidence/after/smoke.json" | ConvertFrom-Json
    if ($after.status -ne 'needs_visual_review' -or $after.accepted_input_events -ne 0 -or -not $after.cleanup_confirmed -or $after.profile_cleanup -ne 'confirmed' -or -not $after.stderr_reader_stopped) { throw 'After observation incomplete; no navigation conclusion' }
    $report.status='needs_visual_review'; $report.reason='Only three locked shader sources changed; no-input before/after recorded. Navigation not tested.'
} catch { $report.reason=$_.Exception.Message }
finally {
    & python (Join-Path $PSScriptRoot 'shader-source.py') cleanup $evidence
    if ($LASTEXITCODE -ne 0) { $report.status='blocked'; $report.cleanup='failed' } else { $report.cleanup='confirmed' }
    try { if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force } } catch { $report.status='blocked'; $report.cleanup='staging_cleanup_failed' }
    Write-Report $report "$evidence/shader-experiment.json"
    Set-GateOutput 'experiment_gate' $report.status
}
if ($report.status -ne 'needs_visual_review') { throw $report.reason }
