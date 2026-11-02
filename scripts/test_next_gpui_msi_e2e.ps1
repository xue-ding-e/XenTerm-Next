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
    $db = $null; $view = $null; $record = $null
    try {
        $db = $installer.OpenDatabase($Path, 0)
        $view = $db.OpenView("SELECT ``Value`` FROM ``Property`` WHERE ``Property``='$Name'")
        # COM void methods can emit null objects into PowerShell's success stream.
        # The caller must receive only one property string, never a mixed array.
        [void]$view.Execute()
        $record = $view.Fetch()
        if ($null -eq $record) { return '' }
        return $record.StringData(1)
    } finally {
        try { if ($null -ne $view) { [void]$view.Close() } }
        finally {
            foreach ($com in @($record, $view, $db)) {
                if ($null -ne $com -and [Runtime.InteropServices.Marshal]::IsComObject($com)) {
                    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($com)
                }
            }
        }
    }
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
. (Join-Path $PSScriptRoot 'gpui_msi_coexistence_sentinels.ps1')
. (Join-Path $PSScriptRoot 'gpui_msi_registration_evidence.ps1')
. (Join-Path $PSScriptRoot 'initialize_gpui_test_profile.ps1')
. (Join-Path $PSScriptRoot 'gpui_msi_gui_observation.ps1')
$profileEvidence = Join-Path $PSScriptRoot 'gpui_msi_profile_evidence.py'
function Profile-Evidence([string[]]$Arguments) {
 & python $profileEvidence @Arguments
 if ($LASTEXITCODE -ne 0) { throw 'GPUI profile evidence failed' }
}
$originalUserPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
$originalMachinePath = [Environment]::GetEnvironmentVariable('PATH', 'Machine')
function Run-Msi([string[]]$Arguments, [string]$Label, [int[]]$Allowed = @(0,3010)) {
 $log = Join-Path $OutputDir "$Label.log"
 $p = Start-Process msiexec.exe -ArgumentList ($Arguments + @('/qn','/norestart','/l*v',"`"$log`"")) -Wait -PassThru
 $events.Add(@{operation=$Label; exit_code=$p.ExitCode})
 Assert-ProtectedProductSentinels $Label
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
 $registration = Get-GpuiRegistration $Code
 $version = Msi-Property $(if($Code -eq $oldCode){$PreviousMsi}else{$Msi}) 'ProductVersion'
 $registrationPath = Join-Path $OutputDir "registration-$($events.Count)-$($Code.Trim('{}'))-$([guid]::NewGuid()).json"
 $registrationBytes = [Text.UTF8Encoding]::new($false).GetBytes(($registration | ConvertTo-Json -Depth 12))
 $registrationStream = [IO.File]::Open($registrationPath,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
 try {$registrationStream.Write($registrationBytes,0,$registrationBytes.Length)} finally {$registrationStream.Dispose()}
 Assert-GpuiRegistration $registration $Code $Directory $version ([Security.Principal.WindowsIdentity]::GetCurrent().User.Value)
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
   $guiEvidence=@{directory=$Directory;product_code=$Code;shortcut=$menu;launch_command='Start-Process $menu -PassThru';samples=@();diagnostic_errors=@();stdout_stderr_capture='Unavailable for Shell shortcut activation; no direct launch or redirected substitute';acceptance_changed=$false}
   $held=$null;$gui=$null;$primaryGuiError=$null
   $guiEvidencePath=Join-Path $OutputDir "gui-observation-$([guid]::NewGuid()).json"
   try {
   try {
     $dataDirItem=Get-Item -LiteralPath Env:XENTERM_DATA_DIR -ErrorAction SilentlyContinue
     $guiEvidence.profile_environment=@{variable='XENTERM_DATA_DIR';present=($null -ne $dataDirItem);length=if($null -ne $dataDirItem){$dataDirItem.Value.Length}else{$null}}
     Add-Type -AssemblyName System.Windows.Forms
     $guiEvidence.environment=@{interactive=[Environment]::UserInteractive;session_id=(Get-Process -Id $PID).SessionId;session_name=$env:SESSIONNAME;os=[Environment]::OSVersion.VersionString;screen=[Windows.Forms.Screen]::PrimaryScreen.Bounds.ToString()}
     $shell=$null;$shortcut=$null
     try {$shell=New-Object -ComObject WScript.Shell;$shortcut=$shell.CreateShortcut($menu);$guiEvidence.shortcut_metadata=@{target=$shortcut.TargetPath;arguments=$shortcut.Arguments;working_directory=$shortcut.WorkingDirectory}}
     finally {foreach($com in @($shortcut,$shell)){if($null -ne $com){[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($com)}}}
   } catch {$guiEvidence.diagnostic_errors+=@{stage='environment-or-shortcut';message=$_.Exception.Message}}
   $held=Start-Process $menu -PassThru
   $gui = $null
   for ($n=0; $n -lt 30; $n++) {
     Start-Sleep -Seconds 1
     $gui = Get-Process xenterm -ErrorAction SilentlyContinue | Where-Object { $_.Path -ieq (Join-Path $Directory 'xenterm.exe') } | Select-Object -First 1
     # Observe the original selected object first, then Refresh only our held
     # launch object. Neither sample supplies or changes the original decision.
     try {$guiEvidence.samples+=@{second=($n+1);utc=[DateTime]::UtcNow.ToString('o');original=(Get-GpuiProcessObservation $gui);held=(Get-GpuiProcessObservation $held $true)}}
     catch {$guiEvidence.diagnostic_errors+=@{stage='sample';message=$_.Exception.Message}}
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
   } catch {$primaryGuiError=$_;$guiEvidence.primary_error=$_.Exception.Message;throw}
   finally {
     try {$guiEvidence.final_original=Get-GpuiProcessObservation $gui;$guiEvidence.final_held=Get-GpuiProcessObservation $held $true}
     catch {$guiEvidence.diagnostic_errors+=@{stage='final-process';message=$_.Exception.Message}}
     try {Save-GpuiGuiObservation $guiEvidence $Directory $guiEvidencePath}
     catch {if($null -ne $primaryGuiError){Write-Warning "GUI evidence write failed; original error retained: $($_.Exception.Message)"}else{throw}}
     finally {if($null -ne $held){try{$held.Dispose()}catch{Write-Warning "Diagnostic process handle dispose failed: $($_.Exception.Message)"}}}
   }
 }
 Assert-Profile $Directory
}
function Assert-Removed([string]$Directory, [string]$Code, [string]$Baseline) {
 if ($installer.ProductState($Code) -ne -1) { throw 'Product still registered' }
 Assert-NoGpuiRegistration $Code
 foreach ($file in $receipt.payload) {
   if (Test-Path -LiteralPath (Join-Path $Directory $file.path)) { throw "Installer-owned payload remains: $($file.path)" }
 }
 foreach ($link in @($menu,$desktop)) { if (Test-Path -LiteralPath $link) { throw 'Shortcut remains' } }
 Profile-Evidence @('verify','--profile',(Join-Path $Directory 'config'),'--baseline',$Baseline)
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
 Assert-EmptyGpuiFamily
 Assert-NoGpuiRegistration $oldCode
 Assert-NoGpuiRegistration $newCode
 foreach($view in @([Microsoft.Win32.RegistryView]::Registry32,[Microsoft.Win32.RegistryView]::Registry64)) {
  $base=[Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::CurrentUser,$view);$anchor=$null
  try {$anchor=$base.OpenSubKey('Software\XenTerm-Next\GPUI\Installer',$false);if($null -ne $anchor){throw 'Refusing preexisting GPUI installer anchor'}}
  finally {if($null -ne $anchor){$anchor.Dispose()};$base.Dispose()}
 }
 New-ProtectedProductSentinels $OutputDir
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
  $profile = Join-Path $directory 'config'
  Profile-Evidence @('assert-empty-or-missing','--profile',$profile)
  Initialize-GpuiTestProfile -Exe (Join-Path $directory 'xenterm.exe') -Profile $profile -ExpectedVersion $previousReceipt.runtime_version -Output (Join-Path $OutputDir "$label-native-initialization.json")
  Profile-Evidence @('assert-native-initialized','--profile',$profile)
  $fixtureId = [guid]::NewGuid().ToString()
  Profile-Evidence @('seed','--profile',$profile,'--fixture-id',$fixtureId)
  $semanticBaseline = Join-Path $OutputDir "$label-seeded-semantics.json"
  Profile-Evidence @('semantic','--profile',$profile,'--fixture-id',$fixtureId,'--output',$semanticBaseline)
  Assert-Installed $directory $oldCode $true
  Profile-Evidence @('verify-semantic','--profile',$profile,'--fixture-id',$fixtureId,'--baseline',$semanticBaseline,'--output',(Join-Path $OutputDir "$label-old-gui-semantics.json"))
  # Freeze after the real old GUI closes. Never overwrite this byte baseline.
  $upgradeBaseline = Join-Path $OutputDir "$label-before-upgrade.json"
  Profile-Evidence @('capture','--profile',$profile,'--output',$upgradeBaseline)
  Remove-Item -LiteralPath (Join-Path $directory 'LICENSE')
  Run-Msi @('/fa',$oldCode) "repair-$label"
  Assert-Installed $directory $oldCode
  Profile-Evidence @('verify','--profile',$profile,'--baseline',$upgradeBaseline)
  Run-Msi @('/i',"`"$Msi`"") "upgrade-$label"
  Profile-Evidence @('verify','--profile',$profile,'--baseline',$upgradeBaseline)
  Assert-Installed $directory $newCode
  if ($installer.ProductState($oldCode) -ne -1) { throw 'Old product remains after major upgrade' }
  Run-Msi @('/i',"`"$PreviousMsi`"") "reject-downgrade-$label" @(1603)
  $downgradeLog = Get-Content -LiteralPath (Join-Path $OutputDir "reject-downgrade-$label.log") -Raw
  if ($downgradeLog -notmatch 'A newer XenTerm-Next GPUI is already installed|This or a newer GPUI build is installed') { throw 'Downgrade did not fail at the version gate' }
  Assert-Installed $directory $newCode
  Profile-Evidence @('verify','--profile',$profile,'--baseline',$upgradeBaseline)
  Run-Msi @('/i',"`"$SameVersionMsi`"") "reject-same-build-$label" @(1603)
  $sameLog = Get-Content -LiteralPath (Join-Path $OutputDir "reject-same-build-$label.log") -Raw
  if ($sameLog -notmatch 'This or a newer GPUI build is installed') { throw 'Equal build did not fail at the version gate' }
  Profile-Evidence @('verify','--profile',$profile,'--baseline',$upgradeBaseline)
  Run-Msi @('/fa',$newCode) "repair-current-$label"
  Assert-Installed $directory $newCode
  Profile-Evidence @('verify','--profile',$profile,'--baseline',$upgradeBaseline)
  # GUI schema writes are evaluated semantically, separately from MSI bytes.
  Assert-Installed $directory $newCode $true
  Profile-Evidence @('verify-semantic','--profile',$profile,'--fixture-id',$fixtureId,'--baseline',$semanticBaseline,'--output',(Join-Path $OutputDir "$label-new-gui-semantics.json"))
  $uninstallBaseline = Join-Path $OutputDir "$label-before-uninstall.json"
  Profile-Evidence @('capture','--profile',$profile,'--output',$uninstallBaseline)
  Run-Msi @('/x',$newCode) "uninstall-$label"
  Assert-Removed $directory $newCode $uninstallBaseline
 }
 Write-Host 'PASS: synthetic coexistence protection plus real GPUI full payload, repair, major upgrade, downgrade rejection, shortcuts, GUI startup, Unicode path restoration and profile preservation.'
} finally {
 $events | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDir 'gpui-lifecycle.json') -Encoding utf8NoBOM
 foreach ($id in $startedGuiIds) {
  $p = Get-Process -Id $id -ErrorAction SilentlyContinue
  if ($p -and $p.Path -in @((Join-Path $defaultInstall 'xenterm.exe'),(Join-Path $customInstall 'xenterm.exe'))) {
   [void]$p.CloseMainWindow(); if (!$p.WaitForExit(10000)) { $p.Kill(); $p.WaitForExit() }
  }
 }
 try {
 foreach ($code in @($newCode,$oldCode)) {
  if ($started -and $installer.ProductState($code) -eq 5) { Run-Msi @('/x',$code) "cleanup-$($code.Trim('{}'))" }
 }
 } finally { Remove-OwnedProductSentinels }
}
