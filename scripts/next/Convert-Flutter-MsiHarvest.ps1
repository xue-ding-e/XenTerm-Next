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
$seenTargets = @{}
foreach ($component in $components) {
    $id = $component.GetAttribute('Id')
    if (-not $id) { throw 'Harvested component has no Id.' }
    $directory = $component.ParentNode
    if ($directory.LocalName -notin @('Directory', 'DirectoryRef')) {
        throw "Unexpected component parent for $id."
    }
    $directoryId = $directory.GetAttribute('Id')
    if (-not $directoryId) { throw "Component $id has no directory Id." }
    $files = @($component.SelectNodes('w:File', $namespaces))
    if ($files.Count -ne 1) { throw "Expected exactly one file per heat component: $id" }
    $fileName = $files[0].GetAttribute('Name')
    if (-not $fileName) { $fileName = [System.IO.Path]::GetFileName($files[0].GetAttribute('Source')) }
    $parts = [System.Collections.Generic.List[string]]::new()
    $parts.Add($fileName)
    $ancestor = $directory
    while ($ancestor.LocalName -eq 'Directory') {
        $parts.Insert(0, $ancestor.GetAttribute('Name'))
        $ancestor = $ancestor.ParentNode
    }
    $target = ($parts -join '/').ToLowerInvariant()
    if ($seenTargets.ContainsKey($target)) { throw "Duplicate installed path: $target" }
    $seenTargets[$target] = $true
    # Explicit deterministic GUIDs: WiX 3 forbids Guid=* with both files and
    # a registry keypath (CNDL0230). Hash only the product namespace + target
    # path, never the checkout path, file contents, build version or heat Id.
    $bytes = [System.Text.Encoding]::UTF8.GetBytes("XenTerm-Next/Flutter/MSI/component/v1/$target")
    $hex = [Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($bytes)).Substring(0, 32)
    $guid = '{' + $hex.Substring(0,8) + '-' + $hex.Substring(8,4) + '-8' + $hex.Substring(13,3) + '-A' + $hex.Substring(17,3) + '-' + $hex.Substring(20,12) + '}'
    $component.SetAttribute('Guid', $guid)
    $component.RemoveAttribute('KeyPath')
    foreach ($file in @($component.SelectNodes('w:File', $namespaces))) {
        $file.SetAttribute('KeyPath', 'no')
    }
    # heat is invoked with -sreg: never reinterpret arbitrary registry imports.
    if ($component.SelectNodes('w:RegistryValue', $namespaces).Count -ne 0) {
        throw "Unexpected registry value in fresh harvest component $id."
    }
    $registry = $document.CreateElement('RegistryValue', $namespace)
    foreach ($entry in @{Root='HKCU'; Key='Software\XenTerm-Next\Flutter\InstallerComponents'; Name=$guid; Type='integer'; Value='1'; KeyPath='yes'}.GetEnumerator()) {
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
