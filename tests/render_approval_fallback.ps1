param([Parameter(Mandatory=$true)][string]$OutputPath, [switch]$Light)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName PresentationFramework
Add-Type -AssemblyName PresentationCore
Add-Type -AssemblyName WindowsBase
$source = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\src\automation\struct\native_approval.ps1') -Raw -Encoding UTF8
$match = [regex]::Match($source, "(?s)\[xml\]\`$markup = @'\r?\n(.*?)\r?\n'@")
if (-not $match.Success) { throw 'Static approval XAML was not found' }
[xml]$markup = $match.Groups[1].Value
$window = [Windows.Markup.XamlReader]::Load([Xml.XmlNodeReader]::new($markup))
if ($Light) {
    foreach ($entry in @{Background='#FFFFFF'; Surface='#F5F5F5'; Border='#D4D4D4'; Foreground='#171717'; Muted='#737373'; Accent='#DC2626'}.GetEnumerator()) {
        $window.Resources[$entry.Key] = [Windows.Media.SolidColorBrush]::new([Windows.Media.ColorConverter]::ConvertFromString($entry.Value))
    }
}
$window.FindName('Session').Text = '演示服务器 · operator@192.0.2.10'
$window.FindName('Command').Text = 'cat /etc/os-release'
$window.FindName('Reasons').Text = '• 命令涉及高风险目录：/etc'
$window.FindName('Countdown').Text = '超时后拒绝 · 剩余 60s'
$root = $window.Content
$root.Background = $window.Background
$root.Measure([Windows.Size]::new(640, [double]::PositiveInfinity))
$root.Arrange([Windows.Rect]::new(0, 0, 640, $root.DesiredSize.Height))
$root.UpdateLayout()
$bitmap = [Windows.Media.Imaging.RenderTargetBitmap]::new(640, [int][Math]::Ceiling($root.ActualHeight), 96, 96, [Windows.Media.PixelFormats]::Pbgra32)
$bitmap.Render($root)
$encoder = [Windows.Media.Imaging.PngBitmapEncoder]::new()
$encoder.Frames.Add([Windows.Media.Imaging.BitmapFrame]::Create($bitmap))
$stream = [IO.File]::Create($OutputPath)
try { $encoder.Save($stream) } finally { $stream.Dispose() }
# Rendering is offscreen. No request, decision, native window or SSH command is created.
