param([Parameter(Mandatory=$true)][string]$Script)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $Script).Path,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Registration helper parse errors'}
$functions=@($ast.FindAll({param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Assert-GpuiRegistration'},$true))
if($functions.Count -ne 1){throw 'Expected one registration assertion'}
. ([scriptblock]::Create($functions[0].Extent.Text))
$code='{5FE800A9-0163-42ED-822A-34D78D542EEC}';$dir='C:\GPUI 测试';$version='106.227.41616';$sid='S-1-5-21-1-2-3-500'
function Fixture {
 $properties=@{};foreach($pair in @(@('InstalledProductName','XenTerm-Next GPUI'),@('VersionString',$version),@('InstallLocation',"$dir\"),@('AssignmentType','0'),@('State','5'))){$properties[$pair[0]]=@{status=0;value=$pair[1]}}
 return @{instances=@(@{code=$code;context=2;sid=$sid;properties=$properties});arp=@(@{hive='LocalMachine';view='Registry64';values=@{DisplayName='XenTerm-Next GPUI';DisplayVersion=$version;InstallLocation="$dir\";WindowsInstaller=1;UninstallString="MsiExec.exe /X$code";SystemComponent=$null;NoRemove=$null}})}
}
Assert-GpuiRegistration (Fixture) $code $dir $version $sid
$e=Fixture;$e.arp[0].hive='CurrentUser';Assert-GpuiRegistration $e $code $dir $version $sid
$e=Fixture;$e.arp[0].values.SystemComponent=0;$e.arp[0].values.NoRemove=0;Assert-GpuiRegistration $e $code $dir $version $sid
$cases=@(
 {param($e) $e.instances=@()},
 {param($e) $e.instances+= $e.instances[0]},
 {param($e) $e.instances[0].context=4},
 {param($e) $e.instances[0].sid='other-user'},
 {param($e) $e.instances[0].code='{00000000-0000-0000-0000-000000000000}'},
 {param($e) $e.instances[0].properties.AssignmentType.value='1'},
 {param($e) $e.instances[0].properties.State.value='1'},
 {param($e) $e.instances[0].properties.VersionString.value='wrong'},
 {param($e) $e.instances[0].properties.InstallLocation.value='C:\wrong'},
 {param($e) $e.instances[0].properties.InstalledProductName.value='upstream'},
 {param($e) $e.instances[0].properties.State.status=87},
 {param($e) $e.arp=@()},
 {param($e) $e.arp[0].values.DisplayName='upstream'},
 {param($e) $e.arp[0].values.DisplayVersion='wrong'},
 {param($e) $e.arp[0].values.InstallLocation='C:\wrong'},
 {param($e) $e.arp[0].values.WindowsInstaller=0},
 {param($e) $e.arp[0].values.SystemComponent=1},
 {param($e) $e.arp[0].values.NoRemove=1},
 {param($e) $e.arp[0].values.UninstallString='MsiExec.exe /X{00000000-0000-0000-0000-000000000000}'},
 {param($e) $e.arp[0].values.UninstallString="MsiExec.exe /X$code & evil"},
 {param($e) $e.arp[0].values.UninstallString=$null},
 {param($e) $e.arp[0].hive='unknown'},
 {param($e) $e.arp[0].view='unknown'}
)
foreach($case in $cases){$e=Fixture;& $case $e;$rejected=$false;try{Assert-GpuiRegistration $e $code $dir $version $sid}catch{$rejected=$true};if(-not $rejected){throw 'Invalid API or ARP evidence accepted'}}
Write-Output "Registration contracts passed: 3 positive + $($cases.Count) negative"

# Preserve independently-tested family protection regressions.
& { param($Script)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $Script).Path,[ref]$tokens,[ref]$errors)
if($errors.Count -ne 0){throw 'Probe parse errors'}
$functions=@($ast.FindAll({param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Assert-EmptyFamily'},$true))
if($functions.Count -ne 1){throw 'Expected exactly one family guard'}
. ([scriptblock]::Create($functions[0].Extent.Text))
$passed=0
function Reject([scriptblock]$case){$rejected=$false;try{& $case}catch{$rejected=$true};if(-not $rejected){throw 'Unsafe family evidence accepted'}}
# Observed COM adapter .Count was null, but explicit dispatch Count was int 0.
$adapter=[pscustomobject]@{Count=$null;DispatchCount=[int]0;Items=@()}
if(-not ($adapter.Count -ne 0)){throw 'Old expression negative control unexpectedly changed'}
Assert-EmptyFamily $adapter.DispatchCount $adapter.Items ([uint32]259) ''; $passed++
Reject {Assert-EmptyFamily $null @() ([uint32]259) ''};$passed++
Reject {Assert-EmptyFamily '0' @() ([uint32]259) ''};$passed++
Reject {Assert-EmptyFamily -1 @() ([uint32]259) ''};$passed++
Reject {Assert-EmptyFamily 0 $null ([uint32]259) ''};$passed++
Reject {Assert-EmptyFamily 1 @('{5FE800A9-0163-42ED-822A-34D78D542EEC}') ([uint32]0) '{5FE800A9-0163-42ED-822A-34D78D542EEC}'};$passed++
Reject {Assert-EmptyFamily 0 @('unexpected') ([uint32]259) ''};$passed++
Reject {Assert-EmptyFamily 0 @() ([uint32]0) '{5FE800A9-0163-42ED-822A-34D78D542EEC}'};$passed++
Reject {Assert-EmptyFamily 0 @() ([uint32]1603) ''};$passed++
Reject {Assert-EmptyFamily 0 @() ([uint32]259) 'unexpected'};$passed++
Reject {Assert-EmptyFamily 0 @() $null ''};$passed++
Reject {Assert-EmptyFamily 0 @() 259 ''};$passed++
if($passed -ne 12){throw 'Missing guard tests'}
Write-Output "Family guard regression passed: $passed"

} $Script

# Capture real native-wrapper parameters without calling msi.dll.
& { param($Script)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $Script).Path,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Probe parse errors'}
$definitions=@($ast.FindAll({param($n) $n -is [Management.Automation.Language.StringConstantExpressionAst] -and $n.Value.Contains('public static class GPUILifecycleRegistrationApi')},$true))
if($definitions.Count -ne 1){throw 'Expected one native wrapper definition'}
$source=$definitions[0].Value.Replace('GPUILifecycleRegistrationApi','GPUILifecycleRegistrationApiContract')
# Replace only native imports with capture stubs: exercise the actual C# wrapper/argument selection without Windows Installer calls.
$enum='(?s)\[DllImport\("msi.dll",CharSet=CharSet.Unicode,ExactSpelling=true\)\]\s+private static extern uint MsiEnumProductsExW\([^;]+;'
$prop='(?s)\[DllImport\("msi.dll",CharSet=CharSet.Unicode,ExactSpelling=true\)\]\s+private static extern uint MsiGetProductInfoExW\([^;]+;'
if([regex]::Matches($source,$enum).Count -ne 1 -or [regex]::Matches($source,$prop).Count -ne 1){throw 'Unexpected native import definitions'}
$source=[regex]::Replace($source,$enum,@'
public static string CapturedSid; public static uint CapturedContext; public static string CapturedCode; public static string CapturedProperty;
private static uint MsiEnumProductsExW(string code,string sid,uint context,uint index,StringBuilder foundCode,out uint foundContext,StringBuilder foundSid,ref uint sidLength) {CapturedSid=sid;CapturedContext=context;CapturedCode=code;foundContext=0;return 259;}
'@)
$source=[regex]::Replace($source,$prop,@'
private static uint MsiGetProductInfoExW(string code,string sid,uint context,string property,StringBuilder value,ref uint length) {CapturedSid=sid;CapturedContext=context;CapturedCode=code;CapturedProperty=property;return 0;}
'@)
Add-Type -TypeDefinition $source
$code='{5FE800A9-0163-42ED-822A-34D78D542EEC}';$sid='S-1-5-21-1-2-3-500'
$found=[Text.StringBuilder]::new(39);$user=[Text.StringBuilder]::new(256);[uint32]$size=256;[uint32]$context=0
$r=[GPUILifecycleRegistrationApiContract]::Enumerate($code,'',4,0,$found,[ref]$context,$user,[ref]$size)
if($r -ne 259 -or $null -ne [GPUILifecycleRegistrationApiContract]::CapturedSid -or [GPUILifecycleRegistrationApiContract]::CapturedContext -ne 4 -or [GPUILifecycleRegistrationApiContract]::CapturedCode -cne $code){throw 'Machine enumeration lost native NULL or exact scope'}
$r=[GPUILifecycleRegistrationApiContract]::Enumerate($code,$sid,3,0,$found,[ref]$context,$user,[ref]$size)
if([GPUILifecycleRegistrationApiContract]::CapturedSid -cne $sid -or [GPUILifecycleRegistrationApiContract]::CapturedContext -ne 3){throw 'Current-user enumeration changed scope'}
$r=[GPUILifecycleRegistrationApiContract]::Property($code,'',4,'State',$user,[ref]$size)
if($null -ne [GPUILifecycleRegistrationApiContract]::CapturedSid -or [GPUILifecycleRegistrationApiContract]::CapturedProperty -cne 'State'){throw 'Machine property lost native NULL'}
foreach($scope in @(1,2)){$r=[GPUILifecycleRegistrationApiContract]::Property($code,$sid,$scope,'State',$user,[ref]$size);if([GPUILifecycleRegistrationApiContract]::CapturedSid -cne $sid -or [GPUILifecycleRegistrationApiContract]::CapturedContext -ne $scope){throw 'Per-user property changed scope'}}
foreach($case in @(@(7,$sid),@(3,''))){$rejected=$false;try{[void][GPUILifecycleRegistrationApiContract]::SidForContext($case[0],$case[1])}catch{$rejected=$true};if(-not $rejected){throw 'Invalid scope or missing SID accepted'}}
Write-Output 'Native parameter capture regression passed: 7'

} $Script
