# Shared Windows portable runtime operations. No real profile is opened here.
Set-StrictMode -Version Latest

function Assert-LocalChildPath {
    param([string]$Root, [string]$Path)
    $rootPath = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    $full = [IO.Path]::GetFullPath($Path)
    if (-not $full.StartsWith($rootPath + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Path is outside workspace: $full"
    }
    $probe = $full
    while ($probe.Length -gt $rootPath.Length) {
        if (Test-Path -LiteralPath $probe) {
            $item = Get-Item -LiteralPath $probe -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Refusing a junction or symlink: $probe"
            }
        }
        $probe = Split-Path -Parent $probe
    }
    return $full
}

function Enter-LocalRuntimeLock {
    param([string]$Root, [switch]$Build)
    $runtime = Assert-LocalChildPath $Root (Join-Path $Root 'runtime')
    [IO.Directory]::CreateDirectory($runtime) | Out-Null
    try {
        $name = if ($Build) { '.local-build.lock' } else { '.local-update.lock' }
        return [IO.File]::Open((Join-Path $runtime $name),
            [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    } catch { throw 'Another local update or launch is in progress. Try again after it finishes.' }
}

function Get-LocalTreeWithoutFollowingLinks {
    param([string]$Directory)
    $stack = New-Object 'Collections.Generic.Stack[string]'
    $stack.Push($Directory)
    while ($stack.Count) {
        foreach ($item in Get-ChildItem -LiteralPath $stack.Pop() -Force) {
            $item
            if ($item.PSIsContainer -and -not ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                $stack.Push($item.FullName)
            }
        }
    }
}

function Get-LocalVersion {
    param([string]$Exe)
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $Exe
    $info.Arguments = '--version'
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $info
    try {
        if (-not $process.Start()) { throw 'Could not start version check.' }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(30000)) {
            $process.Kill()
            $process.WaitForExit()
            throw 'Version check timed out.'
        }
        $output = $stdout.GetAwaiter().GetResult().Trim()
        $errorText = $stderr.GetAwaiter().GetResult().Trim()
        if ($process.ExitCode -ne 0 -or $output -notmatch '^xenterm xt-') {
            throw "Version check failed: $output $errorText"
        }
        return $output
    } finally { $process.Dispose() }
}

function Test-LocalPackage {
    param([string]$Directory)
    $exe = Join-Path $Directory 'xenterm.exe'
    $manifest = Get-Content -LiteralPath (Join-Path $Directory 'BUILD.json') -Raw | ConvertFrom-Json
    if ($manifest.repository -ne 'https://github.com/xue-ding-e/XenTerm-Next.git' -or
        $manifest.commit -notmatch '^[0-9a-f]{40}$' -or
        (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash -ne $manifest.sha256) {
        throw "Invalid local package: $Directory"
    }
    $version = Get-LocalVersion $exe
    if ($version -ne $manifest.runtime_version -or
        $version -notmatch [regex]::Escape($manifest.commit.Substring(0, 7))) {
        throw "Package version does not match its source commit: $Directory"
    }
    return $manifest
}

function Remove-LocalPackage {
    param([string]$Root, [string]$Directory)
    $full = Assert-LocalChildPath $Root $Directory
    $allowed = @('previous', 'pending', 'pending.tmp') | ForEach-Object {
        Join-Path (Join-Path $Root 'runtime') $_
    }
    if ($full -notin $allowed) { throw "Not a disposable runtime package: $full" }
    if (-not (Test-Path -LiteralPath $full)) { return }
    $items = @(Get-LocalTreeWithoutFollowingLinks $full)
    foreach ($item in $items) {
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
            $item.Name -match '^(sessions\.(db.*|json.*)|secret\.key|known_hosts|\.git)$' -or
            $item.Extension -eq '.vhdx') {
            throw "Package contains protected data or a link: $($item.FullName)"
        }
    }
    $running = @(Get-CimInstance Win32_Process -Filter "Name = 'xenterm.exe'" |
        Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($full + '\', [StringComparison]::OrdinalIgnoreCase) })
    if ($running.Count) { throw "Package is still in use: $full" }
    Remove-Item -LiteralPath $full -Recurse -Force
}

function Publish-LocalPending {
    param([string]$Root)
    $runtime = Join-Path $Root 'runtime'
    $pending = Assert-LocalChildPath $Root (Join-Path $runtime 'pending')
    if (-not (Test-Path -LiteralPath $pending)) { return $true }
    $null = Test-LocalPackage $pending
    $current = Assert-LocalChildPath $Root (Join-Path $runtime 'current')
    $previous = Assert-LocalChildPath $Root (Join-Path $runtime 'previous')
    $exe = Join-Path $current 'xenterm.exe'
    $running = @(Get-CimInstance Win32_Process -Filter "Name = 'xenterm.exe'" |
        Where-Object { $_.ExecutablePath -eq $exe })
    if ($running.Count) {
        Write-Host 'Build is ready in runtime\pending. Close XenTerm and its MCP clients, then launch again to apply it.'
        return $false
    }
    # Keep exactly one rollback package. Never replace a running executable.
    Remove-LocalPackage $Root $previous
    $movedCurrent = $false
    try {
        if (Test-Path -LiteralPath $current) {
            Move-Item -LiteralPath $current -Destination $previous
            $movedCurrent = $true
        }
        Move-Item -LiteralPath $pending -Destination $current
    } catch {
        if ($movedCurrent -and -not (Test-Path -LiteralPath $current)) {
            Move-Item -LiteralPath $previous -Destination $current
        }
        throw
    }
    Write-Host "Active runtime: $exe"
    return $true
}

function Set-LocalShortcuts {
    param([string]$Root, [string]$ScriptsDirectory)
    $shell = New-Object -ComObject WScript.Shell
    $powershell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $icon = Join-Path $Root 'runtime\current\xenterm.exe'
    $launch = Join-Path $ScriptsDirectory 'start-local.ps1'
    $update = Join-Path $ScriptsDirectory 'update-local.ps1'
    $desktop = [Environment]::GetFolderPath('Desktop')
    $programs = [Environment]::GetFolderPath('Programs')
    foreach ($folder in @($desktop, $programs)) {
        $link = $shell.CreateShortcut((Join-Path $folder 'XenTerm Next.lnk'))
        $link.TargetPath = $powershell
        $link.Arguments = '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "{0}" -WorkspaceRoot "{1}"' -f $launch, $Root
        $link.WorkingDirectory = $Root
        $link.IconLocation = "$icon,0"
        $link.Description = 'Launch XenTerm Next from the fixed local runtime.'
        $link.Save()
    }
    $link = $shell.CreateShortcut((Join-Path $desktop 'XenTerm Next - Update and Build.lnk'))
    $link.TargetPath = $powershell
    $link.Arguments = '-NoProfile -ExecutionPolicy Bypass -File "{0}" -WorkspaceRoot "{1}" -Pause' -f $update, $Root
    $link.WorkingDirectory = $Root
    $link.IconLocation = "$icon,0"
    $link.Description = 'Pull main, build locally, and update the fixed runtime.'
    $link.Save()
}
