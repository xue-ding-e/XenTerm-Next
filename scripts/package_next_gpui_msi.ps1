[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$BundleDir,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{40}$')][string]$SourceSha,
    [Parameter(Mandatory=$true)][string]$WixBin,
    [Parameter(Mandatory=$true)][string]$Output,
    # A lower version is permitted only for installer upgrade QA, not release.
    [string]$ProductVersion = '',
    [switch]$UpgradeTestFixture
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$BundleDir = (Resolve-Path -LiteralPath $BundleDir).Path
$Output = [IO.Path]::GetFullPath($Output)
$WixBin = (Resolve-Path -LiteralPath $WixBin).Path
$infoText = & python "$PSScriptRoot/prepare_next_gpui_msi.py" --bundle $BundleDir --source-sha $SourceSha
if ($LASTEXITCODE -ne 0) { throw 'GPUI payload validation failed' }
$info = $infoText | ConvertFrom-Json
$derivedVersion = & python "$PSScriptRoot/prepare_next_gpui_msi.py" --bundle $BundleDir --source-sha $SourceSha --installer-version
if ($LASTEXITCODE -ne 0) { throw 'Cannot derive monotonic installer version' }
if (!$ProductVersion) { $ProductVersion = $derivedVersion }
if ($ProductVersion -notmatch '^\d+\.\d+\.\d+$') { throw 'MSI needs a three-field numeric version' }
$fields = @($ProductVersion.Split('.') | ForEach-Object { [int]$_ })
if ($fields[0] -gt 255 -or $fields[1] -gt 255 -or $fields[2] -gt 65535) { throw 'Version exceeds Windows Installer limits' }
if ($ProductVersion -ne $derivedVersion -and !$UpgradeTestFixture) { throw 'Release MSI version must match payload' }
# A version probe must not create a portable profile inside the payload.
$oldDataDir = [Environment]::GetEnvironmentVariable('XENTERM_DATA_DIR', 'Process')
$probeProfile = Join-Path ([IO.Path]::GetTempPath()) ('next-gpui-version-' + [guid]::NewGuid().ToString('N'))
try {
    $env:XENTERM_DATA_DIR = $probeProfile
    $actual = (& "$BundleDir/xenterm.exe" --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $actual -ne $info.binary_version) { throw "Payload runtime version mismatch: $actual" }
} finally {
    [Environment]::SetEnvironmentVariable('XENTERM_DATA_DIR', $oldDataDir, 'Process')
}
& python "$PSScriptRoot/prepare_next_gpui_msi.py" --bundle $BundleDir --source-sha $SourceSha | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Payload was polluted during version probe' }
$dumpbinCommand = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
if ($dumpbinCommand) { $dumpbin = $dumpbinCommand.Source }
else {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    if (!(Test-Path -LiteralPath $vswhere)) { throw 'Visual Studio C++ tools are required for dependency inspection' }
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (!$vs) { throw 'Visual Studio x64 C++ tools not found' }
    $dumpbin = Get-ChildItem -Path "$vs/VC/Tools/MSVC/*/bin/Hostx64/x64/dumpbin.exe" | Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
    if (!$dumpbin) { throw 'dumpbin.exe not found' }
}
# Inspect every PE: non-system dependencies must travel beside the executable.
# Static MSVC is required by the GPUI release pipeline; never silently rely on
# the runner's preinstalled Visual C++ redistributable.
$systemDlls = '^(?i)(api-ms-win-.*|ext-ms-win-.*|kernel32|kernelbase|ntdll|user32|gdi32|gdi32full|advapi32|shell32|shlwapi|ole32|oleaut32|combase|comdlg32|crypt32|bcrypt|bcryptprimitives|ncrypt|secur32|sspicli|ws2_32|iphlpapi|dnsapi|userenv|version|winmm|imm32|setupapi|cfgmgr32|dwmapi|uxtheme|d3d11|d3d12|dxgi|dwrite|d2d1|dcomp|opengl32|msvcrt|ucrtbase|powrprof|propsys|winhttp|wininet|normaliz|dbghelp|hid|wtsapi32|netapi32|psapi|msimg32|winspool|avrt|uiautomationcore|comctl32|icuuc|pdh|gdiplus)\.dll$'
foreach ($pe in (Get-ChildItem -LiteralPath $BundleDir -Recurse -File | Where-Object { $_.Extension -in '.exe','.dll' })) {
    $deps = & $dumpbin /DEPENDENTS $pe.FullName
    if ($LASTEXITCODE -ne 0) { throw "dumpbin failed: $($pe.FullName)" }
    foreach ($line in $deps) {
        if ($line -match '^\s+([A-Za-z0-9_.-]+\.dll)\s*$') {
            $dll = $Matches[1]
            if ($dll -notmatch $systemDlls -and !(Test-Path -LiteralPath (Join-Path $pe.DirectoryName $dll))) {
                throw "Missing runtime DLL $dll required by $($pe.Name)"
            }
        }
    }
}
$work = Join-Path ([IO.Path]::GetTempPath()) ('next-gpui-msi-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
New-Item -ItemType Directory -Path (Split-Path $Output -Parent) -Force | Out-Null
# WiX's license UI needs RTF; retain the complete original license in payload.
$license = [IO.File]::ReadAllText((Join-Path $BundleDir 'LICENSE'))
$license = $license.Replace('\','\\').Replace('{','\{').Replace('}','\}').Replace("`r",'').Replace("`n",'\par ')
$rtf = Join-Path $work 'LICENSE.rtf'
[IO.File]::WriteAllText($rtf, '{\rtf1\ansi ' + $license + '}', [Text.Encoding]::UTF8)
$harvest = Join-Path $work 'files.wxs'
& "$WixBin/heat.exe" dir $BundleDir -nologo -ag -srd -sreg -scom -dr INSTALLFOLDER -cg GPUIFiles -var var.SourceDir -out $harvest
if ($LASTEXITCODE -ne 0) { throw 'WiX harvest failed' }
& python "$PSScriptRoot/prepare_next_gpui_msi.py" --harvest $harvest
if ($LASTEXITCODE -ne 0) { throw 'Per-user harvest conversion failed' }
& "$WixBin/candle.exe" -nologo -arch x64 -ext WixUIExtension "-dSourceDir=$BundleDir" "-dProductVersion=$ProductVersion" "-dRuntimeVersion=$($info.runtime_version)" "-dSourceSha=$SourceSha" "-dIconFile=$root/assets/xenterm.ico" "-dLicenseRtf=$rtf" -out "$work/" "$root/wix/next-gpui.wxs" $harvest
if ($LASTEXITCODE -ne 0) { throw 'WiX compilation failed' }
& "$WixBin/light.exe" -nologo -ext WixUIExtension -out $Output "$work/next-gpui.wixobj" "$work/files.wixobj"
if ($LASTEXITCODE -ne 0) { throw 'WiX linking/ICE validation failed' }
$receipt = @{
    product = 'XenTerm-Next GPUI'; source_sha = $SourceSha; runtime_version = $info.runtime_version
    build_time_utc = [DateTimeOffset]::FromUnixTimeSeconds([long]$info.version_timestamp).UtcDateTime.ToString('o')
    package_version = $info.package_version; installer_version = $ProductVersion; upgrade_test_fixture = [bool]$UpgradeTestFixture
    sha256 = (Get-FileHash -LiteralPath $Output -Algorithm SHA256).Hash.ToLowerInvariant()
    payload = @(Get-ChildItem -LiteralPath $BundleDir -Recurse -File | ForEach-Object {
        @{ path = [IO.Path]::GetRelativePath($BundleDir, $_.FullName).Replace('\','/'); sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
    })
}
$receipt | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath "$Output.json" -Encoding utf8
Write-Host "Built unsigned real GPUI MSI: $Output (source $SourceSha)"
