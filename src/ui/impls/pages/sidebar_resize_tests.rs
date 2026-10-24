//! Exercise the real workspace resize handlers without opening a transport.
use super::*;
use gpui_kit::gpui::{point, TestAppContext, VisualTestContext};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct TestSink;
impl crate::core::EventSink for TestSink {
    fn deliver(&self, _: &str, _: Vec<crate::session::protocol::SessionEvent>) {}
    fn request_render(&self, _: &str) -> Option<crate::terminal::RenderTicket> { None }
}

fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
}

fn assert_width(page: &Entity<TerminalPage>, cx: &mut VisualTestContext, expected: f32) {
    for frame in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        page.read_with(cx, |page, cx| {
            let sidebar = page.sidebar.read(cx);
            let (left, right) = page.sidebar_edge.get();
            eprintln!("resize frame={frame} expected={expected} shown={} content={} observer={} left={left} right={right}", sidebar.width(), sidebar.content_width(), page.sidebar_width);
        });
    }
    let outer = cx.debug_bounds("resource-sidebar-column").expect("sidebar column is rendered");
    let outer_width = f32::from(outer.size.width);
    page.read_with(cx, |page, cx| {
        let sidebar = page.sidebar.read(cx);
        assert_eq!(sidebar.width(), expected, "resource sidebar must reach requested viewport-fitted width: expected={expected}");
        assert_eq!(sidebar.content_width(), expected, "sidebar content must follow resized column");
        assert_eq!(page.sidebar_width, expected, "workspace observer width");
        let (left, right) = page.sidebar_edge.get();
        // The canvas is inside the column's one-pixel right border. Compare
        // the outer box to the requested width, and the inner box separately.
        assert!((outer_width - expected).abs() < 1.1, "rendered outer column: actual={outer_width}, expected={expected}");
        assert!((right - left - (outer_width - 1.)).abs() < 1.1, "rendered inner canvas: left={left}, right={right}, outer={outer_width}");
    });
}

fn drag_to(page: &Entity<TerminalPage>, cx: &mut VisualTestContext, target: f32) {
    let (right, width) = page.read_with(cx, |page, _| (page.sidebar_edge.get().1, page.sidebar_width));
    let start = point(px(right - 1.), px(100.));
    let end = point(px(right - 1. + target - width), px(100.));
    cx.simulate_mouse_move(start, None, Default::default());
    cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
    page.read_with(cx, |page, _| assert!(page.sidebar_drag.is_some(), "actual resize handle receives pointer"));
    cx.simulate_mouse_move(end, MouseButton::Left, Default::default());
    cx.simulate_mouse_up(end, MouseButton::Left, Default::default());
    draw(cx);
}

fn fixture(cx: &mut TestAppContext) -> (Entity<TerminalPage>, &mut VisualTestContext, std::path::PathBuf) {
    fixture_with_tab(cx, true)
}

fn fixture_with_tab(cx: &mut TestAppContext, with_tab: bool) -> (Entity<TerminalPage>, &mut VisualTestContext, std::path::PathBuf) {
    fixture_with_width(cx, with_tab, 220.)
}

fn fixture_with_width(cx: &mut TestAppContext, with_tab: bool, preferred: f32) -> (Entity<TerminalPage>, &mut VisualTestContext, std::path::PathBuf) {
    cx.update(gpui_kit::init);
    cx.update(crate::ui::actions::init);
    let path = std::env::temp_dir().join(format!("xenterm-sidebar-resize-{}.db", uuid::Uuid::new_v4()));
    let mut store = crate::config::ConfigStore {
        path: path.clone(), backup_dir: None,
        cache: crate::config::ConfigFile::default(), key: [7; 32], keyring_enabled: false,
        saved_state: Mutex::new(crate::config::SavedState::default()).into(),
    };
    // Read the same raw values a legacy/imported profile can contain, rather
    // than passing them through the current setter before construction.
    store.cache.sidebar_width = preferred;
    store.set_sidebar_collapsed(false);
    let state = crate::ui::SessionState::new(
        Arc::new(tokio::runtime::Runtime::new().unwrap()),
        Arc::new(Mutex::new(std::collections::HashMap::new())), Rc::new(RefCell::new(store)),
    );
    let (page, cx) = cx.add_window_view(move |window, cx| {
        let mut page = TerminalPage::new(state, Rc::new(Cell::new(1280.)), window, cx);
        if with_tab {
            let mut tab = TerminalPage::open_tab(&page.state, "resize-fixture", "synthetic-profile", "Resize fixture", page.appearance.clone(), cx);
            PENDING_SINKS.with(|sinks| sinks.borrow_mut().remove("resize-fixture"));
            tab.sink = Arc::new(TestSink);
            page.tabs.push(tab);
            page.active_tab = Some("resize-fixture".into());
            page.panes = crate::layout::Layout::new(vec!["resize-fixture".into()], "resize-fixture".into());
        }
        page
    });
    cx.simulate_resize(gpui_kit::size(px(1280.), px(800.)));
    (page, cx, path)
}

#[gpui_kit::gpui::test]
fn sidebar_drag_keeps_content_aligned_and_restores_resized_width(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture(cx);
    assert_width(&page, cx, 220.);
    for (requested, expected) in [(420., 420.), (180., 180.), (500., 500.), (100., 180.), (310.25, 310.25), (310.75, 310.75), (320., 320.)] {
        drag_to(&page, cx, requested);
        assert_width(&page, cx, expected);
        page.read_with(cx, |page, _| assert_eq!(page.state.store.borrow().sidebar_width(), expected));
    }
    for (collapsed, expected) in [(true, 36.), (false, 320.)] {
        page.update(cx, |page, cx| page.sidebar.update(cx, |sidebar, cx| sidebar.set_collapsed(collapsed, cx)));
        // Production interpolation uses wall-clock Instant; advance both clocks.
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(220));
        cx.executor().advance_clock(Duration::from_millis(32));
        assert_width(&page, cx, expected);
    }
    page.read_with(cx, |page, _| assert!(page.state.handles.borrow().is_empty(), "fixture never starts a transport"));
    let _ = std::fs::remove_file(path);
}

/// The same real pointer path must pass the former fixed cap at every display
/// scale, reserve a usable main pane, and restore the preference after shrink.
#[gpui_kit::gpui::test]
fn sidebar_wide_drag_fits_viewport_and_restores_after_resize(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture(cx);
    for scale in [1.0, 1.5, 2.0] {
        cx.simulate_scale_factor_change(scale);
        cx.simulate_resize(gpui_kit::size(px(1920.), px(900.)));
        draw(cx);
        drag_to(&page, cx, 1100.);
        assert_width(&page, cx, 1100.);
        // This pointer remains inside the window, but requests more than the
        // sidebar's budget. Only the current geometry, not 420px, limits it.
        drag_to(&page, cx, 1800.);
        assert_width(&page, cx, 1584.);
        page.read_with(cx, |page, _| {
            assert_eq!(page.pane_width.get(), 280.);
            assert_eq!(page.state.store.borrow().sidebar_width(), 1584.);
        });
        cx.simulate_resize(gpui_kit::size(px(800.), px(700.)));
        assert_width(&page, cx, 464.);
        page.read_with(cx, |page, _| {
            assert_eq!(page.pane_width.get(), 280.);
            assert!(page.sidebar_edge.get().1 < 800., "divider remains reachable");
            assert_eq!(page.state.store.borrow().sidebar_width(), 1584., "temporary fit must not overwrite the preference");
        });
        cx.simulate_resize(gpui_kit::size(px(1920.), px(900.)));
        assert_width(&page, cx, 1584.);
        drag_to(&page, cx, 100.);
        assert_width(&page, cx, 180.);
    }
    page.read_with(cx, |page, _| assert!(page.state.handles.borrow().is_empty()));
    let _ = std::fs::remove_file(path);
}

#[gpui_kit::gpui::test]
fn sidebar_restores_large_saved_width_and_safely_fits_tiny_window(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture_with_width(cx, false, 1600.);
    assert_width(&page, cx, 944.);
    page.read_with(cx, |page, _| assert_eq!(page.state.store.borrow().sidebar_width(), 1600.));
    cx.simulate_resize(gpui_kit::size(px(2400.), px(900.)));
    assert_width(&page, cx, 1600.);
    cx.simulate_resize(gpui_kit::size(px(300.), px(600.)));
    draw(cx);
    page.read_with(cx, |page, cx| {
        assert_eq!(page.sidebar_width, 0.);
        assert_eq!(page.sidebar.read(cx).content_width(), 0.);
        assert_eq!(page.state.store.borrow().sidebar_width(), 1600.);
    });
    cx.simulate_resize(gpui_kit::size(px(2400.), px(900.)));
    assert_width(&page, cx, 1600.);
    let _ = std::fs::remove_file(path);
}

/// The real shell requests a 1100px minimum width, so normal resizing cannot
/// reach this geometry. Simulate a platform/transient size below that minimum
/// while a drag already exists: the pointer handler does not restart its hit-test.
#[gpui_kit::gpui::test]
fn sidebar_active_drag_through_tiny_viewport_restores_persisted_preference(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture_with_tab(cx, false);
    assert_width(&page, cx, 220.);
    let right = page.read_with(cx, |page, _| page.sidebar_edge.get().1);
    let start = point(px(right - 1.), px(100.));
    cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
    page.read_with(cx, |page, _| assert!(page.sidebar_drag.is_some()));
    cx.simulate_resize(gpui_kit::size(px(300.), px(600.)));
    draw(cx);
    page.read_with(cx, |page, _| {
        assert_eq!(page.sidebar_width, 0.);
        assert!(page.sidebar_drag.is_some(), "shrinking does not cancel the pending gesture");
        assert_eq!(page.state.store.borrow().sidebar_width(), 220.);
    });
    let end = point(px(150.), px(100.));
    cx.simulate_mouse_move(end, MouseButton::Left, Default::default());
    cx.simulate_mouse_up(end, MouseButton::Left, Default::default());
    draw(cx);
    page.read_with(cx, |page, _| {
        assert!(page.sidebar_drag.is_none());
        assert_eq!(page.sidebar_width, 0., "only the fitted display may be zero");
        assert_eq!(page.state.store.borrow().sidebar_width(), 180., "the completed drag retains a valid preference");
    });
    cx.simulate_resize(gpui_kit::size(px(1280.), px(800.)));
    assert_width(&page, cx, 180.);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let settings: String = connection.query_row(
        "SELECT value FROM meta WHERE key='settings'", [], |row| row.get(0),
    ).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(settings["sidebar_width"].as_f64().unwrap(), 180.);
    drop(connection);
    let _ = std::fs::remove_file(path);
}

#[gpui_kit::gpui::test]
fn sidebar_set_width_below_minimum_viewport_keeps_preference_and_display_separate(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture_with_tab(cx, false);
    for (viewport, fitted) in [(300., 0.), (400., 64.)] {
        cx.simulate_resize(gpui_kit::size(px(viewport), px(600.)));
        draw(cx);
        page.update(cx, |page, cx| {
            page.sidebar.update(cx, |sidebar, cx| sidebar.set_width(250., cx));
        });
        draw(cx);
        page.read_with(cx, |page, cx| {
            assert_eq!(page.sidebar.read(cx).width(), fitted);
            assert_eq!(page.sidebar.read(cx).content_width(), fitted);
            assert_eq!(page.state.store.borrow().sidebar_width(), 180.);
        });
        cx.simulate_resize(gpui_kit::size(px(1280.), px(800.)));
        assert_width(&page, cx, 180.);
    }
    let _ = std::fs::remove_file(path);
}

#[gpui_kit::gpui::test]
fn sidebar_legacy_nonfinite_width_and_negative_drag_are_safe(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture_with_width(cx, false, f32::NAN);
    assert_width(&page, cx, 220.);
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        page.update(cx, |page, cx| page.sidebar.update(cx, |sidebar, cx| sidebar.set_width(invalid, cx)));
        assert_width(&page, cx, 220.);
    }
    // Negative overshoot must keep the minimum, not wrap or poison layout.
    page.update(cx, |page, cx| page.sidebar.update(cx, |sidebar, cx| sidebar.set_width(-1000., cx)));
    assert_width(&page, cx, 180.);
    drag_to(&page, cx, 700.);
    assert_width(&page, cx, 700.);
    let _ = std::fs::remove_file(path);
}

#[gpui_kit::gpui::test]
fn sidebar_budget_reserves_only_a_visible_right_dock(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture_with_width(cx, true, 1600.);
    cx.simulate_resize(gpui_kit::size(px(1920.), px(900.)));
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let (commands, _receiver) = tokio::sync::mpsc::unbounded_channel();
    page.update(cx, |page, cx| {
        page.state.sftp_handles.lock().unwrap().insert(
            "resize-fixture".into(),
            crate::sftp::SftpHandle { commands, join: runtime.spawn(async {}) },
        );
        page.state.store.borrow_mut().set_sftp_panel_dock("right".into());
        page.sftp_collapsed = false;
        page.dock_progress.set(1.);
        cx.notify();
    });
    assert_width(&page, cx, 1164.);
    page.read_with(cx, |page, _| assert_eq!(page.pane_width.get(), 280.));
    page.update(cx, |page, cx| {
        page.state.store.borrow_mut().set_sftp_panel_dock("bottom".into());
        cx.notify();
    });
    assert_width(&page, cx, 1584.);
    page.read_with(cx, |page, _| assert_eq!(page.state.store.borrow().sidebar_width(), 1600.));
    page.update(cx, |page, cx| {
        page.state.sftp_handles.lock().unwrap().clear();
        page.state.store.borrow_mut().set_sftp_panel_dock("right".into());
        cx.notify();
    });
    assert_width(&page, cx, 1584.);
    page.read_with(cx, |page, _| assert!(page.state.handles.borrow().is_empty(), "only an inert synthetic SFTP channel was registered"));
    let _ = std::fs::remove_file(path);
}

/// Operating systems can report the next pointer move without delivering the
/// release when the user releases outside the window. That move must end the
/// resize, rather than turning ordinary hovering into a second drag.
#[gpui_kit::gpui::test]
fn sidebar_release_outside_window_does_not_leave_hover_resizing(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture(cx);
    assert_width(&page, cx, 220.);
    let right = page.read_with(cx, |page, _| page.sidebar_edge.get().1);
    let start = point(px(right - 1.), px(100.));
    cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
    cx.simulate_mouse_move(point(start.x + px(80.), start.y), MouseButton::Left, Default::default());
    assert_width(&page, cx, 300.);
    // No in-window MouseUp is delivered. The next in-window move carries the
    // authoritative button state, exactly as after release outside the window.
    cx.simulate_mouse_move(point(start.x + px(130.), start.y), None, Default::default());
    page.read_with(cx, |page, cx| assert_eq!(page.sidebar.read(cx).width(), 300., "hover after lost release must not resize"));
    assert_width(&page, cx, 300.);
    page.read_with(cx, |page, _| assert!(page.sidebar_drag.is_none(), "lost release clears resize state"));
    cx.simulate_mouse_move(point(start.x + px(180.), start.y), None, Default::default());
    assert_width(&page, cx, 300.);
    // Cancellation must not disable a subsequent intentional resize.
    drag_to(&page, cx, 240.);
    assert_width(&page, cx, 240.);
    page.read_with(cx, |page, _| assert!(page.state.handles.borrow().is_empty()));
    let _ = std::fs::remove_file(path);
}

/// Clicking expand and grabbing the moving edge is reachable through real UI
/// events. A stale fold animation must not undo the newer explicit resize.
#[gpui_kit::gpui::test]
fn sidebar_resize_during_expand_keeps_the_new_width_after_animation(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture(cx);
    assert_width(&page, cx, 220.);
    let collapse = cx.debug_bounds("collapse-resource-sidebar").expect("visible collapse control");
    cx.simulate_click(collapse.center(), Default::default());
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(220));
    cx.executor().advance_clock(Duration::from_millis(32));
    assert_width(&page, cx, 36.);
    let expand = cx.debug_bounds("expand-resource-sidebar").expect("visible expand control");
    cx.simulate_click(expand.center(), Default::default());
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(50));
    cx.executor().advance_clock(Duration::from_millis(16));
    draw(cx);
    page.read_with(cx, |page, _| assert!(page.sidebar_width > 40. && page.sidebar_width < 220., "drag starts during the actual expansion"));
    drag_to(&page, cx, 320.);
    assert_width(&page, cx, 320.);
    // Let the previous animation finish. It must not restore its old target.
    std::thread::sleep(Duration::from_millis(220));
    cx.executor().advance_clock(Duration::from_millis(32));
    draw(cx);
    page.read_with(cx, |page, cx| assert_eq!(page.sidebar.read(cx).width(), 320., "stale expansion must not overwrite the newer drag target"));
    assert_width(&page, cx, 320.);
    page.read_with(cx, |page, _| assert_eq!(page.state.store.borrow().sidebar_width(), 320.));
    let _ = std::fs::remove_file(path);
}

/// Folding still shows a wide column for a few frames, but the user has
/// explicitly requested the strip: grabbing its moving edge must not cancel it.
#[gpui_kit::gpui::test]
fn sidebar_collapsing_edge_does_not_start_a_resize(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture(cx);
    assert_width(&page, cx, 220.);
    let collapse = cx.debug_bounds("collapse-resource-sidebar").unwrap();
    cx.simulate_click(collapse.center(), Default::default());
    draw(cx);
    let right = page.read_with(cx, |page, _| page.sidebar_edge.get().1);
    let edge = point(px(right - 1.), px(100.));
    cx.simulate_mouse_down(edge, MouseButton::Left, Default::default());
    page.read_with(cx, |page, _| assert!(page.sidebar_drag.is_none(), "collapsing sidebar cannot start a resize"));
    cx.simulate_mouse_move(point(edge.x + px(80.), edge.y), MouseButton::Left, Default::default());
    cx.simulate_mouse_up(edge, MouseButton::Left, Default::default());
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(220));
    cx.executor().advance_clock(Duration::from_millis(32));
    assert_width(&page, cx, 36.);
    page.read_with(cx, |page, _| assert_eq!(page.state.store.borrow().sidebar_width(), 220.));
    let _ = std::fs::remove_file(path);
}

#[gpui_kit::gpui::test]
fn sidebar_same_width_grab_during_expand_retargets_content_and_animation(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture(cx);
    assert_width(&page, cx, 220.);
    drag_to(&page, cx, 420.);
    assert_width(&page, cx, 420.);
    let collapse = cx.debug_bounds("collapse-resource-sidebar").unwrap();
    cx.simulate_click(collapse.center(), Default::default());
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(220));
    cx.executor().advance_clock(Duration::from_millis(32));
    assert_width(&page, cx, 36.);
    let expand = cx.debug_bounds("expand-resource-sidebar").unwrap();
    cx.simulate_click(expand.center(), Default::default());
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(60));
    cx.executor().advance_clock(Duration::from_millis(16));
    draw(cx);
    let (right, width) = page.read_with(cx, |page, _| (page.sidebar_edge.get().1, page.sidebar_width));
    assert!(width > 180. && width < 420., "same-width grab starts during expansion above the minimum");
    let edge = point(px(right - 1.), px(100.));
    cx.simulate_mouse_down(edge, MouseButton::Left, Default::default());
    cx.simulate_mouse_move(edge, MouseButton::Left, Default::default());
    cx.simulate_mouse_up(edge, MouseButton::Left, Default::default());
    page.read_with(cx, |page, cx| assert_eq!(page.sidebar.read(cx).content_width(), width, "same-width grab updates the old content target"));
    std::thread::sleep(Duration::from_millis(220));
    cx.executor().advance_clock(Duration::from_millis(32));
    assert_width(&page, cx, width);
    let _ = std::fs::remove_file(path);
}

/// The empty landing page draws the same resize affordance as an open session.
/// Exercise that production branch without inserting a tab or opening a transport.
#[gpui_kit::gpui::test]
fn empty_landing_sidebar_drag_updates_content_and_stops_after_release(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture_with_tab(cx, false);
    assert_width(&page, cx, 220.);
    page.read_with(cx, |page, _| assert!(page.tabs.is_empty()));
    let right = page.read_with(cx, |page, _| page.sidebar_edge.get().1);
    let start = point(px(right - 1.), px(500.));
    let end = point(start.x + px(80.), start.y);
    cx.simulate_mouse_move(start, None, Default::default());
    cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
    page.read_with(cx, |page, _| assert!(page.sidebar_drag.is_some(), "empty landing resize affordance must receive the actual pointer press"));
    cx.simulate_mouse_move(end, MouseButton::Left, Default::default());
    cx.simulate_mouse_up(end, MouseButton::Left, Default::default());
    assert_width(&page, cx, 300.);
    // A normal release must leave the pointer free to hover without resizing.
    cx.simulate_mouse_move(point(end.x + px(40.), end.y), None, Default::default());
    assert_width(&page, cx, 300.);
    page.read_with(cx, |page, _| {
        assert!(page.sidebar_drag.is_none());
        assert_eq!(page.state.store.borrow().sidebar_width(), 300.);
        assert!(page.tabs.is_empty());
        assert!(page.state.handles.borrow().is_empty(), "empty fixture never opens a transport");
    });
    drag_to(&page, cx, 220.);
    assert_width(&page, cx, 220.);
    let _ = std::fs::remove_file(path);
}
