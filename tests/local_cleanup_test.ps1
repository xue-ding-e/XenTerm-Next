# Exercise deletion against synthetic caches, profiles, and an external junction.
$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) ('xenterm-clean-test-' + [Guid]::NewGuid().ToString('N'))
$external = $root + '-outside'
try {
    foreach ($relative in @('runtime\approval-ui-20261012', 'source\target\debug\deps\config',
        'source\target\debug\build\obsolete', 'builds\windows-next', 'data')) {
        [IO.Directory]::CreateDirectory((Join-Path $root $relative)) | Out-Null
    }
    [IO.Directory]::CreateDirectory($external) | Out-Null
    $files = @('runtime\approval-ui-20261012\xenterm.exe', 'source\target\debug\deps\obsolete.rlib',
        'source\target\debug\build\obsolete\output', 'source\target\debug\deps\config\secret.key',
        'source\target\debug\deps\config\sessions.db', 'builds\windows-next\keep.rlib', 'data\secret.key')
    foreach ($relative in $files) { Set-Content -LiteralPath (Join-Path $root $relative) -Value 'synthetic' }
    Set-Content -LiteralPath (Join-Path $external 'xenterm.exe') -Value 'external-keep'
    New-Item -ItemType Junction -Path (Join-Path $root 'runtime\approval-ui-20261012\config') -Target $external | Out-Null
    $script = Join-Path $PSScriptRoot '..\scripts\clean-local.ps1'
    & $script -WorkspaceRoot $root
    if (-not (Test-Path -LiteralPath (Join-Path $root 'source\target\debug\deps\obsolete.rlib'))) {
        throw 'Preview deleted a file.'
    }
    & $script -WorkspaceRoot $root -Apply
    foreach ($relative in @('runtime\approval-ui-20261012\xenterm.exe', 'source\target\debug\deps\obsolete.rlib',
        'source\target\debug\build\obsolete')) {
        if (Test-Path -LiteralPath (Join-Path $root $relative)) { throw "Obsolete artifact survived: $relative" }
    }
    foreach ($relative in @('source\target\debug\deps\config\secret.key', 'source\target\debug\deps\config\sessions.db',
        'builds\windows-next\keep.rlib', 'data\secret.key')) {
        if ((Get-Content -LiteralPath (Join-Path $root $relative) -Raw).Trim() -ne 'synthetic') {
            throw "Protected file was changed: $relative"
        }
    }
    if ((Get-Content -LiteralPath (Join-Path $external 'xenterm.exe') -Raw).Trim() -ne 'external-keep') {
        throw 'Cleanup followed the junction.'
    }
    Write-Host 'PASS: cleanup preview, artifacts, retained cache, profile data and junction boundaries'
} finally {
    $junction = Join-Path $root 'runtime\approval-ui-20261012\config'
    if (Test-Path -LiteralPath $junction) { [IO.Directory]::Delete($junction) }
    foreach ($directory in @($root, $external)) {
        $full = [IO.Path]::GetFullPath($directory)
        if ($full -like ([IO.Path]::GetTempPath().TrimEnd('\') + '\xenterm-clean-test-*')) {
            Remove-Item -LiteralPath $full -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
