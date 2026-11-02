<#
.SYNOPSIS
    Downloads and unpacks the pinned WiX Toolset 3.14.1 Windows binaries.

.DESCRIPTION
    The Windows MSI packaging
    (apps/xenterm/tool/package_windows_installer.ps1) needs candle.exe,
    light.exe and heat.exe from WiX 3.14. WiX 3 is self-contained native
    .NET Framework binaries with no SDK/global-tool/NuGet-feed dependency, so
    it is the most boring toolchain choice on windows-latest.

    The official wix314-binaries.zip release asset is pinned by version *and*
    SHA256: the toolchain is byte-for-byte the same locally and in CI, and a
    changed/compromised download fails the build instead of being compiled in.

    stdout is the resolved WiX bin directory (one line); logs go to Write-Host.

.PARAMETER InstallDir
    Directory to unpack WiX into. Defaults to $env:RUNNER_TEMP\wix-3.14.1 on
    CI, else $env:TEMP\wix-3.14.1.

.PARAMETER WixBinDir
    Use an already provisioned WiX bin directory instead of downloading.
    Validated and printed; nothing is downloaded.

.PARAMETER Force
    Re-download and re-extract even when InstallDir already looks complete.

.EXAMPLE
    pwsh scripts/install-wix.ps1
    pwsh apps/xenterm/tool/package_windows_installer.ps1 -Version v0.7.3
#>
[CmdletBinding()]
param(
    [string] $InstallDir = "",
    [string] $WixBinDir = "",
    [switch] $Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$WixVersion = "3.14.1"
$WixUrl = "https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip"
# sha256 of wix3141rtm/wix314-binaries.zip; candle.exe -? reports 3.14.1.8722.
$WixSha256 = "6AC824E1642D6F7277D0ED7EA09411A508F6116BA6FAE0AA5F2C7DAA2FF43D31"
$RequiredTools = @("candle.exe", "light.exe", "heat.exe")
$RequiredExtensions = @("WixUIExtension.dll", "WixUtilExtension.dll")

function Write-Log {
    param([string] $Message)
    Write-Host "[install-wix] $Message"
}

function Test-WixBinDir {
    param([string] $Directory)
    if ([string]::IsNullOrWhiteSpace($Directory) -or -not (Test-Path -LiteralPath $Directory -PathType Container)) {
        return $false
    }
    foreach ($file in ($RequiredTools + $RequiredExtensions)) {
        if (-not (Test-Path -LiteralPath (Join-Path $Directory $file) -PathType Leaf)) {
            return $false
        }
    }
    return $true
}

if (-not [string]::IsNullOrWhiteSpace($WixBinDir)) {
    if (-not (Test-WixBinDir -Directory $WixBinDir)) {
        throw "WixBinDir '$WixBinDir' does not look like a WiX 3 bin directory (needs candle.exe/light.exe/heat.exe and the UI/Util extensions)"
    }
    Write-Log "using pre-provisioned WiX at $WixBinDir"
    Write-Output (Resolve-Path -LiteralPath $WixBinDir).Path
    return
}

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $base = $env:RUNNER_TEMP
    if ([string]::IsNullOrWhiteSpace($base)) { $base = $env:TEMP }
    if ([string]::IsNullOrWhiteSpace($base)) { $base = [System.IO.Path]::GetTempPath() }
    $InstallDir = Join-Path $base "wix-$WixVersion"
}
$InstallDir = [System.IO.Path]::GetFullPath($InstallDir)

if (-not $Force -and (Test-WixBinDir -Directory $InstallDir)) {
    Write-Log "reusing existing WiX $WixVersion at $InstallDir"
    Write-Output $InstallDir
    return
}

$ProgressPreference = "SilentlyContinue"
try {
    [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12
} catch {
    # Older/newer runtimes may already have a correct default; ignore.
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("xenterm-wix-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $work -Force | Out-Null
$zip = Join-Path $work "wix314-binaries.zip"

try {
    $downloaded = $false
    for ($attempt = 1; $attempt -le 3 -and -not $downloaded; $attempt++) {
        try {
            Write-Log "downloading WiX $WixVersion (attempt $attempt): $WixUrl"
            Invoke-WebRequest -Uri $WixUrl -OutFile $zip -UseBasicParsing
            $downloaded = $true
        } catch {
            if ($attempt -eq 3) {
                throw "failed to download WiX $WixVersion from $WixUrl`: $($_.Exception.Message)"
            }
            Start-Sleep -Seconds (2 * $attempt)
        }
    }

    $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
    if ($actual -ne $WixSha256) {
        throw "WiX zip checksum mismatch: expected $WixSha256, got $actual"
    }
    Write-Log "checksum verified"

    if (Test-Path -LiteralPath $InstallDir) {
        Remove-Item -LiteralPath $InstallDir -Recurse -Force
    }
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    # No Expand-Archive here: PS 5.1's Expand-Archive is slow on 40 MB and
    # rejects some zip flavours, while tar.exe ships with Windows 10+.
    $tar = Join-Path $env:SystemRoot "System32\tar.exe"
    if (Test-Path -LiteralPath $tar) {
        & $tar -xf $zip -C $InstallDir
        if ($LASTEXITCODE -ne 0) { throw "tar.exe failed to extract $zip (exit code $LASTEXITCODE)" }
    } else {
        Expand-Archive -LiteralPath $zip -DestinationPath $InstallDir -Force
    }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not (Test-WixBinDir -Directory $InstallDir)) {
    throw "WiX extraction did not produce the expected files in $InstallDir"
}

Write-Log "installed WiX $WixVersion into $InstallDir"
Write-Output $InstallDir
