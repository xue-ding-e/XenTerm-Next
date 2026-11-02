param([Parameter(Mandatory=$true)][string]$Script)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $Script).Path,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Initialization script parse errors'}
$f=@($ast.FindAll({param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Assert-GpuiInitializationProof'},$true))
if($f.Count -ne 1){throw 'Expected one proof checker'}
$normal=@($ast.FindAll({param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Normalize-GpuiLocalProfilePath'},$true))
if($normal.Count -ne 1){throw 'Expected one strict local path normalizer'}
. ([scriptblock]::Create($normal[0].Extent.Text))
. ([scriptblock]::Create($f[0].Extent.Text))
$profile='C:\synthetic-native-contract'
$version='xt-beta-8819e2e-T1793303184'
$v=@{exit_code=0;stdout="xenterm $version`n"}
$p=@{exit_code=0;stdout=(@{mode='restricted';data_dir=$profile}|ConvertTo-Json)}
function Reject([scriptblock]$body){$failed=$false;try{& $body}catch{$failed=$true};if(-not $failed){throw 'Unsafe CLI proof accepted'}}
Assert-GpuiInitializationProof $v $p $profile $version
Reject {Assert-GpuiInitializationProof @{exit_code=1;stdout=$v.stdout} $p $profile $version}
Reject {Assert-GpuiInitializationProof @{exit_code=0;stdout='xenterm wrong'} $p $profile $version}
Reject {Assert-GpuiInitializationProof $v @{exit_code=1;stdout=$p.stdout} $profile $version}
Reject {Assert-GpuiInitializationProof $v @{exit_code=0;stdout='invalid JSON'} $profile $version}
Reject {Assert-GpuiInitializationProof $v @{exit_code=0;stdout=(@{mode='full';data_dir=$profile}|ConvertTo-Json)} $profile $version}
Reject {Assert-GpuiInitializationProof $v @{exit_code=0;stdout=(@{mode='restricted';data_dir=($profile+'-other')}|ConvertTo-Json)} $profile $version}
Reject {Assert-GpuiInitializationProof $v @{exit_code=0;stdout='{}'} $profile $version}
Assert-GpuiInitializationProof $v @{exit_code=0;stdout=(@{mode='restricted';data_dir=('\\?\'+$profile)}|ConvertTo-Json)} $profile $version
$unicode='D:\安装验证\用户数据\config'
Assert-GpuiInitializationProof $v @{exit_code=0;stdout=(@{mode='restricted';data_dir=('\\?\'+$unicode)}|ConvertTo-Json)} $unicode $version
Assert-GpuiInitializationProof $v @{exit_code=0;stdout=(@{mode='restricted';data_dir='c:\SYNTHETIC-NATIVE-CONTRACT\'}|ConvertTo-Json)} $profile $version
foreach($bad in @('relative\config','C:relative','\rooted','\\server\share\config','\\?\UNC\server\share\config','\\.\C:\synthetic-native-contract','\??\C:\synthetic-native-contract','\\?\GLOBALROOT\Device\HarddiskVolume1\config','\\?\Volume{abc}\config','C:\other','\\?\C:\other','C:\a\..\synthetic-native-contract','C:\a\.\config','C:\a\\config','C:\a\config.','C:\a\config ','C:\a\NUL','C:\a\COM1.txt','C:\a\file:stream','C:/synthetic-native-contract','C:\')){
 Reject {Assert-GpuiInitializationProof $v @{exit_code=0;stdout=(@{mode='restricted';data_dir=$bad}|ConvertTo-Json)} $profile $version}
 if($bad -notin @('C:\other','\\?\C:\other')){Reject {Normalize-GpuiLocalProfilePath $bad}}
}
Write-Output 'Native initialization proof regression passed: 4 positive + 47 negative; no CLI executed by contract' 
