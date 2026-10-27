<#
.SYNOPSIS
Preview or remove obsolete local build outputs; preserve profiles and WSL disks.
.DESCRIPTION
Pass -Apply to delete. The reusable Windows release cache and current/previous/
pending packages are retained. Only explicitly named legacy output roots qualify.
#>
param([Parameter(Mandatory = $true)][string]$WorkspaceRoot, [switch]$Apply)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'local-runtime.ps1')
$WorkspaceRoot = [IO.Path]::GetFullPath($WorkspaceRoot).TrimEnd('\')
$buildLock = Enter-LocalRuntimeLock $WorkspaceRoot -Build
$runtimeLock = $null
$removedBytes = [long]0

function Get-DisposableCacheItems {
    param([string]$Path, [object[]]$Items)
    $protected = @($Items | Where-Object {
        ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
        $_.Name -match '^(sessions\.(db.*|json.*)|secret\.key|known_hosts|\.git)$' -or
        $_.Extension -eq '.vhdx'
    } | ForEach-Object {
        if ($_.PSIsContainer -or ($_.Attributes -band [IO.FileAttributes]::ReparsePoint)) { $_.FullName }
        else { $_.DirectoryName }
    } | Select-Object -Unique)
    if (-not $protected.Count) { return Get-Item -LiteralPath $Path }
    $ancestors = New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $protected) {
        Write-Host "KEEP (profile/data, not traversed for removal): $entry"
        $probe = $entry
        while ($probe.Length -ge $Path.Length) {
            $null = $ancestors.Add($probe)
            $probe = Split-Path -Parent $probe
        }
    }
    foreach ($item in $Items) {
        # Keep every ancestor of a protected profile; only remove its other children.
        if ($ancestors.Contains($item.FullName)) { continue }
        $keep = $false
        foreach ($entry in $protected) {
            if ($item.FullName -eq $entry -or $item.FullName.StartsWith($entry + '\', [StringComparison]::OrdinalIgnoreCase)) {
                $keep = $true; break
            }
        }
        if (-not $keep -and $ancestors.Contains((Split-Path -Parent $item.FullName))) { $item }
    }
}

try {
    $runtimeLock = Enter-LocalRuntimeLock $WorkspaceRoot
    $processes = @(Get-CimInstance Win32_Process)
    $legacy = @('access-modes-20261012', 'access-modes-60s-20261012',
        'approval-fixed-20261012', 'approval-ui-20261012', 'execution-history-20261012',
        'next-beta-1f97dd843446', 'next-beta-bfa7b42dfd5c')
    foreach ($name in $legacy) {
        $path = Assert-LocalChildPath $WorkspaceRoot (Join-Path $WorkspaceRoot "runtime\$name")
        if (-not (Test-Path -LiteralPath $path)) { continue }
        $busy = @($processes | Where-Object {
            $_.ExecutablePath -and $_.ExecutablePath.StartsWith($path + '\', [StringComparison]::OrdinalIgnoreCase)
        })
        if ($busy.Count) { Write-Host "KEEP (running): $path"; continue }
        $items = @(Get-LocalTreeWithoutFollowingLinks $path)
        foreach ($item in $items) {
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                Write-Host "KEEP (link, not traversed): $($item.FullName)"
                continue
            }
            if (-not $item.PSIsContainer -and ($item.Extension -in @('.exe', '.pdb', '.zip') -or
                $item.Name -in @('BUILD.json', 'SHA256SUMS', 'LICENSE', 'THIRD_PARTY_NOTICES.md',
                    'XenTerm-Next.txt', 'HeroGPUI-APACHE-2.0', 'HeroGPUI-NOTICE'))) {
                Write-Host "REMOVE: $($item.FullName)"
                if ($Apply) { Remove-Item -LiteralPath $item.FullName -Force }
                $removedBytes += $item.Length
            }
        }
        if ($Apply) {
            $directories = @($items | Where-Object { $_.PSIsContainer -and -not ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) } |
                Sort-Object { $_.FullName.Length } -Descending)
            foreach ($dir in @($directories | ForEach-Object FullName) + @($path)) {
                if (Test-Path -LiteralPath $dir) {
                    if (@(Get-ChildItem -LiteralPath $dir -Force).Count -eq 0) { Remove-Item -LiteralPath $dir -Force }
                }
            }
        }
    }
    # Generated caches only. In particular, never recurse into the registered
    # builds\wsl-ubuntu22\distro\ext4.vhdx or maintenance scripts/backups.
    foreach ($relative in @('source\target', 'maintenance\rio-vt-eval\target', 'builds\linux\artifacts')) {
        $path = Assert-LocalChildPath $WorkspaceRoot (Join-Path $WorkspaceRoot $relative)
        if (-not (Test-Path -LiteralPath $path)) { continue }
        $busy = @($processes | Where-Object {
            ($_.ExecutablePath -and $_.ExecutablePath.StartsWith($path + '\', [StringComparison]::OrdinalIgnoreCase)) -or
            ($_.Name -match '^(cargo|rustc|link|cl|cmake|ninja)\.exe$' -and $_.CommandLine -and
                ($_.CommandLine.Contains($path) -or $_.CommandLine.Contains((Join-Path $WorkspaceRoot 'source'))))
        })
        if ($busy.Count) { Write-Host "KEEP (in use by a build/process): $path"; continue }
        $items = @(Get-LocalTreeWithoutFollowingLinks $path)
        $disposable = @(Get-DisposableCacheItems $path $items)
        $bytes = [long]0
        foreach ($item in $disposable) {
            # Items were enumerated without following links under a checked root.
            # Check each final absolute deletion target without re-statting every ancestor.
            $full = [IO.Path]::GetFullPath($item.FullName)
            if (-not $full.StartsWith($WorkspaceRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
                throw "Cache deletion target escaped the workspace: $full"
            }
            if ($item.PSIsContainer) {
                $measure = Get-LocalTreeWithoutFollowingLinks $item.FullName |
                    Where-Object { -not $_.PSIsContainer } | Measure-Object Length -Sum
                if ($measure -and $measure.PSObject.Properties['Sum']) { $bytes += [long]$measure.Sum }
            } else { $bytes += $item.Length }
        }
        Write-Host ('REMOVE CACHE: {0} ({1:N2} GiB)' -f $path, ($bytes / 1GB))
        if ($Apply -and $disposable.Count) { Remove-Item -LiteralPath @($disposable | ForEach-Object FullName) -Recurse -Force }
        $removedBytes += $bytes
    }
    $mode = if ($Apply) { 'Freed' } else { 'Would free (preview only)' }
    Write-Host ('{0}: {1:N2} GiB' -f $mode, ($removedBytes / 1GB))
} finally {
    if ($runtimeLock) { $runtimeLock.Dispose() }
    $buildLock.Dispose()
}
