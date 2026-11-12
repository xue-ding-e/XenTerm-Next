# Convert heat's file-keypath components to per-user MSI components (ICE38/64).
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string] $Path)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
[xml] $document = Get-Content -LiteralPath $Path -Raw
$namespace = 'http://schemas.microsoft.com/wix/2006/wi'
$namespaces = [System.Xml.XmlNamespaceManager]::new($document.NameTable)
$namespaces.AddNamespace('w', $namespace)
$components = @($document.SelectNodes('//w:Component', $namespaces))
if ($components.Count -eq 0) { throw 'No harvested components; refusing an empty MSI payload.' }
$cleanedDirectories = @{}
foreach ($component in $components) {
    $id = $component.GetAttribute('Id')
    if (-not $id) { throw 'Harvested component has no Id.' }
    $directory = $component.ParentNode
    if ($directory.LocalName -notin @('Directory', 'DirectoryRef')) {
        throw "Unexpected component parent for $id."
    }
    $directoryId = $directory.GetAttribute('Id')
    if (-not $directoryId) { throw "Component $id has no directory Id." }
    $component.RemoveAttribute('KeyPath')
    foreach ($file in @($component.SelectNodes('w:File', $namespaces))) {
        $file.SetAttribute('KeyPath', 'no')
    }
    # heat is invoked with -sreg: never reinterpret arbitrary registry imports.
    if ($component.SelectNodes('w:RegistryValue', $namespaces).Count -ne 0) {
        throw "Unexpected registry value in fresh harvest component $id."
    }
    $registry = $document.CreateElement('RegistryValue', $namespace)
    foreach ($entry in @{Root='HKCU'; Key='Software\XenTerm-Next\Flutter\InstallerComponents'; Name=$id; Type='integer'; Value='1'; KeyPath='yes'}.GetEnumerator()) {
        $registry.SetAttribute($entry.Key, $entry.Value)
    }
    [void] $component.AppendChild($registry)
    # RemoveFolder only removes empty directories. Runtime-created profiles are
    # not harvested and survive upgrades/uninstall; never add recursive removal.
    $cursor = $directory
    while ($cursor.LocalName -in @('Directory', 'DirectoryRef')) {
        $cleanupId = $cursor.GetAttribute('Id')
        if (-not $cleanedDirectories.ContainsKey($cleanupId)) {
            $remove = $document.CreateElement('RemoveFolder', $namespace)
            $remove.SetAttribute('Id', "Remove_$cleanupId")
            $remove.SetAttribute('Directory', $cleanupId)
            $remove.SetAttribute('On', 'uninstall')
            [void] $component.AppendChild($remove)
            $cleanedDirectories[$cleanupId] = $true
        }
        $cursor = $cursor.ParentNode
    }
}
$document.Save([System.IO.Path]::GetFullPath($Path))
