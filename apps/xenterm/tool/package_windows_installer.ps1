<#
.SYNOPSIS
    Builds the XenTerm Windows MSI installer from a Flutter desktop release bundle.

.DESCRIPTION
    Stages apps/xenterm/build/windows/x64/runner/Release (the Flutter release
    bundle with xenterm_bridge.dll already copied in by bundle_native.sh),
    harvests it with heat.exe, and links a per-machine MSI with candle/light
    from WiX Toolset 3.14.

    Installer behaviour (see packaging/windows/main.wxs):
      * installs to %ProgramFiles%\XenTerm (per-machine, elevated)
      * Start Menu shortcut, Add/Remove Programs entry with icon
      * major upgrade over any older XenTerm MSI (fixed UpgradeCode)
      * silent install / uninstall: msiexec /i <msi> /qn  and  msiexec /x <msi> /qn
      * silent custom directory: msiexec /i <msi> /qn INSTALLFOLDER="D:\XenTerm"

    The portable zip is a separate artifact and keeps working: it lives in a
    user-chosen folder and keeps its profile next to the executable, while this
    MSI installs read-only program files and the core falls back to %APPDATA%.

    WiX is not vendored. Locally/CI install the pinned toolchain first with
    `pwsh scripts/install-wix.ps1` (or pass -WixBin). This script never
    downloads anything.

.PARAMETER Version
    Artifact version used in the MSI file name and the ARP/provenance string,
    e.g. v0.7.3 (stable tag), beta-56554d7, or a dry-run ref name.

.PARAMETER BundleDir
    Flutter release bundle directory. Defaults to
    apps/xenterm/build/windows/x64/runner/Release.

.PARAMETER ProductVersion
    MSI ProductVersion (three numeric fields). Defaults to the numeric part of
    -Version when it looks like vX.Y.Z, otherwise to crates/xenterm/Cargo.toml's
    [package] version. The full -Version is kept in the installer description.

.PARAMETER OutputDir
    Directory the final xenterm-<version>-windows-x86_64.msi is written to.
    Defaults to the repository root (same as package_desktop.sh).

.PARAMETER WixBin
    Directory containing candle.exe/light.exe/heat.exe.

.PARAMETER DryRun
    Validate inputs and print the planned toolchain commands without running
    WiX. Useful for script checks on machines without WiX.

.PARAMETER KeepWorkDir
    Keep target/wix/installer (staged files and WiX intermediates) for debugging.

.EXAMPLE
    pwsh scripts/install-wix.ps1            # once, installs pinned WiX 3.14.1
    pwsh apps/xenterm/tool/package_windows_installer.ps1 -Version v0.7.3
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string] $Version,

    [string] $BundleDir = "",
    [string] $OutputDir = "",
    [string] $ProductVersion = "",
    [string] $WixBin = "",
    [switch] $DryRun,
    [switch] $KeepWorkDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-InputPath {
    param([string] $Path, [string] $BaseDir)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $BaseDir $Path))
}

function Get-CrateProductVersion {
    param([string] $RepoRoot)
    $cargoToml = Join-Path $RepoRoot "crates\xenterm\Cargo.toml"
    if (-not (Test-Path -LiteralPath $cargoToml)) {
        throw "cannot read the product version: $cargoToml does not exist"
    }
    $match = Select-String -LiteralPath $cargoToml -Pattern '^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"' |
        Select-Object -First 1
    if (-not $match) {
        throw "cannot read [package] version from $cargoToml"
    }
    return $match.Matches[0].Groups[1].Value
}

function Assert-MsiProductVersion {
    param([string] $Value)
    if ($Value -notmatch '^([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,5})$') {
        throw "MSI ProductVersion '$Value' must be three numeric fields (X.Y.Z, X/Y <= 255, Z <= 65535)"
    }
    # Windows Installer limits: major/minor are 8-bit, build is 16-bit.
    $major = [int]$Matches[1]
    $minor = [int]$Matches[2]
    $build = [int]$Matches[3]
    if ($major -gt 255 -or $minor -gt 255 -or $build -gt 65535) {
        throw "MSI ProductVersion '$Value' exceeds the Windows Installer field limits (major/minor <= 255, build <= 65535)"
    }
}

function Find-WixBin {
    param([string] $Explicit)
    $candidates = New-Object System.Collections.Generic.List[string]
    if (-not [string]::IsNullOrWhiteSpace($Explicit)) {
        $candidates.Add($Explicit)
    }
    if (-not [string]::IsNullOrEmpty($env:WIX)) {
        $candidates.Add((Join-Path $env:WIX "bin"))
    }
    $onPath = Get-Command "candle.exe" -ErrorAction SilentlyContinue
    if ($onPath) {
        $candidates.Add((Split-Path -Parent $onPath.Source))
    }
    $programFilesX86 = [Environment]::GetFolderPath("ProgramFilesX86")
    if ($programFilesX86) {
        $candidates.Add((Join-Path $programFilesX86 "WiX Toolset v3.14\bin"))
        $candidates.Add((Join-Path $programFilesX86 "WiX Toolset v3.11\bin"))
    }
    foreach ($candidate in $candidates) {
        if ([string]::IsNullOrWhiteSpace($candidate)) { continue }
        $candle = Join-Path $candidate "candle.exe"
        if (Test-Path -LiteralPath $candle) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    return $null
}

function Copy-ReleaseBundle {
    param([string] $SourceDir, [string] $DestinationDir)
    # robocopy exit codes 0-7 are success (files copied / extra files present).
    # Exclude a local launcher profile (may contain real sessions) and symbols;
    # xenterm_app.exe is a stale pre-CMake-rename artifact, the canonical
    # executable is xenterm.exe (apps/xenterm/windows/CMakeLists.txt).
    $robocopyArgs = @(
        $SourceDir, $DestinationDir,
        "/E",
        "/XD", "config",
        "/XF", "*.pdb", "xenterm_app.exe",
        "/NFL", "/NDL", "/NJH", "/NJS", "/NP",
        "/R:1", "/W:1"
    )
    & robocopy @robocopyArgs | Out-Null
    $code = $LASTEXITCODE
    if ($code -ge 8) {
        throw "robocopy failed with exit code $code"
    }
    $global:LASTEXITCODE = 0
}

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\..\..")).Path

if ($Version -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$') {
    throw "Version '$Version' must be a single path-safe token (letters, digits, '.', '_', '-'): it becomes the artifact file name"
}

if ([string]::IsNullOrWhiteSpace($BundleDir)) {
    $BundleDir = Join-Path $repoRoot "apps\xenterm\build\windows\x64\runner\Release"
} else {
    $BundleDir = Resolve-InputPath -Path $BundleDir -BaseDir $repoRoot
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = $repoRoot
} else {
    $OutputDir = Resolve-InputPath -Path $OutputDir -BaseDir $repoRoot
}

if ([string]::IsNullOrWhiteSpace($ProductVersion)) {
    if ($Version -match '^v?([0-9]+\.[0-9]+\.[0-9]+)$') {
        $ProductVersion = $Matches[1]
    } else {
        $ProductVersion = Get-CrateProductVersion -RepoRoot $repoRoot
    }
}
Assert-MsiProductVersion -Value $ProductVersion

$iconRel = "assets\xenterm.ico"
$licenseRel = "packaging\windows\LICENSE.rtf"
$mainWxsRel = "packaging\windows\main.wxs"
$workRel = "target\wix\installer"
$stageRel = "$workRel\stage"
$msiName = "xenterm-$Version-windows-x86_64.msi"
$finalMsi = Join-Path $OutputDir $msiName

foreach ($rel in @($mainWxsRel, $licenseRel, $iconRel)) {
    if (-not (Test-Path -LiteralPath (Join-Path $repoRoot $rel))) {
        throw "missing installer source file: $rel"
    }
}
if (-not (Test-Path -LiteralPath $BundleDir)) {
    throw "Flutter release bundle not found: $BundleDir`nRun 'flutter build windows --release' first (see docs/release.md)"
}

# The bundle must contain the executable, the FFI bridge and Flutter's data dir;
# anything less would produce an MSI that installs a broken application.
$requiredFiles = @("xenterm.exe", "xenterm_bridge.dll")
$requiredDirs = @("data\flutter_assets")
foreach ($name in $requiredFiles) {
    $path = Join-Path $BundleDir $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "the Flutter release bundle is incomplete: missing $name in $BundleDir"
    }
    if ((Get-Item -LiteralPath $path).Length -le 0) {
        throw "the Flutter release bundle is incomplete: $name is empty"
    }
}
foreach ($name in $requiredDirs) {
    if (-not (Test-Path -LiteralPath (Join-Path $BundleDir $name) -PathType Container)) {
        throw "the Flutter release bundle is incomplete: missing directory $name in $BundleDir"
    }
}

$wixBinResolved = Find-WixBin -Explicit $WixBin
if (-not $DryRun) {
    if (-not $wixBinResolved) {
        throw "WiX Toolset 3.14 not found (candle.exe/light.exe/heat.exe).`nRun 'pwsh scripts/install-wix.ps1' first, or pass -WixBin <dir>."
    }
    foreach ($tool in @("candle.exe", "light.exe", "heat.exe")) {
        if (-not (Test-Path -LiteralPath (Join-Path $wixBinResolved $tool))) {
            throw "WiX Toolset at $wixBinResolved is missing $tool"
        }
    }
}

if ($DryRun) {
    Write-Host "Dry run: no files written, no toolchain invoked."
    Write-Host "  repository       : $repoRoot"
    Write-Host "  bundle           : $BundleDir"
    Write-Host "  version          : $Version"
    Write-Host "  product version  : $ProductVersion"
    Write-Host "  output           : $finalMsi"
    if ($wixBinResolved) {
        Write-Host "  WiX bin          : $wixBinResolved"
    } else {
        Write-Host "  WiX bin          : <not found; required for a real build>"
    }
    Write-Host "  staging dir      : $(Join-Path $repoRoot $stageRel)"
    Write-Host "Planned commands (working directory: $repoRoot):"
    Write-Host "  heat.exe dir $stageRel -nologo -cg XenTermFiles -dr INSTALLFOLDER -ag -sfrag -srd -sreg -scom -var var.SourceDir -out $workRel\Files.wxs"
    Write-Host "  candle.exe -nologo -arch x64 -ext WixUtilExtension -dSourceDir=$stageRel -dProductVersion=$ProductVersion -dAppVersion=$Version -dIconFile=$iconRel -dLicenseRtf=$licenseRel -out $workRel\main.wixobj $mainWxsRel"
    Write-Host "  candle.exe -nologo -arch x64 -dSourceDir=$stageRel -out $workRel\Files.wixobj $workRel\Files.wxs"
    Write-Host "  light.exe -nologo -ext WixUIExtension -ext WixUtilExtension -cultures:en-us -sice:ICE60 -out $workRel\$msiName $workRel\main.wixobj $workRel\Files.wixobj"
    Write-Host "  Copy-Item $workRel\$msiName -> $finalMsi"
    Write-Output $finalMsi
    return
}

$oldLocation = Get-Location
$createdWorkDir = $false
try {
    Set-Location -LiteralPath $repoRoot

    if (Test-Path -LiteralPath $workRel) {
        Remove-Item -LiteralPath $workRel -Recurse -Force
    }
    New-Item -ItemType Directory -Path $stageRel -Force | Out-Null
    $createdWorkDir = $true

    Write-Host "Staging the Flutter release bundle..."
    Copy-ReleaseBundle -SourceDir $BundleDir -DestinationDir $stageRel

    # Ship build provenance and the same documents as the portable archives.
    foreach ($extra in @("README.md", "README.zh.md", "CHANGELOG.md", "THIRD_PARTY_NOTICES.md", "version.json")) {
        $source = Join-Path $repoRoot $extra
        if (Test-Path -LiteralPath $source -PathType Leaf) {
            Copy-Item -LiteralPath $source -Destination $stageRel -Force
        } elseif ($extra -eq "version.json") {
            Write-Warning "version.json not found; the MSI will not carry build provenance (run a Rust build first)"
        }
    }

    $candle = Join-Path $wixBinResolved "candle.exe"
    $light = Join-Path $wixBinResolved "light.exe"
    $heat = Join-Path $wixBinResolved "heat.exe"

    Write-Host "Harvesting the bundle (heat.exe)..."
    # Tool output goes to the host stream: this script's *pipeline* output is the
    # MSI path only, so callers can safely capture it in a variable.
    & $heat dir $stageRel -nologo -cg XenTermFiles -dr INSTALLFOLDER -ag -sfrag -srd -sreg -scom `
        -var var.SourceDir -out "$workRel\Files.wxs" 2>&1 | Write-Host
    if ($LASTEXITCODE -ne 0) { throw "heat.exe failed with exit code $LASTEXITCODE" }

    Write-Host "Compiling the installer (candle.exe)..."
    # -ext WixUtilExtension is required at compile time for the
    # util:CloseApplication element in main.wxs.
    & $candle -nologo -arch x64 -ext WixUtilExtension `
        "-dSourceDir=$stageRel" "-dProductVersion=$ProductVersion" "-dAppVersion=$Version" `
        "-dIconFile=$iconRel" "-dLicenseRtf=$licenseRel" `
        -out "$workRel\main.wixobj" $mainWxsRel 2>&1 | Write-Host
    if ($LASTEXITCODE -ne 0) { throw "candle.exe failed with exit code $LASTEXITCODE" }

    & $candle -nologo -arch x64 "-dSourceDir=$stageRel" -out "$workRel\Files.wixobj" "$workRel\Files.wxs" 2>&1 | Write-Host
    if ($LASTEXITCODE -ne 0) { throw "candle.exe failed with exit code $LASTEXITCODE" }

    Write-Host "Linking the MSI (light.exe)..."
    # ICE60 fires for third-party Flutter/plugin DLLs whose version resource
    # has no language column; harmless for a private installer, so suppressed
    # to keep CI logs readable. ICE61 is intentionally left visible: it checks
    # the AllowSameVersionUpgrades window in main.wxs.
    & $light -nologo -ext WixUIExtension -ext WixUtilExtension -cultures:en-us -sice:ICE60 `
        -out "$workRel\$msiName" "$workRel\main.wixobj" "$workRel\Files.wixobj" 2>&1 | Write-Host
    if ($LASTEXITCODE -ne 0) { throw "light.exe failed with exit code $LASTEXITCODE" }

    $builtMsi = Join-Path $repoRoot (Join-Path $workRel $msiName)
    if (-not (Test-Path -LiteralPath $builtMsi -PathType Leaf)) {
        throw "light.exe reported success but $builtMsi does not exist"
    }
    if ((Get-Item -LiteralPath $builtMsi).Length -le 0) {
        throw "the generated MSI is empty: $builtMsi"
    }

    if (-not (Test-Path -LiteralPath $OutputDir -PathType Container)) {
        New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
    }
    Copy-Item -LiteralPath $builtMsi -Destination $finalMsi -Force
    Write-Host "Installer written: $finalMsi"
    Write-Output $finalMsi
} finally {
    Set-Location -LiteralPath $oldLocation
    if ($createdWorkDir -and -not $KeepWorkDir) {
        $workFull = Join-Path $repoRoot $workRel
        if (Test-Path -LiteralPath $workFull) {
            Remove-Item -LiteralPath $workFull -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
