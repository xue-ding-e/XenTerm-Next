# Read-only, bounded evidence for the real lifecycle's original GUI decision.
# Never use these observations as a substitute for its original window gate.
function Get-GpuiProcessObservation($Process, [bool]$Refresh=$false) {
 $row=@{present=($null -ne $Process);refreshed=$Refresh;errors=@()}
 if($null -eq $Process){return $row}
 if($Refresh){try{$Process.Refresh()}catch{$row.errors+=@{field='Refresh';message=$_.Exception.Message}}}
 foreach($name in @('Id','Path','StartTime','HasExited','MainWindowHandle','MainWindowTitle','SessionId')) {
  try {$value=$Process.$name;if($name -eq 'MainWindowHandle'){$value=$value.ToInt64()};$row[$name]=$value}
  catch {$row.errors+=@{field=$name;message=$_.Exception.Message}}
 }
 if($row.ContainsKey('HasExited') -and $row.HasExited){try{$row.exit_code=$Process.ExitCode}catch{$row.errors+=@{field='ExitCode';message=$_.Exception.Message}}}
 return $row
}
function Read-GpuiOwnedLogTail([string]$Directory) {
 $path=Join-Path $Directory 'log\error.log'
 $record=@{path=$path;present=$false;limit_bytes=65536}
 # Never follow a link out of this task-owned synthetic installation.
 foreach($part in @($Directory,(Join-Path $Directory 'log'),$path)) {
  if(Test-Path -LiteralPath $part){if((Get-Item -LiteralPath $part).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Refusing linked diagnostic log'}}
 }
 if(!(Test-Path -LiteralPath $path -PathType Leaf)){return $record}
 $stream=[IO.File]::Open($path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::ReadWrite)
 try {
  $record.present=$true;$record.size=$stream.Length
  $length=[int][Math]::Min(65536,$stream.Length);[void]$stream.Seek(-$length,[IO.SeekOrigin]::End)
  $bytes=[byte[]]::new($length);$offset=0
  while($offset -lt $length){$read=$stream.Read($bytes,$offset,$length-$offset);if($read -eq 0){break};$offset+=$read}
  $record.tail_utf8=[Text.Encoding]::UTF8.GetString($bytes,0,$offset);$record.tail_bytes=$offset
 }finally{$stream.Dispose()}
 return $record
}
function Save-GpuiGuiObservation($Evidence,[string]$Directory,[string]$Output) {
 try {$Evidence.application_error_log=Read-GpuiOwnedLogTail $Directory}catch{$Evidence.diagnostic_errors+=@{stage='application-log';message=$_.Exception.Message}}
 $bytes=[Text.UTF8Encoding]::new($false).GetBytes(($Evidence|ConvertTo-Json -Depth 14))
 $stream=[IO.File]::Open($Output,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
 try{$stream.Write($bytes,0,$bytes.Length)}finally{$stream.Dispose()}
}
