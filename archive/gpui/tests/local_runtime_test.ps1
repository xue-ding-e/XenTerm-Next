# Real portable-file handoff tests, entirely within a disposable test workspace.
param([Parameter(Mandatory = $true)][string]$Exe)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\scripts\local-runtime.ps1')
$root = Join-Path ([IO.Path]::GetTempPath()) ('xenterm-runtime-test-' + [Guid]::NewGuid().ToString('N'))
$process = $null
$runtimeLock = $null
function Assert-True {
    param([bool]$Value, [string]$Message)
    if (-not $Value) { throw $Message }
}
try {
    [IO.Directory]::CreateDirectory((Join-Path $root 'runtime\pending')) | Out-Null
    [IO.Directory]::CreateDirectory((Join-Path $root 'data')) | Out-Null
    [IO.File]::WriteAllText((Join-Path $root 'data\sessions.json'),
        '{"sessions":[],"defaults_rev":6,"mcp_enabled":true,"mcp_use_saved_credentials":true,"mcp_allow_commands":true,"mcp_allow_file_transfers":true}',
        (New-Object Text.UTF8Encoding($false)))
    $runtimeLock = Enter-LocalRuntimeLock $root
    $locked = $false
    try { $other = Enter-LocalRuntimeLock $root; $other.Dispose() } catch { $locked = $true }
    Assert-True $locked 'Concurrent publication was not blocked.'
    $version = Get-LocalVersion $Exe
    Assert-True ($version -match '-([0-9a-f]{7})-T') 'Test binary must report a prerelease commit.'
    $commit = $Matches[1] + ('0' * 33)
    $pending = Join-Path $root 'runtime\pending'
    Copy-Item -LiteralPath $Exe -Destination (Join-Path $pending 'xenterm.exe')
    $manifest = @{
        repository = 'https://github.com/xue-ding-e/XenTerm-Next.git'
        commit = $commit
        runtime_version = $version
        sha256 = (Get-FileHash -LiteralPath $Exe).Hash
    }
    $manifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $pending 'BUILD.json') -Encoding UTF8
    Assert-True (Publish-LocalPending $root) 'Initial package was not published.'
    $current = Join-Path $root 'runtime\current'
    Copy-Item -LiteralPath $current -Destination $pending -Recurse
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = Join-Path $current 'xenterm.exe'
    $info.Arguments = '--data-dir "{0}" mcp serve' -f (Join-Path $root 'data')
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($info)
    $errorOutput = $process.StandardError.ReadToEndAsync()
    $requests = @(
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"runtime-test","version":"1"}}}',
        '{"jsonrpc":"2.0","method":"notifications/initialized"}',
        '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
    )
    foreach ($request in $requests) { $process.StandardInput.WriteLine($request) }
    $process.StandardInput.Flush()
    $replyTask = $process.StandardOutput.ReadLineAsync()
    Assert-True ($replyTask.Wait(15000)) 'Fixture MCP initialization timed out.'
    $reply = $replyTask.Result | ConvertFrom-Json
    Assert-True ($reply.id -eq 1 -and $reply.PSObject.Properties['result']) ("Fixture MCP initialization failed: " + $replyTask.Result)
    $remainingOutput = $process.StandardOutput.ReadToEndAsync()
    Assert-True (-not (Publish-LocalPending $root)) 'A running current binary was replaced.'
    Assert-True (Test-Path -LiteralPath $pending) 'Pending package was lost while current was running.'
    $process.StandardInput.Close()
    Assert-True ($process.WaitForExit(15000)) 'Fixture MCP did not exit on EOF.'
    Assert-True (Publish-LocalPending $root) 'Deferred update was not applied.'
    Assert-True (Test-Path -LiteralPath (Join-Path $root 'runtime\previous\xenterm.exe')) 'Rollback package was not kept.'
    $blocked = $false
    try { Assert-LocalChildPath $root (Join-Path $root '..\outside') | Out-Null } catch { $blocked = $true }
    Assert-True $blocked 'Workspace boundary was not enforced.'
    Copy-Item -LiteralPath $current -Destination $pending -Recurse
    Set-Content -LiteralPath (Join-Path $pending 'xenterm.exe') -Value 'corrupt'
    $blocked = $false
    try { Publish-LocalPending $root | Out-Null } catch { $blocked = $true }
    Assert-True $blocked 'A corrupt package was accepted.'
    Set-Content -LiteralPath (Join-Path $root 'runtime\previous\secret.key') -Value 'synthetic'
    $blocked = $false
    try { Remove-LocalPackage $root (Join-Path $root 'runtime\previous') } catch { $blocked = $true }
    Assert-True $blocked 'Profile data deletion was not blocked.'
    Write-Host 'PASS: lock, handoff, running-process deferral, rollback, corruption and profile guards'
} finally {
    if ($process) {
        if (-not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
        $process.Dispose()
    }
    if ($runtimeLock) { $runtimeLock.Dispose() }
    if ($root -like ([IO.Path]::GetTempPath().TrimEnd('\') + '\xenterm-runtime-test-*')) {
        Remove-Item -LiteralPath $root -Recurse -Force
    }
}
