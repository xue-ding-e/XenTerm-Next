# Registration identity and ARP metadata are separate gates; neither proves visual UI acceptance.
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class GPUILifecycleRegistrationApi {
 [DllImport("msi.dll",CharSet=CharSet.Unicode,ExactSpelling=true)]
 public static extern uint MsiEnumRelatedProductsW(string upgradeCode,uint reserved,uint index,StringBuilder productCode);
 [DllImport("msi.dll",CharSet=CharSet.Unicode,ExactSpelling=true)]
 private static extern uint MsiEnumProductsExW(string code,string sid,uint context,uint index,StringBuilder foundCode,out uint foundContext,StringBuilder foundSid,ref uint sidLength);
 [DllImport("msi.dll",CharSet=CharSet.Unicode,ExactSpelling=true)]
 private static extern uint MsiGetProductInfoExW(string code,string sid,uint context,string property,StringBuilder value,ref uint length);
 public static string SidForContext(uint context,string sid) {
  if(context==4) return null; // Native NULL must be selected inside C#, not PowerShell's string binder.
  if((context==1 || context==2 || context==3) && !String.IsNullOrEmpty(sid)) return sid;
  throw new ArgumentException("Unsupported context or missing user SID");
 }
 public static uint Enumerate(string code,string sid,uint context,uint index,StringBuilder foundCode,out uint foundContext,StringBuilder foundSid,ref uint sidLength) {
  return MsiEnumProductsExW(code,SidForContext(context,sid),context,index,foundCode,out foundContext,foundSid,ref sidLength);
 }
 public static uint Property(string code,string sid,uint context,string property,StringBuilder value,ref uint length) {
  return MsiGetProductInfoExW(code,SidForContext(context,sid),context,property,value,ref length);
 }
}
'@
function Assert-EmptyFamily($ComCount, [object[]]$ComItems, $NativeStatus, [string]$NativeCode) {
 # Unknown counts/statuses and disagreement fail closed, just like a real family member.
 if($ComCount -isnot [int] -or $ComCount -lt 0 -or $null -eq $ComItems -or $NativeStatus -isnot [uint32]) { throw 'Unknown upgrade-family evidence' }
 if($ComCount -ne $ComItems.Count) { throw 'Inconsistent COM upgrade-family enumeration' }
 if($ComCount -ne 0 -or $NativeStatus -ne 259 -or $NativeCode -cne '') { throw 'Refusing existing, inconsistent or unknown GPUI upgrade-family state' }
}
function Get-GpuiRegistration([string]$Code) {
 $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
 $instances=@();$arp=@();$trace=@()
 foreach($scope in @(3,4)) {
  $done=$false
  for([uint32]$i=0;$i -lt 4;$i++) {
   $found=[Text.StringBuilder]::new(39);$user=[Text.StringBuilder]::new(256);[uint32]$size=256;[uint32]$context=0
   $rc=[GPUILifecycleRegistrationApi]::Enumerate($Code,$sid,$scope,$i,$found,[ref]$context,$user,[ref]$size)
   $trace+=@{scope=$scope;index=$i;status=$rc}
   if($rc -eq 259 -or $rc -eq 1605){$done=$true;break}
   if($rc -ne 0){throw "Registration enumeration failed: scope=$scope index=$i status=$rc"}
   if($found.ToString() -ine $Code){throw 'Unexpected product enumeration'}
   $properties=@{}
   foreach($name in @('InstalledProductName','VersionString','InstallLocation','AssignmentType','State')) {
    $value=[Text.StringBuilder]::new(4096);[uint32]$length=4096
    $status=[GPUILifecycleRegistrationApi]::Property($Code,$user.ToString(),$context,$name,$value,[ref]$length)
    $properties[$name]=@{status=$status;value=$value.ToString()}
   }
   $instances+=@{code=$found.ToString();context=$context;sid=$user.ToString();properties=$properties}
  }
  if(-not $done){throw 'Exact-product enumeration exceeded bound'}
 }
 foreach($hive in @([Microsoft.Win32.RegistryHive]::CurrentUser,[Microsoft.Win32.RegistryHive]::LocalMachine)) {
  foreach($view in @([Microsoft.Win32.RegistryView]::Registry32,[Microsoft.Win32.RegistryView]::Registry64)) {
   $base=[Microsoft.Win32.RegistryKey]::OpenBaseKey($hive,$view);$key=$null
   try {
    $key=$base.OpenSubKey("Software\Microsoft\Windows\CurrentVersion\Uninstall\$Code",$false)
    if($null -ne $key) {
     $values=@{}
     foreach($name in @('DisplayName','DisplayVersion','InstallLocation','UninstallString','WindowsInstaller','SystemComponent','NoRemove')){$values[$name]=$key.GetValue($name,$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)}
     $arp+=@{hive=[string]$hive;view=[string]$view;values=$values}
    }
   } finally {if($null -ne $key){$key.Dispose()};$base.Dispose()}
  }
 }
 return @{instances=$instances;arp=$arp;trace=$trace;current_sid=$sid;visual_installed_apps_tested=$false}
}
function Assert-GpuiRegistration($Evidence,[string]$Code,[string]$Directory,[string]$Version,[string]$Sid) {
 if($Evidence.instances.Count -ne 1){throw 'Expected one exact per-user product instance'}
 $instance=$Evidence.instances[0]
 if($instance.code -ine $Code -or $instance.sid -cne $Sid -or $instance.context -ne 2){throw 'Wrong installed product user/context'}
 $p=$instance.properties
 foreach($name in @('InstalledProductName','VersionString','InstallLocation','AssignmentType','State')) {if($p[$name].status -ne 0){throw "Product property unavailable: $name"}}
 if($p.InstalledProductName.value -cne 'XenTerm-Next GPUI' -or $p.VersionString.value -cne $Version -or $p.AssignmentType.value -cne '0' -or $p.State.value -cne '5' -or $p.InstallLocation.value.TrimEnd('\') -ine $Directory.TrimEnd('\')) {throw 'Wrong per-user API identity/version/path/state'}
 if($Evidence.arp.Count -lt 1){throw 'Missing installed-app uninstall registration'}
 foreach($entry in $Evidence.arp) {
  $v=$entry.values
  if($entry.hive -notin @('CurrentUser','LocalMachine') -or $entry.view -notin @('Registry32','Registry64')){throw 'Unexpected uninstall registry view'}
  if($v.DisplayName -cne 'XenTerm-Next GPUI' -or $v.DisplayVersion -cne $Version -or $v.InstallLocation.TrimEnd('\') -ine $Directory.TrimEnd('\') -or $v.WindowsInstaller -ne 1){throw 'Wrong installed-app display metadata'}
  if(($null -ne $v.SystemComponent -and $v.SystemComponent -ne 0) -or ($null -ne $v.NoRemove -and $v.NoRemove -ne 0)){throw 'Installed app hidden or removal disabled'}
  if($v.UninstallString -ine "MsiExec.exe /X$Code"){throw 'Uninstall entry must target only this exact product'}
 }
}
function Assert-NoGpuiRegistration([string]$Code) {
 $e=Get-GpuiRegistration $Code
 if($e.instances.Count -ne 0 -or $e.arp.Count -ne 0){throw 'Product registration/uninstall entry remains'}
}
function Assert-EmptyGpuiFamily {
 $installer=$null;$related=$null
 try {
  $installer=New-Object -ComObject WindowsInstaller.Installer
  $related=$installer.RelatedProducts('{7548A7CB-F70B-4AE7-A5F0-F952D45AA826}')
  $count=$related.GetType().InvokeMember('Count',[Reflection.BindingFlags]::GetProperty,$null,$related,$null)
  $items=@();foreach($item in $related){if($items.Count -ge 100){throw 'Family enumeration exceeded bound'};$items+=,$item}
  $code=[Text.StringBuilder]::new(39)
  [uint32]$status=[GPUILifecycleRegistrationApi]::MsiEnumRelatedProductsW('{7548A7CB-F70B-4AE7-A5F0-F952D45AA826}',0,0,$code)
  Assert-EmptyFamily $count $items $status $code.ToString()
 } finally {foreach($com in @($related,$installer)){if($null -ne $com -and [Runtime.InteropServices.Marshal]::IsComObject($com)){[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($com)}}}
}
