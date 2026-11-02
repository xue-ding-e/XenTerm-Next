# Run only on a disposable hosted runner with real source-bound payload MSIs.
[CmdletBinding()]
param(
 [Parameter(Mandatory=$true)][string]$Msi,
 [Parameter(Mandatory=$true)][string]$PreviousMsi,
 [Parameter(Mandatory=$true)][string]$SameVersionMsi,
 [Parameter(Mandatory=$true)][string]$OutputDir
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
 throw 'GPUI lifecycle requires a disposable GitHub-hosted runner'
}
$Msi = (Resolve-Path -LiteralPath $Msi).Path
$PreviousMsi = (Resolve-Path -LiteralPath $PreviousMsi).Path
$SameVersionMsi = (Resolve-Path -LiteralPath $SameVersionMsi).Path
New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
$OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path
# WScript.Shell exposes legacy ANSI TargetPath on some Windows builds. Read the
# actual Shell link with the Unicode COM interface, as Explorer does.
# https://learn.microsoft.com/windows/win32/api/shobjidl_core/nn-shobjidl_core-ishelllinkw
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
public static class GPUIUnicodeShellLink {
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

$installer = New-Object -ComObject WindowsInstaller.Installer
function Msi-Property([string]$Path, [string]$Name) {
    $db = $installer.OpenDatabase($Path, 0)
    $view = $db.OpenView("SELECT ``Value`` FROM ``Property`` WHERE ``Property``='$Name'")
    $view.Execute()
    $record = $view.Fetch()
    if ($null -eq $record) { return '' }
    $value = $record.StringData(1)
    $view.Close()
    return $value
}
$newCode = Msi-Property $Msi 'ProductCode'
$oldCode = Msi-Property $PreviousMsi 'ProductCode'
$defaultInstall = Join-Path $env:LOCALAPPDATA 'Programs\XenTerm-Next GPUI'
$customInstall = Join-Path $env:LOCALAPPDATA 'GPUI 测试 路径\XenTerm-Next GPUI'
$menu = Join-Path ([Environment]::GetFolderPath('Programs')) 'XenTerm-Next GPUI\XenTerm-Next GPUI.lnk'
$desktop = Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) 'XenTerm-Next GPUI.lnk'
$registry = 'HKCU:\Software\XenTerm-Next\GPUI\Installer'
$receipt = Get-Content -LiteralPath "$Msi.json" -Raw | ConvertFrom-Json
$previousReceipt = Get-Content -LiteralPath "$PreviousMsi.json" -Raw | ConvertFrom-Json
$events = [Collections.Generic.List[object]]::new()
$startedGuiIds = [Collections.Generic.List[int]]::new()
$started = $false
$originalUserPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
$originalMachinePath = [Environment]::GetEnvironmentVariable('PATH', 'Machine')
function Run-Msi([string[]]$Arguments, [string]$Label, [int[]]$Allowed = @(0,3010)) {
 $log = Join-Path $OutputDir "$Label.log"
 $p = Start-Process msiexec.exe -ArgumentList ($Arguments + @('/qn','/norestart','/l*v',"`"$log`"")) -Wait -PassThru
 $events.Add(@{operation=$Label; exit_code=$p.ExitCode})
 if ($p.ExitCode -notin $Allowed) { Get-Content $log -Tail 80 | Write-Host; throw "$Label failed: $($p.ExitCode)" }
}
function Assert-Profile([string]$Directory) {
 $sentinel = Join-Path $Directory 'config\installer-preserve-test.json'
 if ((Get-Content -LiteralPath $sentinel -Raw).Trim() -ne '{"fixture":"preserve"}') { throw 'GPUI profile changed' }
 if ([Environment]::GetEnvironmentVariable('PATH','User') -cne $originalUserPath -or
     [Environment]::GetEnvironmentVariable('PATH','Machine') -cne $originalMachinePath) { throw 'PATH changed' }
}
function Assert-Installed([string]$Directory, [string]$Code, [bool]$Launch=$false) {
 if ($installer.ProductState($Code) -ne 5) { throw 'Expected installed product' }
 $arp = Get-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$Code"
 if ($arp.DisplayName -ne 'XenTerm-Next GPUI') { throw 'Wrong ARP identity' }
 if ((Get-ItemProperty $registry).InstallDir.TrimEnd('\') -ine $Directory.TrimEnd('\')) { throw 'Custom install path lost' }
 $installedReceipt = if ($Code -eq $oldCode) { $previousReceipt } else { $receipt }
 foreach ($file in $installedReceipt.payload) {
   $path = Join-Path $Directory $file.path
   if (!(Test-Path -LiteralPath $path) -or (Get-FileHash -LiteralPath $path).Hash -ine $file.sha256) { throw "Missing/changed installed payload: $($file.path)" }
 }
 foreach ($link in @($menu,$desktop)) {
   if (!(Test-Path -LiteralPath $link) -or [GPUIUnicodeShellLink]::Target($link) -ine (Join-Path $Directory 'xenterm.exe')) { throw 'Wrong Unicode shortcut target' }
 }
 if ($Launch) {
   Start-Process $menu
   $gui = $null
   for ($n=0; $n -lt 30; $n++) {
     Start-Sleep -Seconds 1
     $gui = Get-Process xenterm -ErrorAction SilentlyContinue | Where-Object { $_.Path -ieq (Join-Path $Directory 'xenterm.exe') } | Select-Object -First 1
     if ($gui -and $gui.MainWindowHandle -ne 0) { break }
   }
   if ($gui) { $startedGuiIds.Add($gui.Id) }
   if (!$gui -or $gui.MainWindowHandle -eq 0) { throw 'Installed GPUI shortcut did not open a live window' }
   Start-Sleep -Seconds 5
   $gui.Refresh()
   if ($gui.HasExited) { throw 'GPUI crashed during startup' }
   $events.Add(@{operation='shortcut-window'; directory=$Directory; process_id=$gui.Id; title=$gui.MainWindowTitle})
   Add-Type -AssemblyName System.Windows.Forms
   Add-Type -AssemblyName System.Drawing
   $bounds = [Windows.Forms.Screen]::PrimaryScreen.Bounds
   $image = [Drawing.Bitmap]::new($bounds.Width,$bounds.Height)
   $graphics = [Drawing.Graphics]::FromImage($image)
   $graphics.CopyFromScreen($bounds.Location,[Drawing.Point]::Empty,$bounds.Size)
   $image.Save((Join-Path $OutputDir "gpui-window-$($events.Count).png"))
   $graphics.Dispose(); $image.Dispose()
   [void]$gui.CloseMainWindow()
   if (!$gui.WaitForExit(10000)) { $gui.Kill(); $gui.WaitForExit() }
 }
 Assert-Profile $Directory
}
function Snapshot-Profile([string]$Directory) {
 $script:snapshot = @{}
 Get-ChildItem -LiteralPath (Join-Path $Directory 'config') -Recurse -File | ForEach-Object {
  $script:snapshot[$_.FullName] = (Get-FileHash -LiteralPath $_.FullName).Hash
 }
}
function Assert-Removed([string]$Directory, [string]$Code) {
 if ($installer.ProductState($Code) -ne -1) { throw 'Product still registered' }
 foreach ($file in $receipt.payload) {
   if (Test-Path -LiteralPath (Join-Path $Directory $file.path)) { throw "Installer-owned payload remains: $($file.path)" }
 }
 foreach ($link in @($menu,$desktop)) { if (Test-Path -LiteralPath $link) { throw 'Shortcut remains' } }
 foreach ($path in $script:snapshot.Keys) {
  if (!(Test-Path -LiteralPath $path) -or (Get-FileHash -LiteralPath $path).Hash -ne $script:snapshot[$path]) { throw "Actual GPUI profile changed: $path" }
 }
 Assert-Profile $Directory
}
try {
 foreach ($path in @($defaultInstall,$customInstall,$menu,$desktop)) {
  if (Test-Path -LiteralPath $path) { throw "Refusing preexisting installation: $path" }
 }
 foreach ($msiPath in @($Msi,$PreviousMsi)) {
  if ((Msi-Property $msiPath 'UpgradeCode') -ne '{7548A7CB-F70B-4AE7-A5F0-F952D45AA826}' -or
      (Msi-Property $msiPath 'ALLUSERS') -ne '') { throw 'Wrong independent per-user identity' }
 }
 if ($newCode -eq $oldCode) { throw 'Upgrade fixture must have distinct ProductCode' }
 if ((Msi-Property $SameVersionMsi 'ProductCode') -eq $newCode -or
     (Msi-Property $SameVersionMsi 'ProductVersion') -ne (Msi-Property $Msi 'ProductVersion')) { throw 'Invalid equal-build collision fixture' }
 $started = $true
 foreach ($directory in @($defaultInstall,$customInstall)) {
  $label = if ($directory -eq $defaultInstall) { 'default' } else { 'unicode' }
  $arguments = @('/i',"`"$PreviousMsi`"")
  if ($label -eq 'unicode') { $arguments += "INSTALLFOLDER=`"$directory`"" }
  Run-Msi $arguments "install-$label"
  # The real legacy GPUI uses a writable config directory beside its EXE.
  # This is not part of the harvested package and must survive MSI removal.
  New-Item -ItemType Directory -Path (Join-Path $directory 'config') -Force | Out-Null
  '{"fixture":"preserve"}' | Set-Content -LiteralPath (Join-Path $directory 'config\installer-preserve-test.json') -Encoding utf8NoBOM
  Assert-Installed $directory $oldCode $true
  Remove-Item -LiteralPath (Join-Path $directory 'LICENSE')
  Run-Msi @('/fa',$oldCode) "repair-$label"
  Assert-Installed $directory $oldCode
  Snapshot-Profile $directory
  Run-Msi @('/i',"`"$Msi`"") "upgrade-$label"
  Assert-Installed $directory $newCode $true
  if ($installer.ProductState($oldCode) -ne -1) { throw 'Old product remains after major upgrade' }
  Run-Msi @('/i',"`"$PreviousMsi`"") "reject-downgrade-$label" @(1603)
  $downgradeLog = Get-Content -LiteralPath (Join-Path $OutputDir "reject-downgrade-$label.log") -Raw
  if ($downgradeLog -notmatch 'A newer XenTerm-Next GPUI is already installed|This or a newer GPUI build is installed') { throw 'Downgrade did not fail at the version gate' }
  Assert-Installed $directory $newCode
  Run-Msi @('/i',"`"$SameVersionMsi`"") "reject-same-build-$label" @(1603)
  $sameLog = Get-Content -LiteralPath (Join-Path $OutputDir "reject-same-build-$label.log") -Raw
  if ($sameLog -notmatch 'This or a newer GPUI build is installed') { throw 'Equal build did not fail at the version gate' }
  Run-Msi @('/fa',$newCode) "repair-current-$label"
  Assert-Installed $directory $newCode
  Snapshot-Profile $directory
  Run-Msi @('/x',$newCode) "uninstall-$label"
  Assert-Removed $directory $newCode
 }
 Write-Host 'PASS: real GPUI full payload, repair, major upgrade, downgrade rejection, shortcuts, GUI startup, Unicode path restoration and profile preservation.'
} finally {
 $events | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDir 'gpui-lifecycle.json') -Encoding utf8NoBOM
 foreach ($id in $startedGuiIds) {
  $p = Get-Process -Id $id -ErrorAction SilentlyContinue
  if ($p -and $p.Path -in @((Join-Path $defaultInstall 'xenterm.exe'),(Join-Path $customInstall 'xenterm.exe'))) {
   [void]$p.CloseMainWindow(); if (!$p.WaitForExit(10000)) { $p.Kill(); $p.WaitForExit() }
  }
 }
 foreach ($code in @($newCode,$oldCode)) {
  if ($started -and $installer.ProductState($code) -eq 5) { Run-Msi @('/x',$code) "cleanup-$($code.Trim('{}'))" }
 }
}
