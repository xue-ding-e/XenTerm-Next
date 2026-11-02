param([Parameter(Mandatory=$true)][string]$Script,[Parameter(Mandatory=$true)][string]$LifecycleScript)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $Script).Path,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Observation helper parse errors'}
foreach($name in @('Get-GpuiProcessObservation','Read-GpuiOwnedLogTail','Save-GpuiGuiObservation')) {
 $nodes=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name},$true))
 if($nodes.Count -ne 1){throw 'Expected unique observation helper'}
 . ([scriptblock]::Create($nodes[0].Extent.Text))
}
$absent=Get-GpuiProcessObservation $null
if($absent.present -or $absent.errors.Count){throw 'Missing process misreported'}
$fake=[pscustomobject]@{Id=42;Path='C:\synthetic\xenterm.exe';StartTime=[DateTime]::UtcNow;HasExited=$false;MainWindowHandle=[IntPtr]12;MainWindowTitle='synthetic';SessionId=1;refreshes=0}
$fake|Add-Member ScriptMethod Refresh {$this.refreshes++}
$r=Get-GpuiProcessObservation $fake
if($fake.refreshes -ne 0 -or $r.MainWindowHandle -ne 12){throw 'Original getter observation mutated process'}
$r=Get-GpuiProcessObservation $fake $true
if($fake.refreshes -ne 1 -or $r.Id -ne 42){throw 'Held refresh not observed'}
$fake.HasExited=$true;$fake|Add-Member NoteProperty ExitCode 7
$r=Get-GpuiProcessObservation $fake $true
if($r.exit_code -ne 7){throw 'Exited status lost'}
$broken=[pscustomobject]@{Id=43}
$broken|Add-Member ScriptMethod Refresh {throw 'synthetic refresh failure'}
$r=Get-GpuiProcessObservation $broken $true
if($r.Id -ne 43 -or $r.errors.Count -lt 2){throw 'Partial process evidence lost on error'}
$dir=Join-Path $env:RUNNER_TEMP ('gpui-observation-contract-'+[guid]::NewGuid())
[void][IO.Directory]::CreateDirectory($dir)
try {
 $r=Read-GpuiOwnedLogTail $dir;if($r.present){throw 'Missing log invented'}
 [void][IO.Directory]::CreateDirectory((Join-Path $dir 'log'))
 $log=Join-Path $dir 'log\error.log';[IO.File]::WriteAllText($log,('x'*70000)+'TAIL')
 $r=Read-GpuiOwnedLogTail $dir
 if($r.size -ne 70004 -or $r.tail_bytes -ne 65536 -or !$r.tail_utf8.EndsWith('TAIL')){throw 'Log tail not bounded/exact'}
 $e=@{diagnostic_errors=@();primary_error='original GUI failure';samples=@()};$out=Join-Path $dir 'evidence.json'
 Save-GpuiGuiObservation $e $dir $out
 $saved=Get-Content -LiteralPath $out -Raw|ConvertFrom-Json
 if($saved.primary_error -cne 'original GUI failure'){throw 'Primary failure lost'}
 $before=(Get-FileHash -LiteralPath $out).Hash;$rejected=$false
 try{Save-GpuiGuiObservation $e $dir $out}catch{$rejected=$true}
 if(!$rejected -or (Get-FileHash -LiteralPath $out).Hash -cne $before){throw 'Existing evidence overwritten'}
 Write-Output 'GUI observation contracts passed: missing/live/refreshed/exited/partial process, missing/bounded log, retained error and exclusive evidence'
}finally{Remove-Item -LiteralPath $dir -Recurse -Force}

# Execute the actual lifecycle catch/finally around a synthetic primary failure.
# No lifecycle body, MSI, launch or cleanup actions are executed by this test.
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $LifecycleScript).Path,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Lifecycle parse errors'}
$tries=@($ast.FindAll({param($n)$n -is [Management.Automation.Language.TryStatementAst] -and $n.Body.Extent.Text.Contains('$held=Start-Process $menu -PassThru')},$true))
if($tries.Count -ne 1 -or $tries[0].CatchClauses.Count -ne 1 -or $null -eq $tries[0].Finally){throw 'Expected unique launch evidence error handling'}
$handler=[scriptblock]::Create("try {throw 'synthetic original GUI failure'} " + $tries[0].CatchClauses[0].Extent.Text + ' finally ' + $tries[0].Finally.Extent.Text)
function Save-GpuiGuiObservation {throw 'synthetic evidence write failure'}
$guiEvidence=@{diagnostic_errors=@()};$gui=$null;$primaryGuiError=$null;$Directory='C:\synthetic';$guiEvidencePath='not-written.json'
$held=[pscustomobject]@{Id=44}
$held|Add-Member ScriptMethod Refresh {throw 'synthetic refresh failure'}
$held|Add-Member ScriptMethod Dispose {throw 'synthetic dispose failure'}
$caught=$null
try{& $handler}catch{$caught=$_.Exception.Message}
if($caught -cne 'synthetic original GUI failure' -or $guiEvidence.primary_error -cne 'synthetic original GUI failure'){throw 'Diagnostic failure replaced original GUI error'}
if(!$guiEvidence.ContainsKey('final_held') -or $guiEvidence.final_held.Id -ne 44 -or $guiEvidence.final_held.errors.Count -lt 2 -or !$guiEvidence.ContainsKey('final_original')){throw 'Partial process evidence lost during original failure'}
Write-Output 'Actual lifecycle catch/finally retained original GUI error across evidence/refresh/dispose failures'
