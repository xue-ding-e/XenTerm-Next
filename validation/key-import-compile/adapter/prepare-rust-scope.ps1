# Preparation candidate: modifies only this step's process PATH temporarily.
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
if(-not $IsWindows -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:RUNNER_ARCH -ne 'X64' -or $env:GITHUB_REPOSITORY -ne 'xue-ding-e/XenTerm-Next'){throw 'Hosted Windows scope required'}
$root=Join-Path $env:RUNNER_TEMP 'key-import-compile-evidence'
function Plain([string]$path) {
    if(-not [IO.Path]::IsPathFullyQualified($path) -or [IO.Path]::GetFullPath($path) -ne $path -or $path.Contains("`n") -or $path.Contains("`r")){throw 'Canonical path required'}
    $p=$path
    while($p){if(Test-Path -LiteralPath $p){if(((Get-Item -LiteralPath $p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'Reparse path'}};$p=[IO.Path]::GetDirectoryName($p)}
}
Plain $root
if(-not(Test-Path -LiteralPath $root -PathType Container)){throw 'Owned output root required'}
$gitRoot=Join-Path $env:ProgramFiles 'Git'
$bash=Join-Path $gitRoot 'bin/bash.exe'
$gitBin=Join-Path $gitRoot 'bin';$gitUsr=Join-Path $gitRoot 'usr/bin'
$osBin=Join-Path $env:SystemRoot 'System32'
foreach($path in @($bash,$gitBin,$gitUsr,$osBin,$env:SystemRoot)){Plain $path;if(-not(Test-Path -LiteralPath $path)){throw 'Required installed shell tool unavailable'}}
foreach($name in @('BASH_ENV','ENV','RUSTUP_TOOLCHAIN','RUSTUP_DIST_ROOT')){if([Environment]::GetEnvironmentVariable($name,'Process')){throw 'Unexpected tool override'}}
foreach($name in @('cargo-home','rustup-home')){if(Test-Path -LiteralPath (Join-Path $root $name)){throw 'Fresh synthetic Rust roots required'}}
New-Item -ItemType Directory -Path (Join-Path $root 'cargo-home/bin') -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $root 'rustup-home') | Out-Null
foreach($name in @('home','appdata','localappdata','tmp')){$p=Join-Path $root $name;Plain $p;if(-not(Test-Path -LiteralPath $p)){New-Item -ItemType Directory -Path $p | Out-Null}}
$path=@((Join-Path $root 'cargo-home/bin'),$gitBin,$gitUsr,$osBin,$env:SystemRoot) -join ';'
$before=(Get-FileHash -LiteralPath $bash -Algorithm SHA256).Hash
$oldPath=$env:PATH;$oldHome=$env:HOME;$oldProfile=$env:USERPROFILE
try {
    $env:PATH=$path;$env:HOME=Join-Path $root 'home';$env:USERPROFILE=$env:HOME
    # Includes empty synthetic bin up front, so proxies created by the fixed
    # action become visible without reintroducing ambient Rust directories.
    & $bash --noprofile --norc -c 'if command -v rustup >/dev/null 2>&1; then exit 9; fi'
    if($LASTEXITCODE -ne 0){throw 'Ambient rustup visible'}
} finally {$env:PATH=$oldPath;$env:HOME=$oldHome;$env:USERPROFILE=$oldProfile}
if((Get-FileHash -LiteralPath $bash -Algorithm SHA256).Hash -ne $before){throw 'Shell changed'}
"setup_path=$path" | Add-Content -LiteralPath $env:GITHUB_OUTPUT
