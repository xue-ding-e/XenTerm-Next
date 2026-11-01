param([Parameter(Mandatory = $true)][string]$WorkspaceRoot)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'local-runtime.ps1')
$runtimeLock = $null
try {
    $runtimeLock = Enter-LocalRuntimeLock $WorkspaceRoot
    $null = Publish-LocalPending $WorkspaceRoot
    $exe = Assert-LocalChildPath $WorkspaceRoot (Join-Path $WorkspaceRoot 'runtime\current\xenterm.exe')
    $profile = Assert-LocalChildPath $WorkspaceRoot (Join-Path $WorkspaceRoot 'data')
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'Run the local update/build first.' }
    if (-not (Test-Path -LiteralPath $profile -PathType Container)) { throw 'Existing data profile was not found.' }
    Start-Process -FilePath $exe -ArgumentList ('--data-dir "{0}"' -f $profile) `
        -WorkingDirectory (Split-Path -Parent $exe) -WindowStyle Normal
} catch {
    Add-Type -AssemblyName System.Windows.Forms
    [Windows.Forms.MessageBox]::Show($_.Exception.Message, 'XenTerm Next') | Out-Null
    exit 1
} finally {
    if ($runtimeLock) { $runtimeLock.Dispose() }
}
