# Real MSI regression, exclusively for a disposable elevated GitHub-hosted runner.
# The fixture uses unique product/component/registry/shortcut identities; it never
# installs a production XenTerm binary or closes an existing XenTerm process.
[CmdletBinding()]
param([string]$WixBin = "${env:ProgramFiles(x86)}\WiX Toolset v3.14\bin")
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'Run this destructive installer fixture only on a disposable GitHub-hosted runner.'
}
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'The disposable MSI fixture needs an elevated runner.'
}
$repo = Split-Path $PSScriptRoot -Parent
$id = [guid]::NewGuid().ToString('N')
$root = Join-Path $env:RUNNER_TEMP "xenterm-msi-$id"
New-Item -ItemType Directory $root | Out-Null
$installDir = Join-Path $root 'Custom 路径 with spaces'
$sentinel = Join-Path $root 'Unrelated PATH entry'
$shortcutName = "XenTerm Installer Test $id"
$registryKey = "Software\XenTermInstallerTest\$id"
$originalPath = [Environment]::GetEnvironmentVariable('PATH', 'Machine')
$candle = Join-Path $WixBin 'candle.exe'
$light = Join-Path $WixBin 'light.exe'
foreach ($tool in @($candle, $light)) { if (-not (Test-Path $tool)) { throw "Missing WiX 3 tool: $tool" } }
Copy-Item "$env:SystemRoot\System32\cmd.exe" (Join-Path $root 'xenterm.exe')
& (Join-Path $repo 'wix/build-custom-actions.ps1') -OutputDirectory $root
[xml]$xml = Get-Content (Join-Path $repo 'wix/main.wxs') -Raw
$ns = [Xml.XmlNamespaceManager]::new($xml.NameTable)
$ns.AddNamespace('w', 'http://schemas.microsoft.com/wix/2006/wi')
$ns.AddNamespace('u', 'http://schemas.microsoft.com/wix/UtilExtension')
$product = $xml.SelectSingleNode('//w:Product', $ns)
$product.SetAttribute('Name', $shortcutName)
$product.SetAttribute('UpgradeCode', [guid]::NewGuid().ToString('B'))
$xml.SelectSingleNode('//w:Property[@Id="XenTermInstallerRegistryKey"]', $ns).SetAttribute('Value', $registryKey)
$xml.SelectSingleNode('//w:Binary[@Id="PathOwnershipActions"]', $ns).SetAttribute('SourceFile', (Join-Path $root 'path_ownership.dll'))
foreach ($node in $xml.SelectNodes('//w:Component', $ns)) { $node.SetAttribute('Guid', [guid]::NewGuid().ToString('B')) }
foreach ($node in $xml.SelectNodes('//w:RegistryValue | //w:RegistrySearch', $ns)) { $node.SetAttribute('Key', $registryKey) }
foreach ($node in $xml.SelectNodes('//w:Shortcut', $ns)) { $node.SetAttribute('Name', $shortcutName) }
foreach ($node in @($xml.SelectNodes('//u:CloseApplication', $ns))) { [void]$node.ParentNode.RemoveChild($node) }
$xml.SelectSingleNode('//w:Icon', $ns).SetAttribute('SourceFile', (Join-Path $repo 'assets/xenterm.ico'))
$xml.SelectSingleNode('//w:File[@Id="thirdPartyNoticesFile"]', $ns).SetAttribute('Source', (Join-Path $repo 'THIRD_PARTY_NOTICES.md'))
$source = Join-Path $root 'fixture.wxs'
$xml.Save($source)
$packages = @()
$products = @()
function Run-Msi([string[]]$Arguments) {
    $log = Join-Path $root ("msi-{0}.log" -f [guid]::NewGuid().ToString('N'))
    $process = Start-Process msiexec.exe -ArgumentList ($Arguments + @('/qn', '/norestart', '/l*v', "`"$log`"")) -Wait -PassThru
    if ($process.ExitCode -notin @(0, 3010)) {
        Get-Content $log -Tail 120 | Write-Host
        throw "MSI failed with $($process.ExitCode); full log: $log"
    }
}
function Path-Entries { @(([Environment]::GetEnvironmentVariable('PATH', 'Machine') -split ';') | ForEach-Object { $_.TrimEnd('\') }) }
function Assert-State([bool]$Path, [bool]$Desktop, [bool]$Menu) {
    $entries = @(Path-Entries)
    $count = @($entries | Where-Object { $_ -ieq $installDir.TrimEnd('\') }).Count
    if ($count -ne [int]$Path) { throw "Expected PATH entry count $([int]$Path), got $count" }
    if ($entries -notcontains $sentinel) { throw 'Unrelated PATH entry was lost.' }
    $desktopLink = Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) "$shortcutName.lnk"
    $menuLink = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) "$shortcutName.lnk"
    if ((Test-Path $desktopLink) -ne $Desktop) { throw 'Desktop shortcut state differs from selection.' }
    if ((Test-Path $menuLink) -ne $Menu) { throw 'Start menu shortcut state differs from selection.' }
    foreach ($link in @($desktopLink, $menuLink)) {
        if (Test-Path $link) {
            $actual = (New-Object -ComObject WScript.Shell).CreateShortcut($link).TargetPath
            if ($actual -ine (Join-Path $installDir 'xenterm.exe')) { throw "Wrong shortcut target: $actual" }
        }
    }
}
try {
    [Environment]::SetEnvironmentVariable('PATH', "$originalPath;$sentinel", 'Machine')
    foreach ($version in @('250.0.1', '250.0.2')) {
        $productCode = [guid]::NewGuid().ToString('B')
        $product.SetAttribute('Id', $productCode)
        $xml.Save($source)
        $obj = Join-Path $root "$version.wixobj"
        $msi = Join-Path $root "$version.msi"
        & $candle -nologo -arch x64 -ext WixUtilExtension "-dVersion=$version" "-dCargoTargetBinDir=$root" -out $obj $source
        if ($LASTEXITCODE -ne 0) { throw 'WiX candle failed.' }
        & $light -nologo -ext WixUIExtension -ext WixUtilExtension -out $msi $obj
        if ($LASTEXITCODE -ne 0) { throw 'WiX light failed.' }
        $packages += $msi
        $products += $productCode
    }
    # First install: all three integrations default to selected.
    Run-Msi -Arguments @('/i', "`"$($packages[0])`"", "APPLICATIONFOLDER=`"$installDir`"")
    Assert-State $true $true $true
    # Repair must neither duplicate PATH nor reset selections.
    Run-Msi -Arguments @('/fa', $products[0])
    Assert-State $true $true $true
    # Change one feature using MSI's supported maintenance interface.
    Run-Msi -Arguments @('/i', "`"$($packages[0])`"", 'REMOVE=DesktopIntegration')
    Assert-State $true $false $true
    # Major upgrade must preserve both the custom directory and opt-out.
    Run-Msi -Arguments @('/i', "`"$($packages[1])`"")
    Assert-State $true $false $true
    Run-Msi -Arguments @('/x', $products[1])
    Assert-State $false $false $false
    # All integrations declined; upgrade and repair must not turn them on.
    Run-Msi -Arguments @('/i', "`"$($packages[0])`"", "APPLICATIONFOLDER=`"$installDir`"", 'ADDLOCAL=MainProgram')
    Assert-State $false $false $false
    Run-Msi -Arguments @('/i', "`"$($packages[1])`"")
    Assert-State $false $false $false
    Run-Msi -Arguments @('/fa', $products[1])
    Assert-State $false $false $false
    Run-Msi -Arguments @('/x', $products[1])
    Assert-State $false $false $false
    # A matching entry predating MSI belongs to its original owner, including
    # different casing/trailing slash. Empty segments must not stop the search.
    $preexisting = $installDir.ToUpperInvariant().TrimEnd('\') + '\'
    [Environment]::SetEnvironmentVariable('PATH', "$originalPath;$sentinel;;$preexisting", 'Machine')
    Run-Msi -Arguments @('/i', "`"$($packages[0])`"", "APPLICATIONFOLDER=`"$installDir`"")
    Assert-State $true $true $true
    Run-Msi -Arguments @('/fa', $products[0])
    Assert-State $true $true $true
    Run-Msi -Arguments @('/i', "`"$($packages[1])`"")
    Assert-State $true $true $true
    Run-Msi -Arguments @('/x', $products[1])
    Assert-State $true $false $false
    if (-not ([Environment]::GetEnvironmentVariable('PATH', 'Machine').Contains($preexisting))) {
        throw 'MSI changed the spelling of an externally owned PATH entry.'
    }
    Write-Host 'PASS: MSI defaults, opt-out, directory restoration, repair, upgrade, shortcuts and additive PATH cleanup.'
} finally {
    # Fixture-only recovery. Preserve logs for CI artifacts; never touch real XenTerm.
    foreach ($code in $products) {
        $process = Start-Process msiexec.exe -ArgumentList @('/x', $code, '/qn', '/norestart') -Wait -PassThru
        if ($process.ExitCode -notin @(0, 1605, 3010)) { Write-Warning "Fixture cleanup failed: $code ($($process.ExitCode))" }
    }
    [Environment]::SetEnvironmentVariable('PATH', $originalPath, 'Machine')
}
