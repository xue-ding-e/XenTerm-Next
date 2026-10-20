param([Parameter(Mandatory=$true)][string]$SourceDir,[Parameter(Mandatory=$true)][string]$EvidenceDir,[Parameter(Mandatory=$true)][string]$PreflightDir)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-HostedDisposableRunner
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
$report=[ordered]@{stage='candidate_build';status='failed';expected_run_sha=$env:GITHUB_SHA;product_source_sha='772da7651e0295c53d896ba04f90f0db078d8dc0';expected_src_tree='c068475846a19b8289e1eead56ac50d2a0cdaa74';target='x86_64-pc-windows-msvc';features='default desktop + remote-mcp';profile='dev';reason=$null}
$process=$null
try {
    $gate=Get-Content -Raw -LiteralPath (Join-Path $PreflightDir 'preflight.json') | ConvertFrom-Json
    if ($gate.status -ne 'passed') { throw 'Build blocked: desktop preflight did not pass' }
    $powershell=Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    # Child emits metadata/logs, not environment dumps or credentials. All tools it
    # invokes inherit the private job, which is killed on timeout or controller exit.
    $child=Join-Path $PSScriptRoot 'build-worker.ps1'
    $arguments='-NoLogo -NoProfile -NonInteractive -File "'+$child+'" -SourceDir "'+$SourceDir+'" -EvidenceDir "'+$EvidenceDir+'"'
    $process=[XenTermNativeProbe.OwnedProcess]::new($powershell,$arguments,$SourceDir,2400)
    [void]$process.Wait(2405000)
    $report.timed_out=$process.TimedOut
    if (-not $process.HasExited) { throw 'Build process did not finish within deadline' }
    $report.exit_code=$process.ExitCode
    if ($process.TimedOut -or $process.ExitCode -ne 0) { throw 'Candidate build failed or timed out; see build.log and build-metadata.json' }
    $meta=Get-Content -Raw -LiteralPath (Join-Path $EvidenceDir 'build-metadata.json') | ConvertFrom-Json
    if ($meta.sha -ne $report.expected_run_sha -or $meta.src_tree -ne $report.expected_src_tree -or $meta.status -ne 'passed') { throw 'Build metadata identity mismatch' }
    $report.metadata=$meta
    $report.status='passed'
} catch { $report.reason=$_.Exception.Message }
finally {
    if ($process) { $process.Dispose(); $report.cleanup_confirmed=$process.CleanupConfirmed; if (-not $process.CleanupConfirmed) { $report.status='failed'; $report.reason='Owned build job cleanup could not be confirmed' } }
    Write-Report $report (Join-Path $EvidenceDir 'build.json')
    Set-GateOutput 'build_gate' $report.status
}
if ($report.status -ne 'passed') { throw $report.reason }
exit 0
