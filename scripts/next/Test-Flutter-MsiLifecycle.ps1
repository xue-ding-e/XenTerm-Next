# Real application files, isolated CI-only MSI install/upgrade/uninstall.
# This is not a clean-Windows, standard-user, GUI, SSH or data-migration test.
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string] $ArtifactDirectory,
      [Parameter(Mandatory = $true)][string] $WixBin)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'Lifecycle fixture is restricted to a disposable GitHub Windows runner.'
}
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$sourceSha = 'a8aa668d6df0c2616dc8b0e72f55ee9a495d83a0'
$temp = Join-Path $env:RUNNER_TEMP ('xenterm-msi-lifecycle-' + [guid]::NewGuid())
$install = Join-Path $temp 'installed'
$activeMsi = $null
function Invoke-FixtureMsi {
    param([string[]] $Arguments)
    $process = Start-Process msiexec.exe -ArgumentList $Arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "msiexec did not finish without reboot: $($process.ExitCode)" }
}
$installer = New-Object -ComObject WindowsInstaller.Installer
function Get-FixtureProductCode {
    param([string] $Msi)
    $db = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($Msi, 0))
    $view = $db.OpenView("SELECT Value FROM Property WHERE Property='ProductCode'")
    $view.Execute()
    $record = $view.Fetch()
    $code = $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, @(1))
    $view.Close()
    return $code
}
function Assert-FixtureProductState {
    param([string] $Code, [int] $Expected)
    $state = $installer.GetType().InvokeMember('ProductState', 'GetProperty', $null, $installer, @($Code))
    if ($state -ne $Expected) { throw "Product $Code state $state, expected $Expected" }
}
try {
    New-Item -ItemType Directory -Path $temp | Out-Null
    $archives = @(Get-ChildItem -LiteralPath $ArtifactDirectory -Filter '*.zip')
    if ($archives.Count -ne 1) { throw 'Expected exactly one portable archive.' }
    $checksum = Get-Content -LiteralPath ([System.IO.Path]::ChangeExtension($archives[0].FullName, '.sha256')) -Raw
    $actual = (Get-FileHash $archives[0].FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($checksum.Split(' ')[0] -ne $actual) { throw 'Portable archive checksum mismatch.' }
    Expand-Archive -LiteralPath $archives[0].FullName -DestinationPath (Join-Path $temp 'unpacked')
    $bundle = Join-Path $temp "unpacked/XenTerm-Next-Flutter-$sourceSha-windows-x86_64"
    $runtime = Get-Content (Join-Path $bundle 'WINDOWS-RUNTIME.json') -Raw | ConvertFrom-Json
    if ($runtime.source_sha -ne $sourceSha) { throw 'Unexpected application source SHA.' }
    foreach ($name in @('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll')) {
        if ((Get-FileHash (Join-Path $bundle $name)).Hash.ToLowerInvariant() -ne $runtime.binary_sha256.$name) {
            throw "Application provenance mismatch: $name"
        }
    }
    $first = & (Join-Path $root 'apps/xenterm/tool/package_windows_installer.ps1') -Version lifecycle-first -ProductVersion 0.1.0 -BundleDir $bundle -OutputDir $temp -WixBin $WixBin
    $second = & (Join-Path $root 'apps/xenterm/tool/package_windows_installer.ps1') -Version lifecycle-upgrade -ProductVersion 0.1.1 -BundleDir $bundle -OutputDir $temp -WixBin $WixBin
    $firstCode = Get-FixtureProductCode $first
    $secondCode = Get-FixtureProductCode $second
    if ($firstCode -eq $secondCode) { throw 'Upgrade fixture must have distinct ProductCodes.' }
    Assert-FixtureProductState $firstCode -1
    Assert-FixtureProductState $secondCode -1
    Invoke-FixtureMsi -Arguments @('/i', "`"$first`"", '/qn', '/norestart', "INSTALLFOLDER=`"$install`"", '/l*v', "`"$temp/install.log`"")
    $activeMsi = $first
    Assert-FixtureProductState $firstCode 5
    Assert-FixtureProductState $secondCode -1
    $cli = Join-Path $install 'xenterm-cli.exe'
    $version = & $cli --version
    if ($LASTEXITCODE -ne 0 -or $version -ne "xenterm $($runtime.core_version)") { throw 'Installed CLI version mismatch.' }
    # A synthetic marker stands in for user data; no sessions or credentials.
    $profile = Join-Path $install 'config-flutter-next'
    New-Item -ItemType Directory -Path $profile -Force | Out-Null
    $marker = Join-Path $profile 'lifecycle-fixture.txt'
    Set-Content $marker 'SYNTHETIC PROFILE MUST SURVIVE'
    $markerHash = (Get-FileHash $marker).Hash
    Invoke-FixtureMsi -Arguments @('/i', "`"$second`"", '/qn', '/norestart', "INSTALLFOLDER=`"$install`"", '/l*v', "`"$temp/upgrade.log`"")
    $activeMsi = $second
    Assert-FixtureProductState $firstCode -1
    Assert-FixtureProductState $secondCode 5
    if ((Get-FileHash $marker).Hash -ne $markerHash) { throw 'Upgrade changed profile marker.' }
    foreach ($name in @('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll')) {
        if ((Get-FileHash (Join-Path $install $name)).Hash.ToLowerInvariant() -ne $runtime.binary_sha256.$name) { throw "Installed binary mismatch: $name" }
    }
    Invoke-FixtureMsi -Arguments @('/x', "`"$second`"", '/qn', '/norestart', '/l*v', "`"$temp/uninstall.log`"")
    $activeMsi = $null
    Assert-FixtureProductState $firstCode -1
    Assert-FixtureProductState $secondCode -1
    foreach ($name in @('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll')) {
        if (Test-Path (Join-Path $install $name)) { throw "Uninstall left managed binary: $name" }
    }
    if ((Get-FileHash $marker).Hash -ne $markerHash) { throw 'Uninstall changed profile marker.' }
    Write-Host "Lifecycle passed for application source $sourceSha; packaging source $env:GITHUB_SHA. No GUI/SSH or standard-user acceptance performed."
} catch {
    foreach ($log in @(Get-ChildItem -LiteralPath $temp -Filter '*.log' -ErrorAction SilentlyContinue)) {
        Write-Host "Synthetic fixture diagnostic: $($log.Name)"
        Get-Content -LiteralPath $log.FullName -Tail 80 | Write-Host
    }
    throw
} finally {
    if ($activeMsi) {
        try { Invoke-FixtureMsi -Arguments @('/x', "`"$activeMsi`"", '/qn', '/norestart') } catch { Write-Warning "Fixture cleanup failed: $_" }
    }
    # Keep only this isolated fixture's logs in runner temp for job diagnostics;
    # the disposable runner is destroyed afterwards. Never upload user data.
}
