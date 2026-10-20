# Read-only diagnostics for the verified existing executable. Never loads a DLL,
# installs a tool, changes PATH, rebuilds, or traverses unrelated host data.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$dir=Join-Path $env:RUNNER_TEMP 'xenterm-exact-reuse'
$exe=Join-Path $dir 'xenterm.exe'
$identity=Get-Content -Raw -LiteralPath "$env:RUNNER_TEMP/native-evidence/reuse-identity.json" | ConvertFrom-Json
if ($identity.status -ne 'identity_verified_not_execution_pass' -or $identity.harness_sha -ne $env:GITHUB_SHA -or (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'f6dfc60bb334247f788012ad52c9ad2faa126b48b8df11d2ec6313dc141325e0') { throw 'Dependency probe requires exact verified binary' }
# Producer's known installed Microsoft toolchain; no search/install fallback.
$dumpbin='C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Tools\MSVC\14.51.36231\bin\Hostx64\x64\dumpbin.exe'
if (-not (Test-Path -LiteralPath $dumpbin -PathType Leaf)) { throw 'Known installed dumpbin absent; do not install or search other hosts' }
$lines=@(& $dumpbin /NOLOGO /DEPENDENTS $exe 2>&1)
if ($LASTEXITCODE -ne 0) { throw 'Installed dumpbin failed to inspect this executable' }
$names=@($lines | ForEach-Object { if ([string]$_ -match '^\s+([A-Za-z0-9_.-]+\.dll)\s*$') { $Matches[1] } } | Sort-Object -Unique)
if ($names.Count -lt 1 -or $names.Count -gt 100) { throw 'Unexpected import count' }
# Inspect only exact imported names in normal candidate locations and existing
# PATH directories. Do not list directory contents or publish the whole PATH.
$dirs=@(@($dir,$env:GITHUB_WORKSPACE,(Join-Path $env:WINDIR 'System32'),$env:WINDIR)+@($env:PATH -split ';') | Where-Object { $_ } | Select-Object -Unique)
if ($dirs.Count -gt 100) { throw 'Unexpected search directory count' }
$imports=@()
foreach ($name in $names) {
    $found=@()
    foreach ($base in $dirs) {
        $path=Join-Path -Path ($base.Trim('"')) -ChildPath $name
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            $f=Get-Item -LiteralPath $path
            if ($f.Attributes -band [IO.FileAttributes]::ReparsePoint) { continue }
            $found+=@([ordered]@{path=$f.FullName;bytes=$f.Length;file_version=$f.VersionInfo.FileVersion;product_version=$f.VersionInfo.ProductVersion})
        }
    }
    $imports+=@([ordered]@{name=$name;api_set_contract=($name -match '^(api|ext)-ms-');physical_candidates=$found})
}
$report=[ordered]@{status='read_only_import_inventory_not_loader_proof';binary_sha256=$identity.binary_sha256;harness_sha=$env:GITHUB_SHA;tool=$dumpbin;tool_version=(Get-Item -LiteralPath $dumpbin).VersionInfo.FileVersion;imports=$imports;limitations='Declared dependencies reported by dumpbin /DEPENDENTS only. API sets, KnownDLLs, transitive imports and dynamic LoadLibrary resolution are not modeled. No physical match is not proof of a missing runtime dependency.'}
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath "$env:RUNNER_TEMP/native-evidence/dependencies.json" -Encoding UTF8
# Read only this already hash-verified executable. Extract a unique absolute
# compile-time crate directory; never guess a Cargo home or search the host.
$shader=[ordered]@{status='unknown';reason='No unique absolute crate directory in verified binary';paths=@();root_count=0}
$bytes=[IO.File]::ReadAllBytes($exe)
try {
    $text=[Text.Encoding]::ASCII.GetString($bytes)
    $pattern='[A-Za-z]:[\\/](?:[A-Za-z0-9_. -]{1,128}[\\/]){1,16}gpui-pre-windows-0\.3\.8(?=[^A-Za-z0-9_.-]|$)'
    $regex=[Text.RegularExpressions.Regex]::new($pattern,[Text.RegularExpressions.RegexOptions]::CultureInvariant,[TimeSpan]::FromSeconds(3))
    $roots=@($regex.Matches($text) | ForEach-Object { $_.Value.Replace('/','\') } | Select-Object -Unique)
    $shader.root_count=$roots.Count
    if ($roots.Count -eq 1 -and [IO.Path]::IsPathRooted($roots[0])) {
        $shader.status='fixed_paths_observed_not_failure_cause'; $shader.reason='Unique literal crate directory from exact binary; only three known shader paths checked'
        foreach ($name in @('shaders.hlsl','color_text_raster.hlsl','alpha_correction.hlsl')) {
            $path=Join-Path (Join-Path $roots[0] 'src') $name
            $shader.paths+=@([ordered]@{path=$path;file_exists=(Test-Path -LiteralPath $path -PathType Leaf)})
        }
    }
} catch { $shader.reason='Bounded extraction failed; no host paths checked: '+$_.Exception.GetType().Name }
finally { $text=$null; $bytes=$null }
$shader | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath "$env:RUNNER_TEMP/native-evidence/shader-paths.json" -Encoding UTF8
