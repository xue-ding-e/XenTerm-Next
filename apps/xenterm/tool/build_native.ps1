# Builds the Rust FFI bridge and drops it next to the Flutter Windows binary.
# Usage: powershell -File apps/xenterm/tool/build_native.ps1
$ErrorActionPreference = 'Stop'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..\..')
Push-Location $repo
try {
    cargo +stable-x86_64-pc-windows-msvc build --release -p xenterm-bridge
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    $dll = Join-Path $repo 'target\release\xenterm_bridge.dll'
    if (-not (Test-Path $dll)) { throw "missing $dll" }

    foreach ($config in @('Debug', 'Release')) {
        $dir = Join-Path $PSScriptRoot "..\build\windows\x64\runner\$config"
        if (Test-Path $dir) {
            Copy-Item $dll $dir -Force
            Write-Output "copied -> $config"
        }
    }
} finally {
    Pop-Location
}
