param([string]$ClPath,[string]$LinkPath)
$ErrorActionPreference = 'Stop'
try {
  $result = @{}
  foreach ($pair in @(@('cl',$ClPath),@('link',$LinkPath))) {
    $signature = Get-AuthenticodeSignature -LiteralPath $pair[1]
    $version = (Get-Item -LiteralPath $pair[1]).VersionInfo
    $result[$pair[0]] = @{
      valid_signature = ($signature.Status -eq 'Valid')
      microsoft_signer = ($signature.SignerCertificate.Subject -match '(^|, )CN=Microsoft Corporation(,|$)')
      microsoft_company = ($version.CompanyName -eq 'Microsoft Corporation')
      version = @($version.FileMajorPart,$version.FileMinorPart,$version.FileBuildPart,$version.FilePrivatePart)
    }
  }
  $result | ConvertTo-Json -Compress -Depth 4
} catch {
  exit 1
}
