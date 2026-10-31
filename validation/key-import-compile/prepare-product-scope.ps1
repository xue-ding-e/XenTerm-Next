# Own only fresh ephemeral output and move the fixed secondary checkout outside control.
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$root=Join-Path $env:RUNNER_TEMP 'key-import-compile-evidence'
function Plain([string]$Path){if(-not[IO.Path]::IsPathFullyQualified($Path) -or [IO.Path]::GetFullPath($Path) -ne $Path){throw 'canonical path'};$p=$Path;while($p){if(Test-Path -LiteralPath $p){if(((Get-Item -LiteralPath $p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'reparse path'}};$p=[IO.Path]::GetDirectoryName($p)}}
if(-not $IsWindows -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:RUNNER_ARCH -ne 'X64' -or $env:GITHUB_REPOSITORY -ne 'xue-ding-e/XenTerm-Next' -or $env:GITHUB_REF -ne 'refs/heads/validation/key-import-compile-only-20261030' -or $env:GITHUB_EVENT_NAME -ne 'workflow_dispatch' -or $env:GITHUB_SHA -ne $env:EXPECTED_CONTROL_SHA -or $env:GITHUB_SHA -notmatch '^[0-9a-f]{40}$'){throw 'fixed scope'}
Plain $root
if(Test-Path -LiteralPath $root){throw 'fresh output required'}
$source=Join-Path $env:GITHUB_WORKSPACE '_product_checkout';Plain $source
$head=& git -c core.fsmonitor=false -C $source rev-parse HEAD
if($LASTEXITCODE -ne 0 -or $head -ne '459b593c830b8be643df29f9d9c0c46915d54696'){throw 'fixed product source'}
$status=& git -c core.fsmonitor=false -C $source status --porcelain --untracked-files=all --ignored
if($LASTEXITCODE -ne 0 -or $status){throw 'source not clean'}
[IO.Directory]::CreateDirectory($root)|Out-Null
Move-Item -LiteralPath $source -Destination (Join-Path $root 'source')
$head=& git -c core.fsmonitor=false -C $env:GITHUB_WORKSPACE rev-parse HEAD
if($LASTEXITCODE -ne 0 -or $head -ne $env:GITHUB_SHA){throw 'control head'}
$status=& git -c core.fsmonitor=false -C $env:GITHUB_WORKSPACE status --porcelain --untracked-files=all
if($LASTEXITCODE -ne 0 -or $status){throw 'control not clean after source move'}
