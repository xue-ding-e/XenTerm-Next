<#
.SYNOPSIS
    Accept the exact produced Next Flutter MSI on a disposable hosted runner.
.DESCRIPTION
    Verify the release receipt and portable bytes, build ONLY an older 0.1.0
    fixture, then install fixture -> supplied candidate -> uninstall. Pass the
    WiX directory provisioned by scripts/install-wix.ps1 with -WixBin; this
    script neither downloads tools nor rebuilds the candidate MSI.
    This checks packaging, CLI and profile preservation, not GUI/SSH acceptance
    or execution on clean Windows without the hosted runner's prerequisites.
    stdout result: XENTERM_RELEASE_MSI_ACCEPTANCE=<one JSON object>.
    EvidencePath must be a new JSON file beneath RUNNER_TEMP. Logs and synthetic
    profiles remain for diagnostics until the disposable runner is destroyed.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $ArtifactDirectory,
    [Parameter(Mandatory = $true)][string] $ExpectedSourceSha,
    [Parameter(Mandatory = $true)][string] $ExpectedRuntimeVersion,
    [Parameter(Mandatory = $true)][string] $WixBin,
    [ValidateSet('default', 'custom')][string] $InstallLocation = 'custom',
    [string] $EvidencePath = ''
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Validation helpers are deliberately free of installer side effects. The Python
# regression extracts their ASTs instead of dot-sourcing this CI-only program.
function Assert-HostedWindowsRunner {
    if ($env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_OS -cne 'Windows' -or
        $env:RUNNER_ENVIRONMENT -cne 'github-hosted' -or
        [string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
        throw 'Release MSI acceptance requires a disposable GitHub-hosted Windows runner with RUNNER_TEMP.'
    }
}

function Get-NormalizedPath {
    param([string] $Path)
    # Rust canonicalize may return an extended-length drive path. Do not strip
    # arbitrary UNC/device prefixes or silently accept network destinations.
    if ($Path -match '^\\\\\?\\[A-Za-z]:\\') { $Path = $Path.Substring(4) }
    if ($Path -notmatch '^[A-Za-z]:[\\/]') { throw "Expected an absolute local drive path: $Path" }
    return [IO.Path]::GetFullPath($Path).TrimEnd([char[]] '\/')
}

function Assert-PathWithin {
    param([string] $Path, [string] $Directory)
    $full = Get-NormalizedPath $Path
    $parent = Get-NormalizedPath $Directory
    if (-not $full.StartsWith($parent + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Path must be beneath RUNNER_TEMP: $Path"
    }
    return $full
}

function Assert-NoReparseAncestor {
    param([string] $Path)
    $cursor = [IO.Path]::GetFullPath($Path)
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            if ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Refusing a reparse point: $cursor"
            }
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
}

function Assert-SourceRuntime {
    param($Document, [string] $SourceSha, [string] $RuntimeVersion, [string] $Label)
    if ($SourceSha -cnotmatch '^[0-9a-f]{40}$' -or [string]::IsNullOrWhiteSpace($RuntimeVersion)) {
        throw 'Expected a full lowercase source SHA and a non-empty runtime version.'
    }
    if ($Document.source_sha -cne $SourceSha) { throw "$Label source SHA mismatch." }
    $versions = @($Document.PSObject.Properties | Where-Object { $_.Name -in @('runtime_version', 'core_version') })
    if ($versions.Count -eq 0) { throw "$Label has no runtime version." }
    foreach ($entry in $versions) {
        if ($entry.Value -cne $RuntimeVersion) { throw "$Label runtime version mismatch." }
    }
}

function Assert-ReleaseProductVersion {
    param([string] $Version)
    if ($Version -cnotmatch '^(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,4})$') {
        throw 'Receipt MSI ProductVersion must have three numeric fields.'
    }
    if ([int] $Matches[1] -gt 255 -or [int] $Matches[2] -gt 255 -or [int] $Matches[3] -gt 65535 -or
        [version] $Version -le [version] '0.1.0') {
        throw 'Receipt MSI ProductVersion must fit MSI limits and be newer than fixture 0.1.0.'
    }
}

function Assert-FileDigest {
    param([string] $Path, [string] $Digest, [long] $Size = -1)
    if ($Digest -cnotmatch '^[0-9a-f]{64}$' -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Missing file or invalid SHA256: $Path"
    }
    Assert-NoReparseAncestor $Path
    $file = Get-Item -LiteralPath $Path
    if ($file.Length -le 0 -or ($Size -ge 0 -and $file.Length -ne $Size) -or
        (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $Digest) {
        throw "Release file size/SHA256 mismatch: $Path"
    }
}

function Read-ReleaseInputs {
    param([string] $Directory, [string] $SourceSha, [string] $RuntimeVersion)
    $stem = "XenTerm-Next-Flutter-$SourceSha-windows-x86_64"
    $receiptPath = Join-Path $Directory "$stem-BUILD.json"
    $receipt = Get-Content -LiteralPath $receiptPath -Raw -Encoding UTF8 | ConvertFrom-Json
    Assert-SourceRuntime $receipt $SourceSha $RuntimeVersion 'Release receipt'
    Assert-ReleaseProductVersion $receipt.msi_product_version
    $expected = @("$stem.zip", "$stem.msi")
    $files = @{}
    foreach ($file in @($receipt.files)) {
        if ($file.name -cnotin $expected -or $files.ContainsKey($file.name) -or
            $file.size -isnot [ValueType] -or $file.size -is [bool] -or
            [decimal] $file.size -ne [long] $file.size -or [long] $file.size -le 0) {
            throw 'Receipt must list each canonical ZIP/MSI exactly once with a positive integral size.'
        }
        Assert-FileDigest (Join-Path $Directory $file.name) $file.sha256 ([long] $file.size)
        $files[$file.name] = $file
    }
    if ($files.Count -ne 2) { throw 'Receipt must contain both the canonical ZIP and candidate MSI.' }
    $checksums = @{}
    foreach ($line in @(Get-Content -LiteralPath (Join-Path $Directory "$stem.sha256") -Encoding UTF8)) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        if ($line -cnotmatch '^([0-9a-f]{64})[ \t]+\*?([^\r\n]+)$') { throw 'Invalid release checksum line.' }
        $digest = $Matches[1]
        $name = $Matches[2]
        if ($name -cnotin @($expected + "$stem-BUILD.json") -or $checksums.ContainsKey($name)) {
            throw 'Checksum manifest contains a duplicate or non-canonical file name.'
        }
        Assert-FileDigest (Join-Path $Directory $name) $digest
        $checksums[$name] = $digest
    }
    foreach ($name in $expected) {
        if (-not $checksums.ContainsKey($name) -or $checksums[$name] -cne $files[$name].sha256) {
            throw "Checksum manifest does not bind the receipt to $name."
        }
    }
    return [pscustomobject]@{
        Stem = $stem; Receipt = $receipt; Files = $files; ReceiptPath = $receiptPath
        Archive = (Join-Path $Directory "$stem.zip"); Candidate = (Join-Path $Directory "$stem.msi")
    }
}

function Assert-PortableArchive {
    param([string] $Archive, [string] $Stem)
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        $seen = @{}
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName
            $parts = $name.TrimEnd('/').Split('/')
            if (-not $name.StartsWith($Stem + '/', [StringComparison]::Ordinal) -or
                $name.Contains('\') -or $name.Contains(':') -or
                @($parts | Where-Object { $_ -in @('', '.', '..') -or $_ -match '[. ]$' }).Count -ne 0 -or
                (($entry.ExternalAttributes -shr 16) -band 0xF000) -eq 0xA000 -or
                $seen.ContainsKey($name.TrimEnd('/'))) {
                throw "Unsafe or duplicate portable ZIP member: $name"
            }
            if ($parts.Count -gt 1 -and $parts[1] -in @('config', 'config-flutter-next', 'log')) {
                throw 'Portable archive contains a profile or log directory.'
            }
            $seen[$name.TrimEnd('/')] = $true
        }
        if ($seen.Count -eq 0) { throw 'Empty portable archive.' }
    } finally { $zip.Dispose() }
}

function Get-BundleBinaryNames {
    param([string] $Directory, [string] $Relative = '')
    foreach ($file in Get-ChildItem -LiteralPath $Directory -Force) {
        Assert-NoReparseAncestor $file.FullName
        $name = if ($Relative) { $Relative + '/' + $file.Name } else { $file.Name }
        if ($file.PSIsContainer) {
            Get-BundleBinaryNames $file.FullName $name
        } elseif ($file.Extension.ToLowerInvariant() -in @('.exe', '.dll', '.so')) {
            $name
        }
    }
}

function Read-PortableRuntime {
    param([string] $Bundle, [string] $SourceSha, [string] $RuntimeVersion, $Receipt)
    $runtime = Get-Content -LiteralPath (Join-Path $Bundle 'WINDOWS-RUNTIME.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    Assert-SourceRuntime $runtime $SourceSha $RuntimeVersion 'WINDOWS-RUNTIME.json'
    if ($runtime.runtime_mode -cne 'app-local') { throw 'Release MSI requires an app-local VC runtime.' }
    $required = @('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll',
        'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')
    $hashes = @{}
    foreach ($entry in $runtime.binary_sha256.PSObject.Properties) {
        $name = $entry.Name
        if ($name -match '(^[\\/]|:|\\|(^|/)\.\.?(/|$))' -or $hashes.ContainsKey($name)) {
            throw 'Invalid or duplicate runtime hash path.'
        }
        Assert-FileDigest (Join-Path $Bundle $name) $entry.Value
        $hashes[$name] = [string] $entry.Value
    }
    foreach ($name in $required) {
        if (-not $hashes.ContainsKey($name)) { throw "Runtime receipt is missing a required binary hash: $name" }
    }
    # Compare every packaged DLL, including Flutter/plugins and any additional
    # app-local CRT files. Their bytes are bound to the platform receipt by ZIP SHA.
    # Build names from directory entries rather than slicing FullName: Windows
    # providers can expand an 8.3 temp path during enumeration.
    foreach ($name in @(Get-BundleBinaryNames $Bundle)) {
        $leaf = ($name -split '/')[-1]
        if ($leaf -match '^(msvcp|vcruntime|concrt|vcomp|vcamp)[0-9].*\.dll$' -and
            -not $hashes.ContainsKey($name)) { throw "App-local CRT hash is missing: $name" }
        if (-not $hashes.ContainsKey($name)) { throw "Runtime receipt is missing a packaged binary hash: $name" }
    }
    if (-not $hashes.ContainsKey('flutter_windows.dll')) { throw 'Missing Flutter engine DLL.' }
    # A publisher may also embed runtime provenance directly in the platform receipt.
    # When present it must describe exactly the same binary hashes and runtime mode.
    if ($Receipt.PSObject.Properties['windows_runtime']) {
        $embedded = $Receipt.windows_runtime
        Assert-SourceRuntime $embedded $SourceSha $RuntimeVersion 'Receipt windows_runtime'
        if ($embedded.runtime_mode -cne 'app-local') { throw 'Receipt runtime mode mismatch.' }
        $left = @($runtime.binary_sha256.PSObject.Properties | ForEach-Object { $_.Name + '=' + $_.Value } | Sort-Object)
        $right = @($embedded.binary_sha256.PSObject.Properties | ForEach-Object { $_.Name + '=' + $_.Value } | Sort-Object)
        if (Compare-Object $left $right -CaseSensitive) { throw 'Receipt/runtime binary hashes disagree.' }
    }
    return $hashes
}

function Read-MsiRows {
    param($Installer, [string] $Msi, [string] $Query, [int] $Columns)
    $database = $null; $view = $null
    try {
        $database = $Installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $Installer, @($Msi, 0))
        $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @($Query))
        [void] $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null)
        while ($true) {
            $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
            if ($null -eq $record) { break }
            try {
                $values = @()
                for ($column = 1; $column -le $Columns; $column++) {
                    $values += [string] $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, @($column))
                }
                [pscustomobject]@{ Values = $values }
            } finally { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) }
        }
    } finally {
        if ($view) {
            [void] $view.GetType().InvokeMember('Close', 'InvokeMethod', $null, $view, $null)
            [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
        }
        if ($database) { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) }
    }
}

function Read-MsiIdentity {
    param($Installer, [string] $Msi, [string] $ProductVersion, [string[]] $BinaryNames)
    $properties = @{}
    foreach ($row in @(Read-MsiRows $Installer $Msi 'SELECT `Property`, `Value` FROM `Property`' 2)) {
        $properties[$row.Values[0]] = $row.Values[1]
    }
    $upgradeCode = '{D308C8B0-2E75-4F13-B4A8-7DD705095893}'
    if ($properties.UpgradeCode -ine $upgradeCode -or $properties.ProductName -cne 'XenTerm Next Flutter' -or
        $properties.ProductVersion -cne $ProductVersion -or
        $properties.ProductCode -notmatch '^\{[0-9A-Fa-f]{8}(-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}\}$' -or
        ($properties.ContainsKey('ALLUSERS') -and $properties.ALLUSERS -ne '')) {
        throw 'MSI product identity, numeric ProductVersion or per-user scope mismatch.'
    }
    $upgrades = @(Read-MsiRows $Installer $Msi 'SELECT `UpgradeCode` FROM `Upgrade`' 1)
    if ($upgrades.Count -eq 0) { throw 'MSI has no native major-upgrade rule.' }
    foreach ($row in $upgrades) {
        if ($row.Values[0] -ine $upgradeCode) { throw 'MSI upgrade rule touches another product family.' }
    }
    $files = @(Read-MsiRows $Installer $Msi 'SELECT `FileName` FROM `File`' 1 | ForEach-Object { ($_.Values[0] -split '\|')[-1] })
    foreach ($name in $BinaryNames) {
        if ([IO.Path]::GetFileName($name) -cnotin $files) { throw "MSI File table is missing $name." }
    }
    $registry = @{}
    $hasInstallLocation = $false
    foreach ($row in @(Read-MsiRows $Installer $Msi 'SELECT `Registry`, `Root`, `Key`, `Name`, `Value` FROM `Registry`' 5)) {
        $values = $row.Values
        if ($values[1] -ne '1' -or ($values[2] -ine 'Software\XenTerm-Next\Flutter' -and
            $values[2] -ine 'Software\XenTerm-Next\Flutter\InstallerComponents')) {
            throw 'MSI registry resources escape the independent per-user Flutter namespace.'
        }
        if ($values[2] -ieq 'Software\XenTerm-Next\Flutter' -and $values[3] -ceq 'InstallLocation' -and
            $values[4] -ceq '[INSTALLFOLDER]') { $hasInstallLocation = $true }
        $registry[$values[0]] = $true
    }
    if (-not $hasInstallLocation) { throw 'MSI is missing its native install-location registry value.' }
    foreach ($row in @(Read-MsiRows $Installer $Msi 'SELECT `Attributes`, `KeyPath` FROM `Component`' 2)) {
        if (([int] $row.Values[0] -band 4) -eq 0 -or -not $registry.ContainsKey($row.Values[1])) {
            throw 'MSI component must retain its isolated HKCU registry keypath.'
        }
    }
    foreach ($row in @(Read-MsiRows $Installer $Msi 'SELECT `FileName` FROM `RemoveFile`' 1)) {
        if ($row.Values[0]) { throw 'MSI must use empty-directory cleanup, never profile/file deletion rules.' }
    }
    return ([guid] $properties.ProductCode).ToString('B').ToUpperInvariant()
}

function Get-MsiStringList {
    param($Installer, [string] $Property, [object[]] $Arguments)
    $list = $Installer.GetType().InvokeMember($Property, 'GetProperty', $null, $Installer, $Arguments)
    try {
        # Adapted .Count can count the COM wrapper itself. A COM error or null
        # must never be treated as an empty list and bypass the ownership gate.
        $count = $list.GetType().InvokeMember('Count', 'GetProperty', $null, $list, $null)
        if ($null -eq $count -or [int] $count -lt 0) { throw 'Windows Installer returned an invalid StringList Count.' }
        for ($index = 0; $index -lt [int] $count; $index++) {
            $code = $list.GetType().InvokeMember('Item', 'GetProperty', $null, $list, @($index))
            ([guid] $code).ToString('B').ToUpperInvariant()
        }
    } finally { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($list) }
}

function Get-MsiProductState {
    param($Installer, [string] $Code)
    $state = $Installer.GetType().InvokeMember('ProductState', 'GetProperty', $null, $Installer, @($Code))
    if ($null -eq $state) { throw 'Windows Installer returned no ProductState.' }
    return [int] $state
}

function Assert-MsiProductState {
    param($Installer, [string] $Code, [int] $Expected)
    $state = Get-MsiProductState $Installer $Code
    if ($state -ne $Expected) { throw "Product $Code state $state, expected $Expected." }
}

function Get-OtherProductsSnapshot {
    param($Installer, [string[]] $OwnedCodes = @())
    foreach ($code in @(Get-MsiStringList $Installer 'Products' $null | Sort-Object)) {
        if ($code -in $OwnedCodes) { continue }
        $state = Get-MsiProductState $Installer $code
        # Products also enumerates advertised registrations. Their installed
        # properties may be unavailable; preserve their identity/state instead
        # of querying ProductInfo as though they were installed packages.
        if ($state -ne 5) {
            "$code|$state|<not-installed>|<not-installed>"
            continue
        }
        $version = $Installer.GetType().InvokeMember('ProductInfo', 'GetProperty', $null, $Installer, @([string]$code, [string]'VersionString'))
        $location = $Installer.GetType().InvokeMember('ProductInfo', 'GetProperty', $null, $Installer, @([string]$code, [string]'InstallLocation'))
        "$code|$state|$version|$location"
    }
}

function Assert-OtherProductsUnchanged {
    param($Installer, [string[]] $Before, [string[]] $OwnedCodes)
    $after = @(Get-OtherProductsSnapshot $Installer $OwnedCodes)
    if (($Before -join "`n") -cne ($after -join "`n")) { throw 'Other installed MSI products changed during release acceptance.' }
}

function Assert-UpgradeFamily {
    param($Installer, [string] $UpgradeCode, [string[]] $Expected = @())
    $actual = @(Get-MsiStringList $Installer 'RelatedProducts' @($UpgradeCode) | Sort-Object)
    if (($actual -join ',') -ine ((@($Expected | Sort-Object)) -join ',')) {
        throw 'Unexpected product in the isolated Flutter UpgradeCode family.'
    }
}

function Invoke-ReleaseMsi {
    param([string[]] $Arguments, [string] $TaskDirectory, [string] $LogName)
    if ($LogName -notmatch '^[a-z-]+\.log$') { throw 'Invalid MSI log name.' }
    $log = Join-Path $TaskDirectory $LogName
    $allArguments = @($Arguments) + @('/qn', '/norestart', '/l*v', "`"$log`"")
    $process = Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\msiexec.exe') `
        -ArgumentList $allArguments -Wait -PassThru -WindowStyle Hidden
    if ($process.ExitCode -ne 0) { throw "msiexec failed or requested reboot: $($process.ExitCode). Log: $log" }
}

function Assert-InstalledPayload {
    param([string] $Directory, [hashtable] $Hashes)
    foreach ($name in $Hashes.Keys) { Assert-FileDigest (Join-Path $Directory $name) $Hashes[$name] }
    $registered = Get-ItemPropertyValue -LiteralPath 'HKCU:\Software\XenTerm-Next\Flutter' -Name InstallLocation
    if ((Get-NormalizedPath $registered) -ine (Get-NormalizedPath $Directory)) {
        throw 'Native install-location registry value did not preserve the selected directory.'
    }
}

function Invoke-InstalledCli {
    param([string] $InstallDirectory, [string] $TaskDirectory, [string] $Phase, [string] $RuntimeVersion)
    $cli = Join-Path $InstallDirectory 'xenterm-cli.exe'
    foreach ($probe in @('version', 'config-info')) {
        $stdout = Join-Path $TaskDirectory "$Phase-$probe.stdout.log"
        $stderr = Join-Path $TaskDirectory "$Phase-$probe.stderr.log"
        $process = Start-Process -FilePath $cli -ArgumentList @("--$probe") -WorkingDirectory $TaskDirectory `
            -Wait -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
        $process.Refresh()
        if ($null -eq $process.ExitCode -or $process.ExitCode -ne 0) {
            $diagnostic = if (Test-Path -LiteralPath $stderr) { Get-Content -LiteralPath $stderr -Raw -Encoding UTF8 } else { '<no stderr>' }
            $exitText = if ($null -eq $process.ExitCode) { '<missing>' } else { [string]$process.ExitCode }
            throw "Installed CLI --$probe failed with exit code $exitText. stderr: $diagnostic"
        }
        $output = (Get-Content -LiteralPath $stdout -Raw -Encoding UTF8).Trim()
        if ($probe -eq 'version') {
            if ($output -cne "xenterm $RuntimeVersion") { throw 'Installed CLI runtime version mismatch.' }
        } else {
            $info = $output | ConvertFrom-Json
            $profile = Join-Path $InstallDirectory 'config-flutter-next'
            if ((Get-NormalizedPath $info.data_dir) -ine (Get-NormalizedPath $profile) -or $info.session_count -ne 0) {
                throw 'Installed CLI must use adjacent config-flutter-next with zero synthetic sessions.'
            }
            return $info
        }
    }
}

function Assert-UninstalledPayload {
    param([string] $Directory, [hashtable] $Hashes, [string] $Marker, [string] $MarkerHash)
    foreach ($name in $Hashes.Keys) {
        if (Test-Path -LiteralPath (Join-Path $Directory $name)) { throw "Uninstall left managed binary: $name" }
    }
    Assert-FileDigest $Marker $MarkerHash
    $key = 'HKCU:\Software\XenTerm-Next\Flutter'
    if (Test-Path -LiteralPath $key) {
        foreach ($item in @((Get-Item -LiteralPath $key)) + @(Get-ChildItem -LiteralPath $key -Recurse)) {
            if ($item.GetValueNames().Count -ne 0) { throw 'Uninstall left native installer registry values.' }
        }
    }
}

function Write-AcceptanceEvidence {
    param([string] $Path, $Result)
    $json = $Result | ConvertTo-Json -Depth 8
    # CreateNew prevents a previous passing receipt from surviving a failed rerun.
    $stream = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
        $bytes = [Text.UTF8Encoding]::new($false).GetBytes($json + "`n")
        $stream.Write($bytes, 0, $bytes.Length)
    } finally { $stream.Dispose() }
    Write-Output ('XENTERM_RELEASE_MSI_ACCEPTANCE=' + ($Result | ConvertTo-Json -Depth 8 -Compress))
}

# The gate precedes all directory creation, COM access, packaging and execution.
Assert-HostedWindowsRunner
if ($ExpectedSourceSha -cnotmatch '^[0-9a-f]{40}$' -or [string]::IsNullOrWhiteSpace($ExpectedRuntimeVersion)) {
    throw 'Expected a full lowercase source SHA and a non-empty runtime version.'
}
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$runnerTemp = Get-NormalizedPath $env:RUNNER_TEMP
Assert-NoReparseAncestor $runnerTemp
if (-not (Test-Path -LiteralPath $runnerTemp -PathType Container)) { throw 'RUNNER_TEMP must already exist.' }
$taskDirectory = Assert-PathWithin (Join-Path $runnerTemp ('xenterm-release-msi-' + [guid]::NewGuid())) $runnerTemp
if ([string]::IsNullOrWhiteSpace($EvidencePath)) { $EvidencePath = Join-Path $taskDirectory 'acceptance.json' }
$EvidencePath = Assert-PathWithin $EvidencePath $runnerTemp
Assert-NoReparseAncestor $EvidencePath
if ([IO.Path]::GetExtension($EvidencePath) -ine '.json' -or (Test-Path -LiteralPath $EvidencePath)) {
    throw 'EvidencePath must name a new JSON file beneath RUNNER_TEMP.'
}
$defaultInstall = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'XenTerm Next Flutter'
$install = if ($InstallLocation -eq 'default') { $defaultInstall } else { Join-Path $taskDirectory 'custom install' }
# A hosted image may not have created its Desktop/Programs directories yet.
# Resolve shell destinations without requiring those directories to exist.
$desktop = [Environment]::GetFolderPath([Environment+SpecialFolder]::DesktopDirectory, [Environment+SpecialFolderOption]::DoNotVerify)
$programs = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs, [Environment+SpecialFolderOption]::DoNotVerify)
foreach ($path in @($defaultInstall, $install, (Join-Path $install 'config'), (Join-Path $install 'config-flutter-next'),
        (Join-Path $desktop 'XenTerm Next Flutter.lnk'), (Join-Path $programs 'XenTerm Next Flutter'))) {
    Assert-NoReparseAncestor $path
    if (Test-Path -LiteralPath $path) { throw 'Refusing an existing default/custom installation, profile or Flutter shortcut.' }
}
if (Test-Path -LiteralPath 'HKCU:\Software\XenTerm-Next\Flutter') {
    throw 'Refusing pre-existing Flutter installer registry/configuration.'
}
$WixBin = (Resolve-Path -LiteralPath $WixBin).Path
foreach ($tool in @('candle.exe', 'light.exe', 'heat.exe', 'WixUIExtension.dll', 'WixUtilExtension.dll')) {
    if (-not (Test-Path -LiteralPath (Join-Path $WixBin $tool) -PathType Leaf)) { throw "Pinned -WixBin is missing $tool." }
}

$installer = $null
$trackedProducts = [Collections.Generic.List[string]]::new()
$savedEnvironment = @{}
$before = @()
$failure = $null
$result = [ordered]@{
    schema_version = 1; acceptance = 'exact-produced-windows-msi'; passed = $false
    source_sha = $ExpectedSourceSha; runtime_version = $ExpectedRuntimeVersion; msi_product_version = $null
    install_location = $InstallLocation; install_directory = $install
    upgrade_code = '{D308C8B0-2E75-4F13-B4A8-7DD705095893}'
    candidate_product_code = $null; fixture_product_code = $null
    candidate_msi_sha256 = $null; portable_zip_sha256 = $null; binary_sha256 = $null
    config_info = $null; uninstall_verified = $false; phases = @(); logs_directory = $taskDirectory
}
try {
    New-Item -ItemType Directory -Path $taskDirectory | Out-Null
    $evidenceDirectory = Split-Path -Parent $EvidencePath
    if (-not (Test-Path -LiteralPath $evidenceDirectory)) { New-Item -ItemType Directory -Path $evidenceDirectory -Force | Out-Null }
    $ArtifactDirectory = (Resolve-Path -LiteralPath $ArtifactDirectory).Path
    $inputs = Read-ReleaseInputs $ArtifactDirectory $ExpectedSourceSha $ExpectedRuntimeVersion
    $result.msi_product_version = $inputs.Receipt.msi_product_version
    $result.candidate_msi_sha256 = $inputs.Files["$($inputs.Stem).msi"].sha256
    $result.portable_zip_sha256 = $inputs.Files["$($inputs.Stem).zip"].sha256
    Assert-PortableArchive $inputs.Archive $inputs.Stem
    $unpacked = Join-Path $taskDirectory 'portable'
    Expand-Archive -LiteralPath $inputs.Archive -DestinationPath $unpacked
    $bundle = Join-Path $unpacked $inputs.Stem
    $hashes = Read-PortableRuntime $bundle $ExpectedSourceSha $ExpectedRuntimeVersion $inputs.Receipt
    $result.binary_sha256 = $hashes
    $installer = New-Object -ComObject WindowsInstaller.Installer
    Assert-UpgradeFamily $installer $result.upgrade_code
    $before = @(Get-OtherProductsSnapshot $installer)
    $candidateCode = Read-MsiIdentity $installer $inputs.Candidate $inputs.Receipt.msi_product_version @($hashes.Keys)
    $result.candidate_product_code = $candidateCode
    Assert-MsiProductState $installer $candidateCode -1
    # The only MSI built here is the old fixture. Its copied bundle has exactly
    # the portable's DLL/GUI/CLI bytes; no stage edits or installer-rule overrides.
    $fixture = & (Join-Path $root 'apps/xenterm/tool/package_windows_installer.ps1') `
        -Version release-acceptance-old -ProductVersion 0.1.0 -BundleDir $bundle -OutputDir $taskDirectory -WixBin $WixBin
    if (@($fixture).Count -ne 1 -or -not (Test-Path -LiteralPath $fixture -PathType Leaf)) { throw 'Old fixture MSI build did not return one file.' }
    $fixtureCode = Read-MsiIdentity $installer $fixture '0.1.0' @($hashes.Keys)
    $result.fixture_product_code = $fixtureCode
    if ($fixtureCode -eq $candidateCode) { throw 'Old fixture and candidate must have distinct ProductCodes.' }
    Assert-MsiProductState $installer $fixtureCode -1
    Assert-UpgradeFamily $installer $result.upgrade_code
    foreach ($name in @('XENTERM_DATA_DIR', 'MEATSHELL_DATA_DIR')) {
        $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        # PowerShell can coerce $null to an empty string on modern .NET. An
        # empty data-dir override is deliberately invalid; remove the key.
        [Environment]::SetEnvironmentVariable($name, [NullString]::Value, 'Process')
    }
    # Track before invoking msiexec: a failed command may still have registered
    # a product. Cleanup is exclusively by these two verified, previously absent IDs.
    $trackedProducts.Add($fixtureCode)
    $installArguments = @('/i', "`"$fixture`"")
    if ($InstallLocation -eq 'custom') { $installArguments += "INSTALLFOLDER=`"$install`"" }
    Invoke-ReleaseMsi $installArguments $taskDirectory 'install-old.log'
    Assert-MsiProductState $installer $fixtureCode 5
    Assert-MsiProductState $installer $candidateCode -1
    Assert-UpgradeFamily $installer $result.upgrade_code @($fixtureCode)
    Assert-OtherProductsUnchanged $installer $before @($fixtureCode, $candidateCode)
    Assert-InstalledPayload $install $hashes
    $null = Invoke-InstalledCli $install $taskDirectory 'old' $ExpectedRuntimeVersion
    $marker = Join-Path $install 'config-flutter-next/release-msi-synthetic.txt'
    if (-not (Test-Path -LiteralPath (Split-Path -Parent $marker))) {
        New-Item -ItemType Directory -Path (Split-Path -Parent $marker) | Out-Null
    }
    [IO.File]::WriteAllText($marker, 'SYNTHETIC PROFILE MUST SURVIVE ' + [guid]::NewGuid())
    $markerHash = (Get-FileHash -LiteralPath $marker -Algorithm SHA256).Hash.ToLowerInvariant()
    $result.phases += 'installed-old-fixture'

    # Recheck the input bytes immediately before installing the actual artifact.
    # No INSTALLFOLDER override here: the native registry search must recover it.
    Assert-FileDigest $inputs.Candidate $result.candidate_msi_sha256 ([long] $inputs.Files["$($inputs.Stem).msi"].size)
    $trackedProducts.Add($candidateCode)
    Invoke-ReleaseMsi @('/i', "`"$($inputs.Candidate)`"") $taskDirectory 'upgrade-candidate.log'
    Assert-MsiProductState $installer $fixtureCode -1
    Assert-MsiProductState $installer $candidateCode 5
    Assert-UpgradeFamily $installer $result.upgrade_code @($candidateCode)
    Assert-OtherProductsUnchanged $installer $before @($fixtureCode, $candidateCode)
    Assert-InstalledPayload $install $hashes
    Assert-FileDigest $marker $markerHash
    $info = Invoke-InstalledCli $install $taskDirectory 'candidate' $ExpectedRuntimeVersion
    Assert-InstalledPayload $install $hashes
    $result.config_info = [ordered]@{ data_dir = $info.data_dir; session_count = $info.session_count }
    $result.phases += 'upgraded-exact-candidate'

    Invoke-ReleaseMsi @('/x', "`"$candidateCode`"") $taskDirectory 'uninstall-candidate.log'
    Assert-MsiProductState $installer $fixtureCode -1
    Assert-MsiProductState $installer $candidateCode -1
    Assert-UpgradeFamily $installer $result.upgrade_code
    Assert-OtherProductsUnchanged $installer $before @($fixtureCode, $candidateCode)
    Assert-UninstalledPayload $install $hashes $marker $markerHash
    $result.uninstall_verified = $true
    $result.phases += 'uninstalled-preserved-profile'
} catch {
    $failure = $_
} finally {
    if ($installer) {
        for ($index = $trackedProducts.Count - 1; $index -ge 0; $index--) {
            $code = $trackedProducts[$index]
            try {
                if ((Get-MsiProductState $installer $code) -ne -1) {
                    $logName = if ($code -eq $result.candidate_product_code) { 'cleanup-candidate.log' } else { 'cleanup-old.log' }
                    Invoke-ReleaseMsi @('/x', "`"$code`"") $taskDirectory $logName
                }
                Assert-MsiProductState $installer $code -1
            } catch {
                if ($null -eq $failure) { $failure = $_ } else { Write-Warning "Tracked product cleanup failed: $_" }
            }
        }
        [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)
    }
    foreach ($name in $savedEnvironment.Keys) {
        $restoreValue = if ($null -eq $savedEnvironment[$name]) { [NullString]::Value } else { $savedEnvironment[$name] }
        [Environment]::SetEnvironmentVariable($name, $restoreValue, 'Process')
    }
    # No name-based process termination, directory deletion or manual registry
    # edits. Only native uninstall of tracked fixture/candidate ProductCodes.
}
if ($failure) {
    $result.error = $failure.Exception.Message
} else {
    # Emit passing evidence only after normal uninstall AND final cleanup checks.
    $result.passed = $true
}
Write-AcceptanceEvidence $EvidencePath $result
if ($failure) { throw $failure }
