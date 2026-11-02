param([Parameter(Mandatory=$true)][string]$Script)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $Script).Path,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Packaging script parse errors'}
$functions=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Restore-GpuiProfileEnvironment'},$true))
if($functions.Count -ne 1){throw 'Expected unique environment restore helper'}
. ([scriptblock]::Create($functions[0].Extent.Text))
$initialPresent=Test-Path -LiteralPath Env:XENTERM_DATA_DIR
$initialValue=[Environment]::GetEnvironmentVariable('XENTERM_DATA_DIR','Process')
try {
 # Actual original setter on this PowerShell/.NET runtime: retain evidence of
 # whether the missing-to-empty bug reproduces, without exposing variable data.
 Remove-Item -LiteralPath Env:XENTERM_DATA_DIR -ErrorAction SilentlyContinue
 $absent=[Environment]::GetEnvironmentVariable('XENTERM_DATA_DIR','Process')
 $env:XENTERM_DATA_DIR='synthetic-version-probe'
 [Environment]::SetEnvironmentVariable('XENTERM_DATA_DIR',$absent,'Process')
 $oldBug=(Test-Path -LiteralPath Env:XENTERM_DATA_DIR) -and ([Environment]::GetEnvironmentVariable('XENTERM_DATA_DIR','Process') -ceq '')
 Write-Output "Original null restore produced present-empty: $oldBug; PowerShell $($PSVersionTable.PSVersion); runtime $([Environment]::Version)"
 if(!$oldBug){throw 'Expected original missing-to-empty negative control not reproduced; investigate runtime rather than claim causality'}
 $passed=0
 foreach($state in @('absent','empty','value')) {
  foreach($fails in @($false,$true)) {
   if($state -eq 'absent'){Remove-Item -LiteralPath Env:XENTERM_DATA_DIR -ErrorAction SilentlyContinue}
   else{Set-Item -LiteralPath Env:XENTERM_DATA_DIR -Value $(if($state -eq 'empty'){''}else{'C:\synthetic profile 中文'})}
   $present=Test-Path -LiteralPath Env:XENTERM_DATA_DIR;$value=[Environment]::GetEnvironmentVariable('XENTERM_DATA_DIR','Process')
   $caught=$null
   try {try{$env:XENTERM_DATA_DIR='synthetic-version-probe';if($fails){throw 'synthetic probe failure'}}finally{Restore-GpuiProfileEnvironment $present $value}}
   catch{$caught=$_.Exception.Message}
   if((Test-Path -LiteralPath Env:XENTERM_DATA_DIR) -ne $present -or [Environment]::GetEnvironmentVariable('XENTERM_DATA_DIR','Process') -cne $value){throw "State not restored: $state"}
   if($fails -and $caught -cne 'synthetic probe failure'){throw 'Original probe exception lost'}
   if(!$fails -and $null -ne $caught){throw 'Unexpected restoration failure'}
   $passed++
  }
 }
 Write-Output "Profile environment restoration contracts passed: $passed success/error states"
}finally{Restore-GpuiProfileEnvironment $initialPresent $initialValue}
