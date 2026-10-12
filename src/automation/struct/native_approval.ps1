$ErrorActionPreference = 'Stop'
try {
    [Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false)
    [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
    $request = ConvertFrom-Json -InputObject ([Console]::In.ReadToEnd())
    # Some automation hosts omit WINDIR; WPF needs it to find Windows fonts.
    if ([string]::IsNullOrEmpty($env:WINDIR)) { $env:WINDIR = $env:SystemRoot }
    Add-Type -AssemblyName PresentationFramework
    Add-Type -AssemblyName PresentationCore
    Add-Type -AssemblyName WindowsBase
    if (-not [Environment]::UserInteractive) { exit 2 }
    $deadline = [DateTimeOffset]::FromUnixTimeMilliseconds([long]$request.created_at_ms).AddSeconds([double]$request.wait_timeout_secs)
    if ([DateTimeOffset]::UtcNow -ge $deadline) { exit 2 }
    # XAML is static. Untrusted commands and reasons are assigned only as text.
    [xml]$markup = @'
<Window xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        Title="XenTerm 风险操作审批" Width="640" SizeToContent="Height"
        MaxHeight="740" WindowStartupLocation="CenterScreen" ResizeMode="NoResize"
        Background="{DynamicResource Background}" Foreground="{DynamicResource Foreground}"
        FontFamily="Segoe UI, Microsoft YaHei UI" FontSize="13" Topmost="True">
  <Window.Resources>
    <SolidColorBrush x:Key="Background" Color="#171717"/>
    <SolidColorBrush x:Key="Surface" Color="#262626"/>
    <SolidColorBrush x:Key="Border" Color="#404040"/>
    <SolidColorBrush x:Key="Foreground" Color="#FAFAFA"/>
    <SolidColorBrush x:Key="Muted" Color="#A3A3A3"/>
    <SolidColorBrush x:Key="Accent" Color="#EF4444"/>
    <Style TargetType="Button">
      <Setter Property="Height" Value="36"/><Setter Property="MinWidth" Value="100"/>
      <Setter Property="Padding" Value="16,0"/><Setter Property="Foreground" Value="{DynamicResource Foreground}"/>
      <Setter Property="Background" Value="{DynamicResource Surface}"/>
      <Setter Property="BorderBrush" Value="{DynamicResource Border}"/>
      <Setter Property="Cursor" Value="Hand"/>
      <Setter Property="Template">
        <Setter.Value><ControlTemplate TargetType="Button">
          <Border x:Name="ButtonBorder" Background="{TemplateBinding Background}"
                  BorderBrush="{TemplateBinding BorderBrush}" BorderThickness="1" CornerRadius="7">
            <ContentPresenter HorizontalAlignment="Center" VerticalAlignment="Center"/>
          </Border>
          <ControlTemplate.Triggers>
            <Trigger Property="IsMouseOver" Value="True"><Setter TargetName="ButtonBorder" Property="Opacity" Value="0.88"/></Trigger>
            <Trigger Property="IsKeyboardFocused" Value="True"><Setter TargetName="ButtonBorder" Property="BorderBrush" Value="{DynamicResource Foreground}"/></Trigger>
            <Trigger Property="IsEnabled" Value="False"><Setter Property="Opacity" Value="0.45"/></Trigger>
          </ControlTemplate.Triggers>
        </ControlTemplate></Setter.Value>
      </Setter>
    </Style>
  </Window.Resources>
  <Grid><Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/></Grid.RowDefinitions>
    <StackPanel Margin="24,24,24,20">
      <TextBlock x:Name="Heading" Text="确认本次操作" FontSize="20" FontWeight="SemiBold"/>
      <TextBlock x:Name="Description" Margin="0,8,0,20" TextWrapping="Wrap" Foreground="{DynamicResource Muted}"
                 Text="此操作触及敏感位置。请核对会话和命令后决定是否执行。"/>
      <Border Background="{DynamicResource Surface}" BorderBrush="{DynamicResource Border}"
              BorderThickness="1" CornerRadius="8" Padding="12" Margin="0,0,0,18">
        <StackPanel><TextBlock x:Name="SessionLabel" Text="会话" FontSize="11" Foreground="{DynamicResource Muted}"/>
          <TextBlock x:Name="Session" Margin="0,5,0,0" TextWrapping="Wrap" FontWeight="SemiBold"/>
        </StackPanel>
      </Border>
      <TextBlock x:Name="CommandLabel" Text="命令" Margin="0,0,0,8" Foreground="{DynamicResource Muted}"/>
      <Border Background="{DynamicResource Surface}" BorderBrush="{DynamicResource Border}"
              BorderThickness="1" CornerRadius="8" Padding="12">
        <TextBox x:Name="Command" IsReadOnly="True" AcceptsReturn="True" TextWrapping="Wrap"
                 MinHeight="56" MaxHeight="190" VerticalScrollBarVisibility="Auto" BorderThickness="0"
                 Background="Transparent" Foreground="{DynamicResource Foreground}"
                 FontFamily="Cascadia Mono, Consolas" FontSize="13"/>
      </Border>
      <TextBlock x:Name="ReasonsLabel" Text="风险原因" Margin="0,18,0,8" Foreground="{DynamicResource Muted}"/>
      <ScrollViewer MaxHeight="110" VerticalScrollBarVisibility="Auto">
        <TextBlock x:Name="Reasons" TextWrapping="Wrap"/>
      </ScrollViewer>
    </StackPanel>
    <Border Grid.Row="1" BorderBrush="{DynamicResource Border}" BorderThickness="0,1,0,0" Padding="24,16">
      <Grid><Grid.ColumnDefinitions><ColumnDefinition Width="*"/><ColumnDefinition Width="Auto"/></Grid.ColumnDefinitions>
        <TextBlock x:Name="Countdown" VerticalAlignment="Center" Foreground="{DynamicResource Muted}"/>
        <StackPanel Grid.Column="1" Orientation="Horizontal">
          <Button x:Name="Deny" Content="拒绝" IsDefault="True" IsCancel="True" Margin="0,0,10,0"/>
          <Button x:Name="Approve" Content="允许一次" Background="{DynamicResource Accent}" Foreground="White" BorderBrush="{DynamicResource Accent}"/>
        </StackPanel>
      </Grid>
    </Border>
  </Grid>
</Window>
'@
    $window = [Windows.Markup.XamlReader]::Load([Xml.XmlNodeReader]::new($markup))
    $isEnglish = $request.language -eq 'en'
    $theme = [string]$request.theme
    if ($theme -notin @('dark', 'light')) {
        $light = (Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize' -ErrorAction SilentlyContinue).AppsUseLightTheme
        $theme = if ($light -eq 1) { 'light' } else { 'dark' }
    }
    if ($theme -eq 'light') {
        foreach ($entry in @{Background='#FFFFFF'; Surface='#F5F5F5'; Border='#D4D4D4'; Foreground='#171717'; Muted='#737373'; Accent='#DC2626'}.GetEnumerator()) {
            $window.Resources[$entry.Key] = [Windows.Media.SolidColorBrush]::new([Windows.Media.ColorConverter]::ConvertFromString($entry.Value))
        }
    }
    $window.FindName('Session').Text = [string]$request.session
    $window.FindName('Command').Text = [string]$request.command
    $window.FindName('Reasons').Text = ($request.reasons | ForEach-Object { '• ' + $_ }) -join "`r`n"
    $approve = $window.FindName('Approve')
    $deny = $window.FindName('Deny')
    if ($isEnglish) {
        $window.Title = 'XenTerm Risk approval'
        foreach ($entry in @{Heading='Confirm this operation'; Description='This operation touches a sensitive location. Review the session and command before continuing.'; SessionLabel='Session'; CommandLabel='Command'; ReasonsLabel='Why this was flagged'}.GetEnumerator()) {
            $window.FindName($entry.Key).Text = $entry.Value
        }
        $approve.Content = 'Allow once'; $deny.Content = 'Deny'
    }
    # Default focus and Enter refuse. Escape/title-bar close also refuse.
    $window.Add_ContentRendered({ [void]$deny.Focus() })
    $approve.Add_Click({ if ([DateTimeOffset]::UtcNow -lt $deadline) { $window.DialogResult = $true } })
    $timer = [Windows.Threading.DispatcherTimer]::new()
    $timer.Interval = [TimeSpan]::FromMilliseconds(200)
    $timer.Add_Tick({
        $left = [Math]::Max(0, [Math]::Ceiling(($deadline - [DateTimeOffset]::UtcNow).TotalSeconds))
        $window.FindName('Countdown').Text = if ($isEnglish) { "Refuses on timeout · ${left}s" } else { "超时后拒绝 · 剩余 ${left}s" }
        if ($left -le 0) { $approve.IsEnabled = $false; $window.DialogResult = $false }
    })
    $timer.Start()
    $result = $window.ShowDialog()
    $timer.Stop()
    if ($result -eq $true -and [DateTimeOffset]::UtcNow -lt $deadline) { exit 0 }
    exit 1
} catch {
    # A missing interactive desktop or a failed UI never grants permission.
    $failure = $_.Exception
    while ($failure) {
        [Console]::Error.WriteLine($failure.Message)
        $failure = $failure.InnerException
    }
    exit 2
}
