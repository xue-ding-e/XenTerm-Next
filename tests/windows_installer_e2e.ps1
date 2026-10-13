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
function Run-Msi([string[]]$Arguments, [int[]]$AllowedExitCodes = @(0, 3010)) {
    $log = Join-Path $root ("msi-{0}.log" -f [guid]::NewGuid().ToString('N'))
    $process = Start-Process msiexec.exe -ArgumentList ($Arguments + @('/qn', '/norestart', '/l*v', "`"$log`"")) -Wait -PassThru
    $script:LastMsiExitCode = $process.ExitCode
    if ($process.ExitCode -notin $AllowedExitCodes) {
        Get-Content $log -Tail 120 | Write-Host
        throw "MSI failed with $($process.ExitCode); full log: $log"
    }
}
function Path-Entries { @(([Environment]::GetEnvironmentVariable('PATH', 'Machine') -split ';') | ForEach-Object { $_.TrimEnd('\') }) }
# WScript.Shell exposes legacy ANSI TargetPath on some Windows builds. Read the
# actual Shell link with the Unicode COM interface, as Explorer does.
# https://learn.microsoft.com/windows/win32/api/shobjidl_core/nn-shobjidl_core-ishelllinkw
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
public static class UnicodeShellLink {
    [DllImport("msi.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    public static extern int MsiQueryProductStateW(string productCode);
    [ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IShellLinkW {
        void GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int count, IntPtr findData, uint flags);
    }
    public static string Target(string file) {
        object link = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("00021401-0000-0000-C000-000000000046")));
        try {
            ((IPersistFile)link).Load(file, 0);
            var result = new StringBuilder(32768);
            ((IShellLinkW)link).GetPath(result, result.Capacity, IntPtr.Zero, 4);
            return result.ToString();
        } finally { Marshal.FinalReleaseComObject(link); }
    }
}
'@
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
            $actual = [UnicodeShellLink]::Target($link)
            if ($actual -ine (Join-Path $installDir 'xenterm.exe')) { throw "Wrong Unicode shortcut target: $actual" }
            if (-not (Test-Path -LiteralPath $actual -PathType Leaf)) { throw "Shortcut target does not exist: $actual" }
        }
    }
}
try {
    [Environment]::SetEnvironmentVariable('PATH', "$originalPath;$sentinel", 'Machine')
    foreach ($version in @('250.0.1', '250.0.2', '250.0.1')) {
        $productCode = [guid]::NewGuid().ToString('B')
        $product.SetAttribute('Id', $productCode)
        $xml.Save($source)
        # The third package deliberately has the first package's version and a
        # different ProductCode. Do not overwrite its original MSI fixture.
        $fixtureName = "$version-$($packages.Count)"
        $obj = Join-Path $root "$fixtureName.wixobj"
        $msi = Join-Path $root "$fixtureName.msi"
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
    # Observe the existing same-version release policy without enabling
    # AllowSameVersionUpgrades or changing the application's version numbers.
    # MSI state DEFAULT=5 means installed; UNKNOWN=-1 means unregistered.
    Run-Msi -Arguments @('/i', "`"$($packages[0])`"", "APPLICATIONFOLDER=`"$installDir`"")
    Assert-State $true $true $true
    if ([UnicodeShellLink]::MsiQueryProductStateW($products[0]) -ne 5) {
        throw 'First same-version product is not registered as installed.'
    }
    Run-Msi -Arguments @('/i', "`"$($packages[2])`"", "APPLICATIONFOLDER=`"$installDir`"") -AllowedExitCodes @(0, 3010, 1638)
    $secondExit = $script:LastMsiExitCode
    $firstState = [UnicodeShellLink]::MsiQueryProductStateW($products[0])
    $secondState = [UnicodeShellLink]::MsiQueryProductStateW($products[2])
    $observation = [ordered]@{
        version = '250.0.1'; firstProduct = $products[0]; secondProduct = $products[2]
        secondInstallExit = $secondExit; firstState = $firstState; secondState = $secondState
    }
    $observation | ConvertTo-Json | Tee-Object -FilePath (Join-Path $root 'msi-same-version-observation.log') | Write-Host
    if ($secondExit -eq 1638) {
        if ($firstState -ne 5 -or $secondState -ne -1) { throw 'Rejected package changed product registration.' }
        Write-Host 'SAME_VERSION: second ProductCode rejected with ERROR_PRODUCT_VERSION (1638).'
    } else {
        if ($firstState -ne 5 -or $secondState -ne 5) { throw 'Expected two installed products for the accepted same-version packages.' }
        Write-Host 'SAME_VERSION: both ProductCodes are installed; verifying shared-component removal safety.'
        Assert-State $true $true $true
        Run-Msi -Arguments @('/x', $products[2])
        if ([UnicodeShellLink]::MsiQueryProductStateW($products[2]) -ne -1) { throw 'Second product remained registered after uninstall.' }
    }
    # Removing/rejecting the second package must not break the first installation.
    if ([UnicodeShellLink]::MsiQueryProductStateW($products[0]) -ne 5) { throw 'First product registration was removed by the second package.' }
    if (-not (Test-Path -LiteralPath (Join-Path $installDir 'xenterm.exe') -PathType Leaf)) { throw 'Second same-version package removed the first executable.' }
    Assert-State $true $true $true
    $registeredDir = (Get-ItemProperty -LiteralPath "HKLM:\$registryKey" -Name InstallDir).InstallDir
    if ($registeredDir.TrimEnd('\') -ine $installDir.TrimEnd('\')) { throw 'First installation directory registration was changed.' }
    Run-Msi -Arguments @('/x', $products[0])
    Assert-State $false $false $false
    Write-Host 'PASS: same-version install/rejection and second-product removal preserve the first installation.'

    # A matching entry predating MSI belongs to its original owner, including
    # different casing/trailing slash. Empty segments must not stop the search.
    $preexisting = $installDir.ToUpperInvariant().TrimEnd('\') + '\'
    [Environment]::SetEnvironmentVariable('PATH', "$originalPath;$sentinel;;$preexisting", 'Machine')
    Run-Msi -Arguments @('/i', "`"$($packages[0])`"", "APPLICATIONFOLDER=`"$installDir`"")
    Assert-State $true $true $true
    # Mixed maintenance must not skip ownership detection because REMOVE is set.
    Run-Msi -Arguments @('/i', "`"$($packages[0])`"", 'REMOVE=DesktopIntegration', 'ADDLOCAL=CommandLineIntegration')
    Assert-State $true $false $true
    Run-Msi -Arguments @('/fa', $products[0])
    Assert-State $true $false $true
    Run-Msi -Arguments @('/i', "`"$($packages[1])`"")
    Assert-State $true $false $true
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
