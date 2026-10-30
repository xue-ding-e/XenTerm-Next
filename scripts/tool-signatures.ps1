param([string]$ClPath,[string]$LinkPath,[string]$MetadataPath)
$ErrorActionPreference = 'Stop'
$timer = [Diagnostics.Stopwatch]::StartNew()
$metadata = $null
$stages = @('powershell-entered','cl-signature-before','cl-signature-after','cl-version-before','cl-version-after','link-signature-before','link-signature-after','link-version-before','link-version-after','finished')
function Record-Stage([string]$Stage) {
  if ($Stage -notin $stages) { throw 'Unknown signature stage' }
  $bytes = [Text.Encoding]::UTF8.GetBytes((@{schema=1;stage=$Stage;elapsed_ms=$timer.ElapsedMilliseconds;raw_data_included=$false} | ConvertTo-Json -Compress))
  $metadata.Position = 0
  $metadata.SetLength(0)
  $metadata.Write($bytes,0,$bytes.Length)
  $metadata.Flush($true)
}
try {
  if (-not [IO.Path]::IsPathRooted($MetadataPath) -or [IO.Path]::GetFileName($MetadataPath) -ne 'signature-stage.json' -or -not [IO.Directory]::Exists([IO.Path]::GetDirectoryName($MetadataPath))) { throw 'Invalid metadata path' }
  $metadata = [IO.File]::Open($MetadataPath,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
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
  $result | ConvertTo-Json -Compress -Depth 4
} catch {
  exit 1
} finally {
  if ($null -ne $metadata) { $metadata.Dispose() }
}
