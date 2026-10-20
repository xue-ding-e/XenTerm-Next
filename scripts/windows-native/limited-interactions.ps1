# Dot-sourced by smoke.ps1 only for an explicit opt-in, using the same owned app process.
# Fixed three flows, not a general-purpose input runner. No connection submission.
function Invoke-LimitedInteractions([object]$OwnedProcess,[string]$OutputDir) {
    Add-Type -Path (Join-Path $PSScriptRoot 'LimitedInput.cs') -ReferencedAssemblies System.dll,System.Core.dll,System.Drawing.dll
    $result=[ordered]@{status='blocked';scope='visible_client_only_not_full_window';full_window_verified=$false;semantic_status='requires_visual_review';reason=$null;flows=@();steps=@();screenshots=@();input_cleanup='not_started'}
    $layoutBounds=$null; $reference=$null; $injector=$null; $handle=[IntPtr]::Zero; $frame=$null; $after=$null
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
        $referencePath=Join-Path $PSScriptRoot 'trusted-home-03.png'
        if ((Get-FileHash -LiteralPath $referencePath -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'e1a94d5ac2ab5e3f317b1b5bd342a3419f83b9291a504fac0604a3776ec28de4') { throw 'Reviewed home reference changed' }
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
        $beforeMeasure=Measure-Sidebar $frame 275 'sidebar-before'
        # 1. Actual left-button drag; one planned outward/back round-trip, never retries.
        Assert-ActionFresh $frame; $injector.Drag(($origin.X+275),($origin.Y+500),($origin.X+355))
        $frame.Dispose(); $frame=$null; Start-Sleep -Milliseconds 350
        $frame=Read-FreshFrame '01-sidebar-expanded-released'; Assert-Nav $frame 20
        $expanded=Measure-Sidebar $frame 355 'sidebar-expanded-released'
        Assert-ActionFresh $frame; $injector.Hover(($origin.X+395),($origin.Y+500))
        $frame.Dispose(); $frame=$null; Start-Sleep -Milliseconds 350
        $frame=Read-FreshFrame '02-sidebar-after-release-hover'; Assert-Nav $frame 20
        $hovered=Measure-Sidebar $frame 355 'sidebar-after-release-hover'
        if ($hovered.outer_edge -ne $expanded.outer_edge -or $hovered.separator_73_right -ne $expanded.separator_73_right -or $hovered.separator_174_right -ne $expanded.separator_174_right) { throw 'Sidebar still changed after release and pure hover' }
        Assert-ActionFresh $frame; $injector.Drag(($origin.X+355),($origin.Y+500),($origin.X+275))
        $frame.Dispose(); $frame=$null; Start-Sleep -Milliseconds 350
        $frame=Read-FreshFrame '03-sidebar-restored'; Assert-Home $frame; Assert-Nav $frame 20
        $restored=Measure-Sidebar $frame 275 'sidebar-restored'
        $result.flows+=@([ordered]@{name='sidebar_real_drag_release';status='structural_assertions_passed_needs_visual_review';before=$beforeMeasure;expanded=$expanded;released_hover=$hovered;restored=$restored})
        # 2. Exactly three known rail clicks. No settings mutation/list action.
        foreach ($stage in @(@{name='settings';from=20;to=84},@{name='sessions';from=84;to=52},@{name='terminal';from=52;to=20})) {
            # Do not reuse a prior screenshot to authorize the next page action.
            $frame.Dispose(); $frame=$null
            $frame=Read-FreshFrame ('04-before-'+$stage.name); Assert-Nav $frame $stage.from
            if ($stage.from -eq 20) { Assert-Home $frame }
            Assert-ActionFresh $frame; $injector.Click(($origin.X+27),($origin.Y+$stage.to))
            Start-Sleep -Milliseconds 400
            $after=Read-FreshFrame ('05-after-'+$stage.name); Assert-Nav $after $stage.to
            $changed=[XenTermNativeProbe.LimitedPixels]::Changed($frame.Image,$after.Image,[Drawing.Rectangle]::new(290,120,700,520))
            if ($changed -lt 0.015) { throw 'Navigation did not visibly change the main client content' }
            $frame.Dispose(); $frame=$after; $after=$null
            $result.flows+=@([ordered]@{name=('navigate_'+$stage.name);status='selection_and_content_change_observed_needs_visual_review';main_changed_fraction=$changed})
        }
        # Clear a possible rail hover tooltip by moving to reviewed blank home content.
        Assert-Nav $frame 20; Assert-ActionFresh $frame; $injector.Hover(($origin.X+900),($origin.Y+650))
        $frame.Dispose(); $frame=$null; Start-Sleep -Milliseconds 300
        $frame=Read-FreshFrame '06-before-ctrl-k'; Assert-Home $frame; Assert-Nav $frame 20
        # 3. Keyboard only: open palette, capture shape, cancel. Never type or confirm.
        Assert-ActionFresh $frame; $injector.QuickConnect()
        $frame.Dispose(); $frame=$null; Start-Sleep -Milliseconds 400
        $frame=Read-FreshFrame '07-palette-before-escape'; Assert-Overlay $frame
        Assert-ActionFresh $frame; $injector.Escape()
        $frame.Dispose(); $frame=$null; Start-Sleep -Milliseconds 400
        $frame=Read-FreshFrame '08-after-escape'; Assert-Home $frame; Assert-Nav $frame 20
        $result.flows+=@([ordered]@{name='ctrl_k_escape_cancel';status='overlay_shape_and_home_restoration_observed_needs_visual_review';no_confirmation_or_text_sent=$true})
        $result.status='needs_visual_review'
        $result.reason='Three bounded native flows completed structural checks. Parent must inspect screenshots; not full-window, text-semantic, connection, or comprehensive Windows QA.'
    } catch { $result.reason=$_.Exception.Message }
    finally {
        if ($frame) { $frame.Dispose() }; if ($after) { $after.Dispose() }
        if ($injector) { $injector.Dispose(); $result.accepted_input_events=$injector.AcceptedEvents; $result.input_cleanup=$injector.CleanupConfirmed; if (-not $injector.CleanupConfirmed) { $result.status='blocked'; $result.reason=$injector.CleanupError } }
        if ($reference) { $reference.Dispose() }
        $result.elapsed_seconds=[Math]::Round($elapsed.Elapsed.TotalSeconds,2)
        Write-Report $result (Join-Path $OutputDir 'interactions.json')
    }
    return $result
}
