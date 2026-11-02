# Test only the property reader AST: never dot-source the lifecycle/install script.
[CmdletBinding()]
param(
 [string]$LifecycleScript = (Join-Path $PSScriptRoot '../scripts/test_next_gpui_msi_e2e.ps1'),
 [string[]]$MsiPaths = @()
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$parseErrors = $null; $tokens = $null
$ast = [Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $LifecycleScript).Path, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -ne 0) { throw 'Lifecycle script does not parse' }
$definitions = @($ast.FindAll({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Msi-Property'}, $true))
if ($definitions.Count -ne 1) { throw 'Expected one property reader' }
. ([scriptblock]::Create($definitions[0].Extent.Text))
function Assert-Scalar([object[]]$Values, [string]$Expected) {
 if ($Values.Count -ne 1 -or $Values[0] -isnot [string] -or $Values[0] -cne $Expected) { throw 'Property reader returned a polluted/non-string result' }
}
# Explicit null success output reproduces the native COM observation from all
# three retained MSIs; empty ALLUSERS is not permission to return a null array.
$script:closeCount = 0
$script:missing = $false
$script:failExecute = $false
$script:failFetch = $false
$script:failRead = $false
$recordFixture = [pscustomobject]@{}
$recordFixture | Add-Member ScriptMethod StringData { param($index) if ($script:failRead) { throw 'synthetic read failure' }; return '{7548A7CB-F70B-4AE7-A5F0-F952D45AA826}' }
$viewFixture = [pscustomobject]@{}
$viewFixture | Add-Member ScriptMethod Execute { if ($script:failExecute) { throw 'synthetic execute failure' }; Write-Output -NoEnumerate $null }
$viewFixture | Add-Member ScriptMethod Fetch { if ($script:failFetch) { throw 'synthetic fetch failure' }; if ($script:missing) { return $null }; return $recordFixture }
$viewFixture | Add-Member ScriptMethod Close { $script:closeCount++; Write-Output -NoEnumerate $null }
$dbFixture = [pscustomobject]@{}
$dbFixture | Add-Member ScriptMethod OpenView { param($sql) return $viewFixture }
$installer = [pscustomobject]@{}
$installer | Add-Member ScriptMethod OpenDatabase { param($path, $mode) if ($mode -ne 0) { throw 'Database is not read-only' }; return $dbFixture }
Assert-Scalar @(Msi-Property 'synthetic.msi' 'UpgradeCode') '{7548A7CB-F70B-4AE7-A5F0-F952D45AA826}'
if ($script:closeCount -ne 1) { throw 'Present-property view not closed once' }
$script:missing = $true
Assert-Scalar @(Msi-Property 'synthetic.msi' 'ALLUSERS') ''
if ($script:closeCount -ne 2) { throw 'Missing-property view not closed once' }
$script:missing = $false
foreach ($failure in @('failExecute','failFetch','failRead')) {
 Set-Variable -Scope Script -Name $failure -Value $true
 $before = $script:closeCount; $caught = $false
 try { $null = Msi-Property 'synthetic.msi' 'ProductCode' }
 catch { if ($_.Exception.ToString() -notmatch 'synthetic (execute|fetch|read) failure') { throw }; $caught = $true }
 finally { Set-Variable -Scope Script -Name $failure -Value $false }
 if (-not $caught -or $script:closeCount -ne $before + 1) { throw 'Error path did not propagate and close its view' }
}
Write-Host 'Property reader: scalar, absent-row and three error-path regressions passed'
if ($MsiPaths.Count -gt 0) {
 if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $MsiPaths.Count -ne 3) { throw 'Real database regression requires three MSIs on a disposable hosted runner' }
 $installer = New-Object -ComObject WindowsInstaller.Installer
 try {
  foreach ($path in $MsiPaths) {
   $resolved = (Resolve-Path -LiteralPath $path).Path
   $before = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash
   Assert-Scalar @(Msi-Property $resolved 'UpgradeCode') '{7548A7CB-F70B-4AE7-A5F0-F952D45AA826}'
   Assert-Scalar @(Msi-Property $resolved 'ALLUSERS') ''
   Assert-Scalar @(Msi-Property $resolved 'GPUI_DIAGNOSTIC_NONEXISTENT_PROPERTY') ''
   $code = @(Msi-Property $resolved 'ProductCode')
   if ($code.Count -ne 1 -or $code[0] -isnot [string] -or $code[0] -notmatch '^\{[0-9A-Fa-f-]{36}\}$') { throw 'ProductCode is not a scalar GUID' }
   if ((Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash -cne $before) { throw 'Read-only regression changed an MSI' }
  }
 } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) }
 Write-Host 'Property reader: all three real MSI scalar/absent-row/hash regressions passed'
}
