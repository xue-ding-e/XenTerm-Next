# Synthetic packaging validation only. Never install the generated MSI.
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string] $WixBin)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$temp = Join-Path ([System.IO.Path]::GetTempPath()) ('xenterm-msi-fixture-' + [guid]::NewGuid())
try {
    $bundle = Join-Path $temp 'bundle'
    foreach ($name in @('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll',
            'data/flutter_assets/fixture.txt', 'config/sessions.db',
            'config-flutter-next/sessions.db', 'log/fixture.log')) {
        $file = Join-Path $bundle $name
        New-Item -ItemType Directory -Path (Split-Path $file) -Force | Out-Null
        Set-Content -LiteralPath $file -Value 'SYNTHETIC FIXTURE; NOT EXECUTABLE; NO USER DATA'
    }
    & (Join-Path $root 'apps/xenterm/tool/package_windows_installer.ps1') `
        -Version v0.1.0 -BundleDir $bundle -OutputDir $temp -WixBin $WixBin -KeepWorkDir
    $work = Join-Path $root 'target/wix/next-flutter-installer'
    foreach ($excluded in @('config', 'config-flutter-next', 'log')) {
        if (Test-Path (Join-Path $work "stage/$excluded")) { throw "Private directory staged: $excluded" }
    }
    [xml] $xml = Get-Content (Join-Path $work 'Files.wxs') -Raw
    $ns = [System.Xml.XmlNamespaceManager]::new($xml.NameTable)
    $ns.AddNamespace('w', 'http://schemas.microsoft.com/wix/2006/wi')
    $components = @($xml.SelectNodes('//w:Component', $ns))
    if ($components.Count -lt 4) { throw 'Fixture components unexpectedly missing.' }
    foreach ($component in $components) {
        $keys = @($component.SelectNodes('w:RegistryValue[@Root="HKCU"][@KeyPath="yes"]', $ns))
        if ($keys.Count -ne 1) { throw 'Missing unique per-user registry keypath.' }
        if ($component.SelectNodes('w:File[@KeyPath="yes"]', $ns).Count) { throw 'File keypath survived.' }
    }
    foreach ($directory in @($xml.SelectNodes('//w:Directory | //w:DirectoryRef', $ns))) {
        $id = $directory.GetAttribute('Id')
        if ($xml.SelectNodes("//w:RemoveFolder[@Directory='$id'][@On='uninstall']", $ns).Count -ne 1) {
            throw "Missing unique empty-folder cleanup: $id"
        }
    }
    if ($xml.SelectNodes('//w:RemoveFile', $ns).Count) { throw 'Unexpected file deletion rule.' }
    if (-not (Test-Path (Join-Path $temp 'xenterm-next-flutter-v0.1.0-windows-x86_64.msi'))) {
        throw 'Validated fixture MSI missing.'
    }
    Write-Host 'Synthetic MSI compiled and passed WiX validation; no install was performed.'
} finally {
    if (Test-Path $temp) { Remove-Item $temp -Recurse -Force }
}
