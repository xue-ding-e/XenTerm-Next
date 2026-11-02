# Synthetic protection probes, NOT actual upstream/portable/Flutter application
# acceptance. No Flutter code or binary is loaded. The real GPUI GUI is tested
# separately by test_next_gpui_msi_e2e.ps1. Never run on a user computer.
$script:ProtectedSentinels = [Collections.Generic.List[object]]::new()
$script:OwnedSentinelPaths = [Collections.Generic.List[string]]::new()

function New-ProtectedProductSentinels([string]$EvidenceDir) {
 if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
  throw 'Synthetic protection fixtures require a disposable hosted runner'
 }
 $root = Join-Path $env:RUNNER_TEMP ('gpui-protected-products-' + [guid]::NewGuid().ToString('N'))
 $products = @(
  @{name='upstream'; directory=(Join-Path $env:ProgramFiles 'XenTerm'); profile=(Join-Path $env:LOCALAPPDATA 'XenTerm'); registry='HKCU:\Software\xenterm'; shortcut='XenTerm'},
  @{name='portable'; directory=(Join-Path $root 'portable'); profile=(Join-Path $root 'portable\config'); registry='HKCU:\Software\XenTerm-Portable-Installer-Sentinel'; shortcut='XenTerm Portable Sentinel'},
  @{name='flutter'; directory=(Join-Path $env:LOCALAPPDATA 'Programs\XenTerm-Next Flutter'); profile=(Join-Path $env:LOCALAPPDATA 'XenTerm-Next\Flutter'); registry='HKCU:\Software\XenTerm-Next\Flutter'; shortcut='XenTerm-Next Flutter'}
 )
 foreach ($product in $products) {
  $product.links = @(
   (Join-Path ([Environment]::GetFolderPath('Programs')) ($product.shortcut + '.lnk')),
   (Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) ($product.shortcut + '.lnk'))
  )
  foreach ($path in @($product.directory,$product.profile,$product.registry) + $product.links) {
   if (Test-Path -LiteralPath $path) { throw "Refusing preexisting other-product fixture location: $path" }
  }
 }
 New-Item -ItemType Directory -Path $root | Out-Null
 $script:OwnedSentinelPaths.Add($root)
 # A tiny native window responds to WM_CLOSE. A console/sleep-only process
 # would miss name-based CloseApplication regressions and give false assurance.
 $source = Join-Path $root 'sentinel.c'
 @'
#include <windows.h>
static LRESULT CALLBACK sentinel_proc(HWND window, UINT message, WPARAM w, LPARAM l) {
 if (message == WM_DESTROY) { PostQuitMessage(0); return 0; }
 return DefWindowProcW(window, message, w, l);
}
int WINAPI wWinMain(HINSTANCE instance, HINSTANCE prior, PWSTR command, int show) {
 WNDCLASSW kind = {0}; kind.lpfnWndProc = sentinel_proc; kind.hInstance = instance;
 kind.lpszClassName = L"SyntheticInstallerProtectionSentinel";
 if (!RegisterClassW(&kind)) return 2;
 HWND window = CreateWindowW(kind.lpszClassName, L"SYNTHETIC installer sentinel - not XenTerm",
  WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT, 420, 160, NULL, NULL, instance, NULL);
 if (!window) return 3;
 ShowWindow(window, SW_SHOWNOACTIVATE);
 MSG message; while (GetMessageW(&message, NULL, 0, 0) > 0) {
  TranslateMessage(&message); DispatchMessageW(&message);
 }
 return 0;
}
'@ | Set-Content -LiteralPath $source -Encoding utf8NoBOM
 $executable = Join-Path $root 'sentinel.exe'
 & cl /nologo /O1 /MT "/Fe:$executable" "/Fo:$root/sentinel.obj" $source /link /SUBSYSTEM:WINDOWS user32.lib 2>&1 | Write-Host
 if ($LASTEXITCODE -ne 0 -or !(Test-Path -LiteralPath $executable)) { throw 'Synthetic native sentinel build failed' }
 foreach ($product in $products) {
  foreach ($directory in @($product.directory,$product.profile)) {
   if (!(Test-Path -LiteralPath $directory)) {
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    $script:OwnedSentinelPaths.Add($directory)
   }
  }
  $product.exe = Join-Path $product.directory 'xenterm.exe'
  Copy-Item -LiteralPath $executable -Destination $product.exe
  $product.marker = Join-Path $product.profile 'protected-product.fixture'
  $product.name | Set-Content -LiteralPath $product.marker -Encoding utf8NoBOM
  New-Item -Path $product.registry -Force | Out-Null
  $script:OwnedSentinelPaths.Add($product.registry)
  $product.value = [guid]::NewGuid().ToString('N')
  New-ItemProperty -LiteralPath $product.registry -Name 'SyntheticInstallerSentinel' -Value $product.value -PropertyType String | Out-Null
  $shell = New-Object -ComObject WScript.Shell
  foreach ($link in $product.links) {
   $shortcut = $shell.CreateShortcut($link); $shortcut.TargetPath = $product.exe
   $shortcut.Description = 'Synthetic installer protection fixture, not a product shortcut'
   $shortcut.Save(); $script:OwnedSentinelPaths.Add($link)
  }
  [Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) | Out-Null
  $product.hashes = @{}
  foreach ($path in @($product.exe,$product.marker) + $product.links) {
   $product.hashes[$path] = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
  }
  $product.process = Start-Process -FilePath $product.exe -PassThru
  $script:ProtectedSentinels.Add($product)
  for ($attempt = 0; $attempt -lt 30; $attempt++) {
   Start-Sleep -Milliseconds 200; $product.process.Refresh()
   if ($product.process.HasExited) { throw 'Synthetic sentinel exited before installation' }
   if ($product.process.MainWindowHandle -ne 0) { break }
  }
  if ($product.process.MainWindowHandle -eq 0) { throw 'Synthetic sentinel did not create its closeable native window' }
 }
 @{
  coverage='synthetic files, HKCU registry values, shortcuts and independently located same-name closeable native processes'
  real_other_product_gui_tested=$false
  real_gpui_gui_tested_separately=$true
  machine_upstream_registry_tested=$false
  products=@($products | ForEach-Object { @{product=$_.name; executable=$_.exe; profile=$_.profile; registry=$_.registry; process_id=$_.process.Id} })
 } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $EvidenceDir 'synthetic-protection-scope.json') -Encoding utf8NoBOM
 Assert-ProtectedProductSentinels 'before-first-install'
}

function Assert-ProtectedProductSentinels([string]$Operation) {
 foreach ($product in $script:ProtectedSentinels) {
  $product.process.Refresh()
  if ($product.process.HasExited -or $product.process.MainWindowHandle -eq 0 -or $product.process.Path -ine $product.exe) {
   throw "GPUI installer closed another product's synthetic same-name window during $Operation"
  }
  foreach ($path in $product.hashes.Keys) {
   if (!(Test-Path -LiteralPath $path) -or (Get-FileHash -LiteralPath $path).Hash -ne $product.hashes[$path]) {
    throw "GPUI installer changed protected synthetic file/shortcut during ${Operation}: $path"
   }
  }
  $key = Get-Item -LiteralPath $product.registry
  if (@($key.GetValueNames()).Count -ne 1 -or @($key.GetSubKeyNames()).Count -ne 0 -or
      $key.GetValueKind('SyntheticInstallerSentinel').ToString() -ne 'String' -or
      $key.GetValue('SyntheticInstallerSentinel') -cne $product.value) {
   throw "GPUI installer changed protected synthetic registry during $Operation"
  }
 }
}

function Remove-OwnedProductSentinels {
 foreach ($product in $script:ProtectedSentinels) {
  if (!$product.process.HasExited) {
   # Exact tracked PID/path only; never terminate processes by shared name.
   if ($product.process.Path -ine $product.exe) { throw 'Synthetic cleanup process identity changed' }
   [void]$product.process.CloseMainWindow()
   if (!$product.process.WaitForExit(5000)) { $product.process.Kill(); $product.process.WaitForExit() }
  }
 }
 for ($i = $script:OwnedSentinelPaths.Count - 1; $i -ge 0; $i--) {
  $path = $script:OwnedSentinelPaths[$i]
  if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Recurse -Force }
 }
}
