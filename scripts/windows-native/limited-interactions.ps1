# Dot-sourced by smoke.ps1 only for an explicit opt-in, using the same owned app process.
# Three reviewed navigation clicks, then Ctrl+K observation only; no Enter, data input or Escape.
function Invoke-LimitedInteractions([object]$OwnedProcess,[string]$OutputDir) {
    Add-Type -Path (Join-Path $PSScriptRoot 'LimitedInput.cs') -ReferencedAssemblies System.dll,System.Core.dll,System.Drawing.dll
    $result=[ordered]@{status='blocked';scope='visible_client_only_not_full_window';full_window_verified=$false;semantic_status='requires_visual_review';reason=$null;flows=@();steps=@();screenshots=@();input_cleanup='not_started'}
    $layoutBounds=$null; $sessionsReference=$null; $settingsReference=$null; $reference=$null; $injector=$null; $handle=[IntPtr]::Zero; $frame=$null; $after=$null
    $elapsed=[Diagnostics.Stopwatch]::StartNew()
    function Assert-Geometry($capture) {
        $o=$capture.OriginalBounds; $v=$capture.VisibleBounds
        if ($layoutBounds -and $o -ne $layoutBounds) { throw 'App window moved after the reviewed first frame' }
        if ([XenTermNativeProbe.Desktop]::WindowClass($handle) -ne 'Zed::Window' -or [XenTermNativeProbe.Desktop]::GetDpiForWindow($handle) -ne 96 -or
            $o.Width -ne 1100 -or $o.Height -ne 720 -or $v.Width -ne 1024 -or $v.Height -ne 708 -or $o.Left -ne $v.Left -or $o.Top -ne $v.Top) {
            throw 'Layout/DPI differs from reviewed actual screenshot; no scaled or guessed input coordinates'
        }
    }
    function Read-FreshFrame([string]$name,[bool]$save=$true) {
        if ($OwnedProcess.HasExited) { throw 'Owned app exited' }
        if ($injector) { $injector.AssertForeground() }
        [XenTermNativeProbe.LimitedInput]::RequireForeground($handle,$OwnedProcess.Pid)
        $capture=[XenTermNativeProbe.Desktop]::CaptureVisibleClient($handle,$OwnedProcess.Pid)
        try {
            Assert-Geometry $capture
            [XenTermNativeProbe.LimitedInput]::RequireForeground($handle,$OwnedProcess.Pid)
            $capture.Image.Tag=[DateTime]::UtcNow
            if ($save) {
                $file=$name+'.png'
                $capture.Image.Save((Join-Path $OutputDir $file),[Drawing.Imaging.ImageFormat]::Png)
                $result.screenshots+=@([ordered]@{file=$file;elapsed_seconds=[Math]::Round($elapsed.Elapsed.TotalSeconds,2);window_handle=$handle.ToInt64();pid=$OwnedProcess.Pid;foreground_pid_verified=$true;window_dpi=96;original_client_bounds=$capture.OriginalBounds;visible_client_bounds=$capture.VisibleBounds;desktop_bounds=$capture.DesktopBounds;work_area_bounds=$capture.WorkAreaBounds;partial=$capture.Partial;full_window_verified=$false})
            }
            return $capture
        } catch { $capture.Dispose(); throw }
    }
    function Test-Home($capture) {
        # Exact published 77ce731 home from run39469301493, visually reviewed.
        # Excludes changing CPU/memory/network values/curves and bottom version text.
        foreach ($r in @([Drawing.Rectangle]::new(5,0,45,104),[Drawing.Rectangle]::new(60,10,200,60),[Drawing.Rectangle]::new(510,305,360,70),[Drawing.Rectangle]::new(570,387,238,34))) {
            if ([XenTermNativeProbe.LimitedPixels]::Match($capture.Image,$reference,$r) -lt 0.98) { return $false }
        }
        return $true
    }
    function Assert-Home($capture) { if (-not (Test-Home $capture)) { throw 'Fresh frame is not the source-bound, reviewed home layout' } }
    function Assert-Nav($capture,[int]$selected) {
        foreach ($y in @(20,52,84)) {
            foreach ($x in @(16,35)) {
                $patch=[Drawing.Rectangle]::new($x,($y-6),3,12)
                $dark=[XenTermNativeProbe.LimitedPixels]::Dark($capture.Image,$patch)
                $light=[XenTermNativeProbe.LimitedPixels]::Light($capture.Image,$patch)
                if (($y -eq $selected -and $dark -lt 0.95) -or ($y -ne $selected -and $light -lt 0.95)) { throw 'Fresh icon-free navigation patch is not the expected stage' }
            }
        }
        # Known rail divider must remain at x55, independent of the body page.
        if ([XenTermNativeProbe.LimitedPixels]::Match($capture.Image,$reference,[Drawing.Rectangle]::new(45,110,12,550)) -lt 0.98) { throw 'Navigation rail geometry changed' }
    }
    function Assert-Settings($capture) {
        # Fresh stable text/menu regions from the reviewed actual release Settings page.
        foreach ($r in @([Drawing.Rectangle]::new(65,8,150,302),[Drawing.Rectangle]::new(240,15,560,350))) {
            if ([XenTermNativeProbe.LimitedPixels]::Match($capture.Image,$settingsReference,$r) -lt 0.98) { throw 'Fresh page does not match reviewed release Settings; no Sessions input' }
        }
        Assert-Nav $capture 84
    }
    function Assert-Sessions($capture) {
        foreach ($r in @([Drawing.Rectangle]::new(65,8,350,270),[Drawing.Rectangle]::new(465,35,510,350))) {
            if ([XenTermNativeProbe.LimitedPixels]::Match($capture.Image,$sessionsReference,$r) -lt 0.98) { throw 'Fresh page differs from reviewed Sessions; no return-home input' }
        }
        Assert-Nav $capture 52
    }
    function Assert-ActionFresh($capture) {
        if (([DateTime]::UtcNow-[DateTime]$capture.Image.Tag).TotalSeconds -gt 1) { throw 'Input authorization screenshot is older than one second' }
        if ([XenTermNativeProbe.Desktop]::ClientBounds($handle) -ne $capture.OriginalBounds) { throw 'App moved after the fresh input screenshot' }
        $injector.AssertForeground()
    }
    function Measure-Sidebar($capture,[int]$expected,[string]$name) {
        $measurement=[ordered]@{name=$name;status='measuring';expected_outer=$expected;expected_inner=($expected-8);outer_edge=$null;separator_73_right=$null;separator_174_right=$null;reason=$null}
        $result.steps+=@($measurement)
        try {
            $measurement.outer_edge=[XenTermNativeProbe.LimitedPixels]::OuterEdge($capture.Image,$expected)
            $inside=[XenTermNativeProbe.LimitedPixels]::InnerEdges($capture.Image,$measurement.outer_edge)
            $measurement.separator_73_right=$inside[0]; $measurement.separator_174_right=$inside[1]
            $measurement.status='passed'
        } catch { $measurement.status='failed'; $measurement.reason=$_.Exception.Message; throw }
        finally { Write-Report $result (Join-Path $OutputDir 'interactions.json') }
        return $measurement
    }
    function Assert-Overlay($capture) {
        # Source pins width=420 and nominal centered top=88 (client=1100x720).
        # Conservative geometry only, not OCR or proof of palette semantics.
        $inside=[XenTermNativeProbe.LimitedPixels]::Light($capture.Image,[Drawing.Rectangle]::new(350,150,400,420))
        $outside=[XenTermNativeProbe.LimitedPixels]::Light($capture.Image,[Drawing.Rectangle]::new(800,150,160,420))
        if ($inside -lt 0.65 -or $outside -gt 0.35 -or ($inside-$outside) -lt 0.40) { throw 'Quick-connect overlay shape was not established; do not send Esc blindly' }
    }
    try {
        $referencePath=Join-Path $PSScriptRoot 'reviewed-release-home-77ce731.png'
        if ((Get-FileHash -LiteralPath $referencePath -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'd81a84f4c49e331969196c9bd859ab6435511e3cb323f656b3fa67e95b67a7b6') { throw 'Reviewed home reference changed' }
        $settingsPath=Join-Path $PSScriptRoot 'reviewed-release-settings-77ce731.png'
        if ((Get-FileHash -LiteralPath $settingsPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne '1a536d736ad9fe4224ecd2643494469ef152bfc9a50c8993c83071a26fdb0eb8') { throw 'Reviewed release Settings reference changed' }
        $sessionsPath=Join-Path $PSScriptRoot 'reviewed-release-sessions-77ce731.png'
        if ((Get-FileHash -LiteralPath $sessionsPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'd7f99db30da9ac79d2a195a079c419fedd18efb9932201ae05c3835246201db4') { throw 'Reviewed release Sessions reference changed' }
        $sessionsReference=[Drawing.Bitmap]::new($sessionsPath)
        $settingsReference=[Drawing.Bitmap]::new($settingsPath)
        $reference=[Drawing.Bitmap]::new($referencePath)
        # Read-only first-frame wait, at most 20 seconds. Never save/map the rich
        # but unrendered host-background frame 01 as a trusted application frame.
        $stable=0
        while ($elapsed.Elapsed.TotalSeconds -lt 20 -and $stable -lt 2) {
            if ($OwnedProcess.HasExited) { throw 'Owned app exited before trusted first frame' }
            $targets=@([XenTermNativeProbe.Desktop]::WindowsForProcess($OwnedProcess.Pid) | Where-Object { [XenTermNativeProbe.Desktop]::WindowClass([IntPtr]$_) -eq 'Zed::Window' })
            if ($targets.Count -eq 0) { Start-Sleep -Milliseconds 300; continue }
            if ($targets.Count -ne 1) { throw 'Ambiguous application HWND' }
            $handle=[IntPtr]$targets[0]
            $frame=Read-FreshFrame 'unsaved-first-frame' $false
            if (Test-Home $frame) { $stable++ } else { $stable=0 }
            $frame.Dispose(); $frame=$null
            Start-Sleep -Milliseconds 300
        }
        if ($stable -ne 2) { throw 'No two fresh trusted home frames within 20 seconds; no input sent' }
        $frame=Read-FreshFrame '00-trusted-home'; Assert-Home $frame; Assert-Nav $frame 20
        $injector=[XenTermNativeProbe.LimitedInput]::new($handle,$OwnedProcess.Pid,$frame.VisibleBounds,$frame.DesktopBounds)
        $layoutBounds=$frame.OriginalBounds
        $origin=$frame.OriginalBounds.Location
        function Record-Phase([string]$phase,[int]$targetY=84) {
            $result.steps+=@([ordered]@{name=$phase;evidence=$injector.DiagnosticSnapshot(($origin.X+27),($origin.Y+$targetY))})
            Write-Report $result (Join-Path $OutputDir 'interactions.json')
        }
        # One same-coordinate click, with evidence between input phases. Never retries.
        Record-Phase 'before-move'
        Assert-ActionFresh $frame; $injector.Hover(($origin.X+27),($origin.Y+84))
        $frame.Dispose(); $frame=$null; Start-Sleep -Milliseconds 120
        $frame=Read-FreshFrame '01-settings-hover'; Assert-Nav $frame 20
        Record-Phase 'after-move'
        Assert-ActionFresh $frame
        $injector.PressDiagnostic(($origin.X+27),($origin.Y+84))
        Start-Sleep -Milliseconds 120
        $frame.Dispose(); $frame=$null
        $frame=Read-FreshFrame '02-settings-held'
        Record-Phase 'after-down'
        $result.steps+=@([ordered]@{name='left-held';observed=$injector.DiagnosticLeftHeld()})
        Write-Report $result (Join-Path $OutputDir 'interactions.json')
        Assert-ActionFresh $frame; $injector.ReleaseDiagnostic()
        Record-Phase 'after-up'
        $observed=$false
        foreach ($sample in 0..4) {
            $frame.Dispose(); $frame=$null
            if ($sample -gt 0) { Start-Sleep -Milliseconds 250 }
            $frame=Read-FreshFrame ('03-settings-released-'+$sample)
            $selected=$false
            try { Assert-Nav $frame 84; $selected=$true } catch { }
            $result.steps+=@([ordered]@{name=('released-'+$sample);settings_selection_observed=$selected})
            Write-Report $result (Join-Path $OutputDir 'interactions.json')
            if($selected) { $observed=$true; break }
        }
        if(-not $observed) { throw 'One diagnostic click did not establish settings selection within one second; no retry or later input' }
        Assert-Settings $frame
        $result.flows+=@([ordered]@{name='home-to-settings';status='reviewed-page-regions-matched';semantic_status='requires_visual_review'})
        # Exactly one further click on the visible Sessions icon (client x27/y52).
        # The resulting page must match the separately reviewed Sessions reference before any further input.
        Record-Phase 'sessions-before-move' 52
        Assert-ActionFresh $frame; $injector.Hover(($origin.X+27),($origin.Y+52))
        $frame.Dispose(); $frame=$null; Start-Sleep -Milliseconds 120
        $frame=Read-FreshFrame '04-sessions-hover'; Assert-Settings $frame
        Record-Phase 'sessions-after-move' 52
        Assert-ActionFresh $frame; $injector.PressDiagnostic(($origin.X+27),($origin.Y+52))
        Start-Sleep -Milliseconds 120
        $frame.Dispose(); $frame=$null; $frame=Read-FreshFrame '05-sessions-held'
        Record-Phase 'sessions-after-down' 52
        $result.steps+=@([ordered]@{name='sessions-left-held';observed=$injector.DiagnosticLeftHeld()})
        Write-Report $result (Join-Path $OutputDir 'interactions.json')
        Assert-ActionFresh $frame; $injector.ReleaseDiagnostic()
        Record-Phase 'sessions-after-up' 52
        $sessionsObserved=$false
        foreach ($sample in 0..4) {
            $frame.Dispose(); $frame=$null
            if ($sample -gt 0) { Start-Sleep -Milliseconds 250 }
            $frame=Read-FreshFrame ('06-sessions-released-'+$sample)
            $selected=$false
            try { Assert-Nav $frame 52; $selected=$true } catch { }
            $result.steps+=@([ordered]@{name=('sessions-released-'+$sample);sessions_selection_observed=$selected})
            Write-Report $result (Join-Path $OutputDir 'interactions.json')
            if ($selected) { $sessionsObserved=$true; break }
        }
        if (-not $sessionsObserved) { throw 'One Sessions click did not establish selection within one second; stop without retry' }
        Assert-Sessions $frame
        $result.flows+=@([ordered]@{name='settings-to-sessions';status='reviewed-page-regions-matched';semantic_status='requires_visual_review'})
        # One return-home click on the reviewed rail, with the same fresh/owned guards.
        Record-Phase 'terminal-before-move' 20
        Assert-ActionFresh $frame; $injector.Hover(($origin.X+27),($origin.Y+20))
        $frame.Dispose(); $frame=$null; Start-Sleep -Milliseconds 120
        $frame=Read-FreshFrame '07-terminal-hover'; Assert-Sessions $frame
        Assert-ActionFresh $frame; $injector.PressDiagnostic(($origin.X+27),($origin.Y+20))
        Start-Sleep -Milliseconds 120
        $frame.Dispose(); $frame=$null; $frame=Read-FreshFrame '08-terminal-held'
        Record-Phase 'terminal-after-down' 20
        Assert-ActionFresh $frame; $injector.ReleaseDiagnostic()
        Record-Phase 'terminal-after-up' 20
        $homeStable=0
        foreach ($sample in 0..7) {
            $frame.Dispose(); $frame=$null
            if ($sample -gt 0) { Start-Sleep -Milliseconds 250 }
            $frame=Read-FreshFrame ('09-returned-home-'+$sample)
            try { Assert-Home $frame; Assert-Nav $frame 20; $homeStable++ } catch { $homeStable=0 }
            if ($homeStable -ge 2) { break }
        }
        if ($homeStable -lt 2) { throw 'Return-home click did not yield two reviewed home frames; no keyboard input' }
        $result.flows+=@([ordered]@{name='sessions-to-home';status='two-reviewed-home-frames-matched';semantic_status='requires_visual_review'})
        # Ctrl down, K down, K up, Ctrl up only. Never types text, Enter, or selects a connection.
        Assert-ActionFresh $frame; $injector.QuickConnect()
        Record-Phase 'after-ctrl-k' 20
        $overlayObserved=$false
        foreach ($sample in 0..4) {
            $frame.Dispose(); $frame=$null
            if ($sample -gt 0) { Start-Sleep -Milliseconds 250 }
            $frame=Read-FreshFrame ('10-quick-connect-'+$sample)
            $shape=$false
            try { Assert-Overlay $frame; $shape=$true; $overlayObserved=$true } catch { }
            $result.steps+=@([ordered]@{name=('quick-connect-'+$sample);overlay_shape_observed=$shape})
            Write-Report $result (Join-Path $OutputDir 'interactions.json')
        }
        # No Escape in this increment: shape alone is not a reviewed semantic overlay.
        if (-not $overlayObserved) { throw 'Ctrl+K overlay shape not established; screenshots saved, no retry or Escape' }
        $result.status='needs_visual_review'
        $result.reason='Three navigation clicks and Ctrl+K only; inspect the actual palette. No text, Enter, connection selection, Escape or subsequent input.'

    } catch { $result.reason=$_.Exception.Message }
    finally {
        if ($frame) { $frame.Dispose() }; if ($after) { $after.Dispose() }
        if ($injector) { $injector.Dispose(); $result.accepted_input_events=$injector.AcceptedEvents; $result.input_cleanup=$injector.CleanupConfirmed; if (-not $injector.CleanupConfirmed) { $result.status='blocked'; $result.reason=$injector.CleanupError } }
        if ($reference) { $reference.Dispose() }; if ($settingsReference) { $settingsReference.Dispose() }; if ($sessionsReference) { $sessionsReference.Dispose() }
        $result.elapsed_seconds=[Math]::Round($elapsed.Elapsed.TotalSeconds,2)
        Write-Report $result (Join-Path $OutputDir 'interactions.json')
    }
    return $result
}
