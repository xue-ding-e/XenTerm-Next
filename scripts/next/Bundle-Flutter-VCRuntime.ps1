# Bundle the installed Visual Studio x64 CRT, without download, install or elevation.
[CmdletBinding()]
param(
    [Alias('BundleDir', 'BundlePath')][Parameter(Mandatory = $true)][string]$Bundle,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$SourceSha,
    [Alias('VCToolsRedistDir')][string]$RedistDir = $env:VCToolsRedistDir
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-VcPlainPath([string]$Path) {
    $candidate = [IO.Path]::GetFullPath($Path)
    while ($candidate) {
        if (Test-Path -LiteralPath $candidate) {
            $item = Get-Item -LiteralPath $candidate -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Path links are not permitted: $candidate"
            }
        }
        $parent = [IO.Directory]::GetParent($candidate)
        if ($null -eq $parent) { break }
        $candidate = $parent.FullName
    }
}

function Test-VcExcluded([string]$Relative) {
    return ($Relative -match '(?i)(^|[\\/])(config|config-flutter-next|log)([\\/]|$)' -or
        $Relative -match '(?i)\.pdb$' -or
        $Relative -match '(?i)(^|[\\/])xenterm_app\.exe$')
}

function Get-VcBundleFiles([string]$Directory, [string]$Root) {
    # Do not use -Recurse: inspect each directory before entering it.
    foreach ($item in Get-ChildItem -LiteralPath $Directory -Force) {
        Assert-VcPlainPath $item.FullName
        $relative = $item.FullName.Substring($Root.Length + 1).Replace('\', '/')
        if (Test-VcExcluded $relative) { continue }
        if ($item.PSIsContainer) {
            Get-VcBundleFiles $item.FullName $Root
        } else {
            $item
        }
    }
}

function Assert-VcMicrosoftDll([string]$Path) {
    Assert-VcPlainPath $Path
    $stream = [IO.File]::OpenRead($Path)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5A4D) {
            throw "Not a PE DLL: $Path"
        }
        $stream.Position = 0x3C
        $offset = $reader.ReadUInt32()
        if ($offset -gt $stream.Length - 24) { throw "Invalid PE header: $Path" }
        $stream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {
            throw "MSVC CRT must be an x64 PE: $Path"
        }
        $stream.Position = $offset + 22
        if (-not ($reader.ReadUInt16() -band 0x2000)) { throw "Not a PE DLL: $Path" }
    } finally {
        $reader.Dispose()
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or $null -eq $signature.SignerCertificate -or
        $signature.SignerCertificate.Subject -notmatch '(^|,\s*)O=Microsoft Corporation(,|$)') {
        throw "CRT must have a valid Microsoft Authenticode signature: $Path"
    }
}

if ($env:OS -ne 'Windows_NT') { throw 'MSVC CRT bundling requires Windows.' }
if ($SourceSha -cnotmatch '^[0-9a-f]{40}$') { throw 'A full lowercase source commit SHA is required.' }
$versionMatch = [regex]::Match($Version, '^xt-(?:v[0-9]+\.[0-9]+\.[0-9]+|(?:beta|alpha|dev)-([0-9a-f]{7})-T[0-9]{10})$')
if (-not $versionMatch.Success -or
    ($versionMatch.Groups[1].Success -and $versionMatch.Groups[1].Value -cne $SourceSha.Substring(0, 7))) {
    throw 'Runtime source/version mismatch; use the supplied XT_VERSION.'
}
Assert-VcPlainPath $Bundle
$bundlePath = (Get-Item -LiteralPath $Bundle -Force).FullName.TrimEnd('\', '/')
if (-not (Test-Path -LiteralPath $bundlePath -PathType Container)) { throw 'Missing Flutter bundle.' }
$receiptPath = Join-Path $bundlePath 'WINDOWS-RUNTIME.json'
if (Test-Path -LiteralPath $receiptPath) { throw "Runtime receipt already exists: $receiptPath" }
$bundleFiles = @(Get-VcBundleFiles $bundlePath $bundlePath)
foreach ($name in @('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll', 'flutter_windows.dll',
        'data/icudtl.dat', 'data/app.so', 'data/flutter_assets/NOTICES.Z')) {
    $file = Get-Item -LiteralPath (Join-Path $bundlePath $name) -Force
    if ($file.PSIsContainer -or $file.Length -eq 0) { throw "Missing or empty runtime file: $name" }
}
$guiVersion = (Get-Item -LiteralPath (Join-Path $bundlePath 'xenterm.exe')).VersionInfo
if ($guiVersion.FileVersion -cne $Version -or $guiVersion.ProductVersion -cne $Version) {
    throw 'GUI resource version differs from the supplied XT_VERSION.'
}

if (-not $RedistDir) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    Assert-VcPlainPath $vswhere
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
        throw 'Set VCToolsRedistDir or use an installed Visual Studio with vswhere.'
    }
    $installations = @(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
    if ($LASTEXITCODE -ne 0 -or $installations.Count -ne 1 -or -not $installations[0]) {
        throw 'vswhere did not return one installed Visual Studio C++ toolchain.'
    }
    $redistRoot = Join-Path $installations[0] 'VC/Redist/MSVC'
    Assert-VcPlainPath $redistRoot
    $candidates = @(Get-ChildItem -LiteralPath $redistRoot -Directory | Where-Object {
        $_.Name -match '^[0-9]+\.[0-9]+\.[0-9]+(?:\.[0-9]+)?$'
    } | Sort-Object { [version]$_.Name } -Descending)
    foreach ($candidate in $candidates) {
        Assert-VcPlainPath $candidate.FullName
        if (Test-Path -LiteralPath (Join-Path $candidate.FullName 'x64/Microsoft.VC143.CRT') -PathType Container) {
            $RedistDir = $candidate.FullName
            break
        }
    }
    if (-not $RedistDir) { throw 'No installed Microsoft.VC143.CRT x64 redist was found.' }
}
Assert-VcPlainPath $RedistDir
$crtDirectory = Join-Path $RedistDir 'x64/Microsoft.VC143.CRT'
Assert-VcPlainPath $crtDirectory
$crtDirectory = (Get-Item -LiteralPath $crtDirectory -Force).FullName
$crtFiles = @(Get-ChildItem -LiteralPath $crtDirectory -Force | Sort-Object Name)
foreach ($item in $crtFiles) { Assert-VcPlainPath $item.FullName }
$crtFiles = @($crtFiles | Where-Object { -not $_.PSIsContainer -and $_.Extension -ieq '.dll' })
foreach ($name in @('msvcp140.dll', 'vcruntime140.dll')) {
    if ($name -cnotin $crtFiles.Name) { throw "Missing core Microsoft.VC143.CRT DLL: $name" }
}
$crtHashes = @{}
foreach ($file in $crtFiles) {
    $destination = Join-Path $bundlePath $file.Name
    if (Test-Path -LiteralPath $destination) { throw "Refusing to overwrite CRT: $destination" }
    Assert-VcMicrosoftDll $file.FullName
    $crtHashes[$file.Name] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
}
$vcVersion = (Get-Item -LiteralPath (Join-Path $crtDirectory 'msvcp140.dll')).VersionInfo
$minimumVersion = '{0}.{1}.{2}.{3}' -f $vcVersion.FileMajorPart, $vcVersion.FileMinorPart, $vcVersion.FileBuildPart, $vcVersion.FilePrivatePart
if ($vcVersion.FileMajorPart -lt 14 -or $minimumVersion -notmatch '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$') {
    throw 'Cannot read the Microsoft CRT file version.'
}

# Validate every source and destination before copying any DLL. File.Copy(false)
# also refuses a destination created concurrently after this preflight check.
foreach ($file in $crtFiles) {
    $destination = Join-Path $bundlePath $file.Name
    [IO.File]::Copy($file.FullName, $destination, $false)
    if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant() -cne $crtHashes[$file.Name]) {
        throw "CRT changed while being copied: $($file.Name)"
    }
}
$hashes = [ordered]@{}
foreach ($file in @(Get-VcBundleFiles $bundlePath $bundlePath) | Sort-Object FullName) {
    if ($file.Extension.ToLowerInvariant() -in @('.exe', '.dll', '.so')) {
        $relative = $file.FullName.Substring($bundlePath.Length + 1).Replace('\', '/')
        $hashes[$relative] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}
$receipt = [ordered]@{
    schema_version = 1
    source = $crtDirectory
    source_sha = $SourceSha
    core_version = $Version
    minimum_vc_runtime = $minimumVersion
    windows = 'Windows 10/11 x64'
    runtime_mode = 'app-local'
    crt_files = @($crtFiles.Name)
    binary_sha256 = $hashes
    clean_windows_tested = $false
}
$stream = [IO.File]::Open($receiptPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
$writer = [IO.StreamWriter]::new($stream, [Text.UTF8Encoding]::new($false))
try {
    $writer.NewLine = "`n"
    $writer.WriteLine(($receipt | ConvertTo-Json -Depth 5).Replace("`r`n", "`n"))
} finally {
    $writer.Dispose()
}
Write-Output $receiptPath
