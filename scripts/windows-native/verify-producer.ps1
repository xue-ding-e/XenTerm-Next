Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$h=@{Accept='application/vnd.github+json'}
$root='https://api.github.com/repos/xue-ding-e/XenTerm-Next/actions'
$a=Invoke-RestMethod -Uri "$root/artifacts/12261619305" -Headers $h -TimeoutSec 20
$r=Invoke-RestMethod -Uri "$root/runs/39415201204" -Headers $h -TimeoutSec 20
if ($a.id -ne 12261619305 -or $a.expired -or [DateTime]::UtcNow -ge [DateTime]::Parse($a.expires_at) -or $a.expires_at -ne '2026-10-23T11:47:44Z') { throw 'Artifact ID/expiry mismatch' }
if ($a.name -ne 'xenterm-validation-ad095a64a4054caa27a3f0fe7b556d901f252bf6-39415201204-1' -or $a.size_in_bytes -ne 102248733 -or $a.digest -ne 'sha256:647c06d50f2c6e7c5b872d4974e64fb4d29bc2dcb0385b8ab0c10ffd5cc6e53c') { throw 'Artifact metadata identity mismatch' }
if ($a.workflow_run.id -ne 39415201204 -or $a.workflow_run.repository_id -ne 1418449265 -or $a.workflow_run.head_repository_id -ne 1418449265 -or $a.workflow_run.head_sha -ne 'ad095a64a4054caa27a3f0fe7b556d901f252bf6' -or $a.workflow_run.head_branch -ne 'test/sidebar-resize-gap-20261020') { throw 'Artifact producer mismatch' }
if ($r.id -ne 39415201204 -or $r.workflow_id -ne 383994652 -or $r.run_attempt -ne 1 -or $r.event -ne 'workflow_dispatch' -or $r.path -ne '.github/workflows/ci.yml' -or $r.head_branch -ne 'test/sidebar-resize-gap-20261020' -or $r.head_sha -ne 'ad095a64a4054caa27a3f0fe7b556d901f252bf6' -or $r.status -ne 'completed' -or $r.conclusion -ne 'failure') { throw 'Known producer run identity/outcome mismatch' }
if (Test-Path "$env:RUNNER_TEMP/xenterm-exact-reuse") { throw 'Download directory must be new' }
# Do not print headers, token, signed URLs, or a full API response.
