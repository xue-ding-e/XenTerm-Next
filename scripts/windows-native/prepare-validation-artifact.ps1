param([Parameter(Mandatory=$true)][string]$SourceDir,[Parameter(Mandatory=$true)][string]$BuildEvidenceDir,[Parameter(Mandatory=$true)][string]$AppEvidenceDir,[Parameter(Mandatory=$true)][string]$PreflightDir,[Parameter(Mandatory=$true)][string]$OutputDir)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
Assert-HostedDisposableRunner
if ($env:GITHUB_REPOSITORY -ne 'xue-ding-e/XenTerm-Next' -or $env:GITHUB_REF -ne 'refs/heads/test/sidebar-resize-gap-20261020' -or $env:GITHUB_EVENT_NAME -ne 'workflow_dispatch') { throw 'Validation artifact scope mismatch' }
$build=Get-Content -Raw -LiteralPath (Join-Path $BuildEvidenceDir 'build-metadata.json') | ConvertFrom-Json
$app=Get-Content -Raw -LiteralPath (Join-Path $AppEvidenceDir 'smoke.json') | ConvertFrom-Json
$preflight=Get-Content -Raw -LiteralPath (Join-Path $PreflightDir 'preflight.json') | ConvertFrom-Json
if ($preflight.status -ne 'passed') { throw 'Missing verified native environment metadata' }
$sha=(& git -C $SourceDir rev-parse HEAD | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $sha -ne $env:GITHUB_SHA -or $build.sha -ne $sha -or $app.sha -ne $sha -or $build.status -ne 'passed' -or $app.cleanup_confirmed -ne $true -or $app.profile_cleanup -ne 'confirmed') { throw 'Build identity or owned-process/profile cleanup not proven' }
$exe=Join-Path $SourceDir 'target\x86_64-pc-windows-msvc\debug\xenterm.exe'
$hash=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
if ($hash -ne $build.binary_sha256 -or $hash -ne $app.binary_sha256 -or $build.src_tree -ne 'a8f4de26c9c0b0782b5cce12e629133664ce5a5d') { throw 'Validation binary identity mismatch' }
if (Test-Path -LiteralPath $OutputDir) { throw 'Artifact staging directory must be new' }
New-Item -ItemType Directory -Path $OutputDir | Out-Null
Copy-Item -LiteralPath $exe -Destination (Join-Path $OutputDir 'xenterm.exe')
if ((Get-FileHash -LiteralPath (Join-Path $OutputDir 'xenterm.exe') -Algorithm SHA256).Hash.ToLowerInvariant() -ne $hash) { throw 'Staged binary differs' }
$manifest=[ordered]@{
    kind='isolated_ci_validation_binary_not_release';repository=$env:GITHUB_REPOSITORY;ref=$env:GITHUB_REF;event=$env:GITHUB_EVENT_NAME
    run_id=$env:GITHUB_RUN_ID;run_attempt=$env:GITHUB_RUN_ATTEMPT;workflow_ref=$env:GITHUB_WORKFLOW_REF;sha=$sha
    src_tree=$build.src_tree;build_input_objects=$build.build_input_objects;cargo_lock_sha256=$build.cargo_lock_sha256
    target='x86_64-pc-windows-msvc';profile='dev';binary='xenterm.exe';binary_sha256=$hash;binary_bytes=(Get-Item -LiteralPath $exe).Length
    rustc=$build.rustc;cargo=$build.cargo;msvc_cl=$build.msvc_cl;version_metadata=$build.version_metadata
    os=$preflight.os;gpu=$preflight.gpu;runner_image=$preflight.image;desktop=$app.desktop;app_result=$app.status;interaction_result=$app.interaction;full_window_verified=$false
    created_utc=[DateTime]::UtcNow.ToString('o');retention_days=3
    limitations='Not signed/released. Identity is bound to this build/run, not to future harness HEAD. Consumer must verify artifact/run provenance and hash. Native interaction semantics still require screenshot review.'
}
Write-Report $manifest (Join-Path $OutputDir 'manifest.json')
Set-GateOutput 'validation_binary_gate' 'verified'
