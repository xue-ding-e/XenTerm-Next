<#
.SYNOPSIS
Pull XenTerm-Next main, build the Windows desktop, and publish a fixed runtime.
.DESCRIPTION
Uses a separate source-next checkout so development work is never overwritten.
Keeps one release cache, current/previous packages, and at most one pending build.
Running GUI/MCP processes are not stopped; pending updates apply on a later launch.
#>
param(
    [Parameter(Mandatory = $true)][string]$WorkspaceRoot,
    [ValidateRange(1, 64)][int]$Jobs = 4,
    [switch]$ForceBuild,
    [switch]$DryRun,
    [switch]$Pause
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'local-runtime.ps1')
$repository = 'https://github.com/xue-ding-e/XenTerm-Next.git'
$WorkspaceRoot = [IO.Path]::GetFullPath($WorkspaceRoot).TrimEnd('\')
$source = Assert-LocalChildPath $WorkspaceRoot (Join-Path $WorkspaceRoot 'source-next')
$target = Assert-LocalChildPath $WorkspaceRoot (Join-Path $WorkspaceRoot 'builds\windows-next')
$stage = Assert-LocalChildPath $WorkspaceRoot (Join-Path $WorkspaceRoot 'runtime\pending.tmp')
$pending = Assert-LocalChildPath $WorkspaceRoot (Join-Path $WorkspaceRoot 'runtime\pending')
$buildLock = $null
$runtimeLock = $null
$savedEnvironment = @{}
$exitCode = 0

function Invoke-LocalGit {
    param([string[]]$GitArguments)
    $result = & git @GitArguments
    if ($LASTEXITCODE -ne 0) { throw "git failed: $($GitArguments -join ' ')" }
    return $result
}

try {
    if ($DryRun) {
        Write-Host "Source: $source (origin/main; clean fast-forward updates only)"
        Write-Host "Build: cargo build --release --locked --bin xenterm --target x86_64-pc-windows-msvc --jobs $Jobs"
        Write-Host "Cache: $target"
        Write-Host "Runtime: $(Join-Path $WorkspaceRoot 'runtime\current\xenterm.exe')"
        Write-Host "Profile: $(Join-Path $WorkspaceRoot 'data')"
        return
    }
    foreach ($command in @('git', 'cargo', 'rustc')) {
        if (-not (Get-Command $command -ErrorAction SilentlyContinue)) { throw "Required command not found: $command" }
    }
    $gitDirectory = Split-Path -Parent (Split-Path -Parent (Get-Command git).Source)
    $bash = Join-Path $gitDirectory 'bin\bash.exe'
    if (-not (Test-Path -LiteralPath $bash)) { throw "Git Bash not found: $bash" }
    $buildLock = Enter-LocalRuntimeLock $WorkspaceRoot -Build
    Write-Host '[1/4] Updating the dedicated main checkout...'
    if (-not (Test-Path -LiteralPath $source)) {
        Invoke-LocalGit -GitArguments @('clone', '--single-branch', '--branch', 'main', $repository, $source)
    }
    $top = [string](Invoke-LocalGit -GitArguments @('-C', $source, 'rev-parse', '--show-toplevel'))
    if ([IO.Path]::GetFullPath($top) -ne $source) { throw 'Unexpected build checkout root.' }
    $remote = [string](Invoke-LocalGit -GitArguments @('-C', $source, 'remote', 'get-url', 'origin'))
    if ($remote -ne $repository) { throw "Unexpected build remote: $remote" }
    $branch = [string](Invoke-LocalGit -GitArguments @('-C', $source, 'branch', '--show-current'))
    if ($branch -ne 'main') { throw 'The dedicated build checkout must stay on main.' }
    $dirty = @(Invoke-LocalGit -GitArguments @('-C', $source, 'status', '--porcelain=v1', '--untracked-files=all'))
    if ($dirty.Count) { throw 'source-next contains local changes. Preserve/review them before updating.' }
    Invoke-LocalGit -GitArguments @('-C', $source, 'fetch', '--prune', 'origin', 'main')
    & git -C $source merge-base --is-ancestor HEAD origin/main
    if ($LASTEXITCODE -ne 0) { throw 'Build checkout diverged from origin/main; no reset or overwrite was attempted.' }
    Invoke-LocalGit -GitArguments @('-C', $source, 'merge', '--ff-only', 'origin/main')
    $commit = [string](Invoke-LocalGit -GitArguments @('-C', $source, 'rev-parse', 'HEAD'))
    Write-Host "Source commit: $commit"
    $ready = $false
    $readyName = ''
    if (-not $ForceBuild) {
        foreach ($name in @('pending', 'current')) {
            $directory = Join-Path $WorkspaceRoot "runtime\$name"
            if (Test-Path -LiteralPath (Join-Path $directory 'BUILD.json')) {
                try {
                    $package = Test-LocalPackage $directory
                    if ($package.commit -eq $commit) { $ready = $true; $readyName = $name; break }
                } catch { Write-Warning "Ignoring an invalid $name package: $($_.Exception.Message)" }
            }
        }
    }
    if (-not $ready) {
        Write-Host '[2/4] Building the Windows desktop (first build takes longer)...'
        foreach ($name in @('XT_BASH', 'XT_CHANNEL', 'XT_VERSION', 'GITHUB_SHA', 'GITHUB_REF_TYPE', 'GITHUB_REF_NAME', 'CARGO_TARGET_DIR')) {
            $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        }
        $env:XT_BASH = $bash
        $env:XT_CHANNEL = 'beta'
        $env:CARGO_TARGET_DIR = $target
        foreach ($name in @('XT_VERSION', 'GITHUB_SHA', 'GITHUB_REF_TYPE', 'GITHUB_REF_NAME')) {
            [Environment]::SetEnvironmentVariable($name, $null, 'Process')
        }
        Push-Location -LiteralPath $source
        try {
            & cargo build --release --locked --bin xenterm --target x86_64-pc-windows-msvc --jobs $Jobs
            if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE); current runtime was preserved." }
        } finally { Pop-Location }
        $exe = Join-Path $target 'x86_64-pc-windows-msvc\release\xenterm.exe'
        $version = Get-LocalVersion $exe
        if ($version -notmatch [regex]::Escape($commit.Substring(0, 7))) {
            throw 'The binary does not report the source commit; refusing to publish.'
        }
        Write-Host '[3/4] Verifying and staging the package...'
        Remove-LocalPackage $WorkspaceRoot $stage
        [IO.Directory]::CreateDirectory($stage) | Out-Null
        Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'xenterm.exe')
        foreach ($name in @('LICENSE', 'THIRD_PARTY_NOTICES.md', 'licenses', 'version.json')) {
            Copy-Item -LiteralPath (Join-Path $source $name) -Destination $stage -Recurse
        }
        $manifest = [ordered]@{
            repository = $repository
            commit = $commit
            runtime_version = $version
            sha256 = (Get-FileHash -LiteralPath (Join-Path $stage 'xenterm.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
            built_at = [DateTimeOffset]::Now.ToString('o')
            target = 'x86_64-pc-windows-msvc'
        }
        $manifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stage 'BUILD.json') -Encoding UTF8
        $null = Test-LocalPackage $stage
        $runtimeLock = Enter-LocalRuntimeLock $WorkspaceRoot
        Remove-LocalPackage $WorkspaceRoot $pending
        Move-Item -LiteralPath $stage -Destination $pending
    } else {
        Write-Host '[2/4] This commit is already built and verified; reusing it.'
        $runtimeLock = Enter-LocalRuntimeLock $WorkspaceRoot
        # An older pending build must never replace an already-current package.
        if ($readyName -eq 'current') { Remove-LocalPackage $WorkspaceRoot $pending }
    }
    Write-Host '[4/4] Applying the update and installing stable shortcuts...'
    $applied = Publish-LocalPending $WorkspaceRoot
    Set-LocalShortcuts $WorkspaceRoot (Join-Path $source 'scripts')
    if ($applied) { Write-Host 'Done. Open XenTerm Next from the desktop shortcut.' }
    else { Write-Host 'Done. The verified build is staged for the next launch after current processes exit.' }
} catch {
    Write-Host "FAILED: $($_.Exception.Message)" -ForegroundColor Red
    $exitCode = 1
} finally {
    foreach ($name in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
    }
    if ($runtimeLock) { $runtimeLock.Dispose() }
    if ($buildLock) { $buildLock.Dispose() }
    if ($Pause) { Read-Host 'Press Enter to close' | Out-Null }
}
exit $exitCode
