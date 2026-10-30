param([Parameter(Mandatory)][ValidateSet('Initial','Final')][string]$Phase)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$root=Join-Path $env:RUNNER_TEMP 'key-import-compile-evidence'
function Plain([string]$path){
 if(-not [IO.Path]::IsPathFullyQualified($path) -or [IO.Path]::GetFullPath($path) -ne $path){throw 'Noncanonical output'}
 $p=$path;while($p){if(Test-Path -LiteralPath $p){if(((Get-Item -LiteralPath $p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'Reparse output'}};$p=[IO.Path]::GetDirectoryName($p)}
}
function Control {
 if($env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:RUNNER_ARCH -ne 'X64' -or $env:GITHUB_REPOSITORY -ne 'xue-ding-e/XenTerm-Next' -or $env:GITHUB_REF -ne 'refs/heads/validation/xenterm-compile-preflight-20261030' -or $env:EXPECTED_CONTROL_SHA -notmatch '^[0-9a-f]{40}$' -or $env:GITHUB_SHA -ne $env:EXPECTED_CONTROL_SHA -or $env:GITHUB_RUN_ID -notmatch '^[0-9]+$'){throw 'Control scope'}
 $head=& git -C $env:GITHUB_WORKSPACE rev-parse HEAD
 if($LASTEXITCODE -ne 0 -or $head -ne $env:EXPECTED_CONTROL_SHA){throw 'Control head'}
 $status=& git -C $env:GITHUB_WORKSPACE status --porcelain --untracked-files=all
 if($LASTEXITCODE -ne 0 -or $status){throw 'Control modified'}
}
Plain $root
if($Phase -eq 'Initial'){
 Control
 if(Test-Path -LiteralPath $root){throw 'Output already exists'}
 [IO.Directory]::CreateDirectory($root)|Out-Null
 [IO.File]::WriteAllText((Join-Path $root 'control.identity'),($env:GITHUB_RUN_ID+':'+$env:EXPECTED_CONTROL_SHA))
}else{
 if(-not [IO.Directory]::Exists($root)){throw 'Initial scope missing'}
 $final=$false
 try{Control;$final=([IO.File]::ReadAllText((Join-Path $root 'control.identity')) -ceq ($env:GITHUB_RUN_ID+':'+$env:EXPECTED_CONTROL_SHA))}finally{
   $steps=@{};foreach($n in @('SETUP_OUTCOME','HOST_OUTCOME','SDK_OUTCOME')){$v=[Environment]::GetEnvironmentVariable($n);$steps[$n]=if($v -in @('success','failure','cancelled','skipped')){$v}else{'unknown'}}
   $record=@{schema=1;run=$env:GITHUB_RUN_ID;control=$env:EXPECTED_CONTROL_SHA;final_identity=$final;steps=$steps;product_compile_executed=$false;product_runtime_executed=$false;rust_toolchain_verified=$false;product_environment_qualified=$false}
   $f=[IO.File]::Open((Join-Path $root 'preflight.json'),[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
   try{$b=[Text.Encoding]::UTF8.GetBytes(($record|ConvertTo-Json -Depth 8 -Compress));$f.Write($b,0,$b.Length)}finally{$f.Dispose()}
 }
 if(-not $final -or $env:SETUP_OUTCOME -ne 'success' -or $env:HOST_OUTCOME -ne 'success' -or $env:SDK_OUTCOME -ne 'success'){throw 'Preflight incomplete'}
}
