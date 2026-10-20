Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$dir=Join-Path $env:RUNNER_TEMP 'xenterm-exact-reuse'
$files=@(Get-ChildItem -LiteralPath $dir -Force -Recurse)
if ($files.Count -ne 2 -or @($files | Where-Object { $_.PSIsContainer -or ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $_.Name -notin @('xenterm.exe','manifest.json') }).Count -ne 0) { throw 'Artifact must contain exactly two ordinary files' }
$manifestPath=Join-Path $dir 'manifest.json'
if ((Get-Item -LiteralPath $manifestPath).Length -gt 65536) { throw 'Unexpected manifest size' }
$m=Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($m.kind -ne 'isolated_ci_validation_binary_not_release' -or $m.repository -ne 'xue-ding-e/XenTerm-Next' -or $m.ref -ne 'refs/heads/test/sidebar-resize-gap-20261020' -or $m.event -ne 'workflow_dispatch') { throw 'Manifest scope mismatch' }
if ([string]$m.run_id -ne '39415201204' -or [string]$m.run_attempt -ne '1' -or $m.workflow_ref -ne 'xue-ding-e/XenTerm-Next/.github/workflows/ci.yml@refs/heads/test/sidebar-resize-gap-20261020' -or $m.sha -ne 'ad095a64a4054caa27a3f0fe7b556d901f252bf6') { throw 'Manifest build provenance mismatch' }
if ($m.src_tree -ne 'a8f4de26c9c0b0782b5cce12e629133664ce5a5d' -or $m.target -ne 'x86_64-pc-windows-msvc' -or $m.profile -ne 'dev' -or $m.binary -ne 'xenterm.exe') { throw 'Manifest build target/source mismatch' }
$objects=[ordered]@{
  'src'='a8f4de26c9c0b0782b5cce12e629133664ce5a5d'
  'Cargo.toml'='6646afc7299b2f533d9fdf4ea028f70866a2712f'
  'Cargo.lock'='b233737afbdb2126bb7be9bdb90360a5e961619a'
  'build.rs'='71fab7fb90ffab62e7111ef5b0d256976b6bd197'
  'assets'='9fea0ceab930f971e8295a6cf37380ac9cc649e1'
  'scripts/version.sh'='4b04370a93e8245a37895fb487ac9eef7cb1d4b7'
}
foreach ($key in $objects.Keys) {
  $oid=(& git rev-parse ('HEAD:'+$key) | Out-String).Trim()
  if ($LASTEXITCODE -ne 0 -or $oid -ne $objects[$key] -or $m.build_input_objects.$key -ne $objects[$key]) { throw ('Source object mismatch: '+$key) }
}
if ($m.cargo_lock_sha256 -ne 'fdc97d4a3f614c14b3b520b7d1b95f912393eb5d96082a533281207e2d104823') { throw 'Producer lockfile hash mismatch' }
$exe=Join-Path $dir 'xenterm.exe'
$hash=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
if ($hash -ne 'f6dfc60bb334247f788012ad52c9ad2faa126b48b8df11d2ec6313dc141325e0' -or $m.binary_sha256 -ne $hash -or $m.binary_bytes -ne 102243840 -or (Get-Item -LiteralPath $exe).Length -ne 102243840) { throw 'Executable byte identity mismatch' }
$harnessSha=(& git rev-parse HEAD | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $harnessSha -ne $env:GITHUB_SHA) { throw 'Harness checkout mismatch' }
$identity=[ordered]@{status='identity_verified_not_execution_pass';artifact_id=12261619305;producer_run=39415201204;build_sha=$m.sha;harness_sha=$harnessSha;src_tree=$m.src_tree;binary_sha256=$hash;producer_result=$m.app_result;producer_interaction=$m.interaction_result;source_objects=$objects;advertised_zip_digest='647c06d50f2c6e7c5b872d4974e64fb4d29bc2dcb0385b8ab0c10ffd5cc6e53c'}
$identity | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath "$env:RUNNER_TEMP/native-evidence/reuse-identity.json" -Encoding UTF8
"verified_exe=$exe" | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding UTF8
'identity_gate=verified' | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding UTF8
