param([string]$ClPath,[string]$LinkPath,[string]$MetadataPath)
$ErrorActionPreference = 'Stop'
$timer = [Diagnostics.Stopwatch]::StartNew()
$stages = @('powershell-entered','cl-signature-before','cl-signature-after','cl-version-before','cl-version-after','link-signature-before','link-signature-after','link-version-before','link-version-after','finished')
function Record-Stage([string]$Stage) {
  $index = [Array]::IndexOf($stages,$Stage)
  if ($index -lt 0) { throw 'Unknown signature stage' }
  $elapsed = $timer.ElapsedMilliseconds.ToString([Globalization.CultureInfo]::InvariantCulture)
  $json = '{"schema":1,"stage":"' + $Stage + '","elapsed_ms":' + $elapsed + ',"raw_data_included":false}'
  $path = [IO.Path]::Combine([IO.Path]::GetDirectoryName($MetadataPath),'signature-stage-' + $index.ToString('D2',[Globalization.CultureInfo]::InvariantCulture) + '.json')
  $stream = [IO.File]::Open($path,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
  try {
    $bytes = [Text.Encoding]::UTF8.GetBytes($json)
    $stream.Write($bytes,0,$bytes.Length)
    $stream.Flush($true)
  } finally { $stream.Dispose() }
}
function Fixed-ResultJson($Result) {
  $rows = @()
  foreach ($name in @('cl','link')) {
    $row = $Result[$name]
    $numbers = @()
    foreach ($number in $row.version) {
      if ($number -isnot [int] -or $number -lt 0 -or $number -gt 65535) { throw 'Invalid version component' }
      $numbers += $number.ToString([Globalization.CultureInfo]::InvariantCulture)
    }
    if ($numbers.Count -ne 4) { throw 'Invalid version length' }
    $flags = @()
    foreach ($key in @('valid_signature','microsoft_signer','microsoft_company')) {
      if ($row[$key] -isnot [bool]) { throw 'Invalid signature flag' }
      $flags += '"' + $key + '":' + $row[$key].ToString().ToLowerInvariant()
    }
    $rows += '"' + $name + '":{' + [string]::Join(',',$flags) + ',"version":[' + [string]::Join(',',$numbers) + ']}'
  }
  return '{' + [string]::Join(',',$rows) + '}'
}
try {
  if (-not [IO.Path]::IsPathRooted($MetadataPath) -or [IO.Path]::GetFileName($MetadataPath) -ne 'signature-stage.json' -or -not [IO.Directory]::Exists([IO.Path]::GetDirectoryName($MetadataPath))) { throw 'Invalid metadata path' }
  Record-Stage 'powershell-entered'
  $result = @{}
  foreach ($pair in @(@('cl',$ClPath),@('link',$LinkPath))) {
    Record-Stage ($pair[0]+'-signature-before')
    $signature = Get-AuthenticodeSignature -LiteralPath $pair[1]
    Record-Stage ($pair[0]+'-signature-after')
    Record-Stage ($pair[0]+'-version-before')
    $version = (Get-Item -LiteralPath $pair[1]).VersionInfo
    Record-Stage ($pair[0]+'-version-after')
    $result[$pair[0]] = @{
      valid_signature = ($signature.Status -eq 'Valid')
      microsoft_signer = ($signature.SignerCertificate.Subject -match '(^|, )CN=Microsoft Corporation(,|$)')
      microsoft_company = ($version.CompanyName -eq 'Microsoft Corporation')
      version = @($version.FileMajorPart,$version.FileMinorPart,$version.FileBuildPart,$version.FilePrivatePart)
    }
  }
  Record-Stage 'finished'
  [Console]::Out.WriteLine((Fixed-ResultJson $result))
} catch {
  exit 1
}
