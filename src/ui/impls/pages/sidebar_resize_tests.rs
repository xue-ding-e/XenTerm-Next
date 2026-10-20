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
        assert_eq!(sidebar.width(), expected, "outer sidebar width");
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
    cx.update(gpui_kit::init);
    cx.update(crate::ui::actions::init);
    let path = std::env::temp_dir().join(format!("xenterm-sidebar-resize-{}.db", uuid::Uuid::new_v4()));
    let mut store = crate::config::ConfigStore {
        path: path.clone(), backup_dir: None,
        cache: crate::config::ConfigFile::default(), key: [7; 32], keyring_enabled: false,
        saved_state: Mutex::new(crate::config::SavedState::default()).into(),
    };
    store.set_sidebar_width(220.);
    store.set_sidebar_collapsed(false);
    let state = crate::ui::SessionState::new(
        Arc::new(tokio::runtime::Runtime::new().unwrap()),
        Arc::new(Mutex::new(std::collections::HashMap::new())), Rc::new(RefCell::new(store)),
    );
    let (page, cx) = cx.add_window_view(move |window, cx| {
        let mut page = TerminalPage::new(state, Rc::new(Cell::new(1280.)), window, cx);
        let mut tab = TerminalPage::open_tab(&page.state, "resize-fixture", "synthetic-profile", "Resize fixture", page.appearance.clone(), cx);
        PENDING_SINKS.with(|sinks| sinks.borrow_mut().remove("resize-fixture"));
        tab.sink = Arc::new(TestSink);
        page.tabs.push(tab);
        page.active_tab = Some("resize-fixture".into());
        page.panes = crate::layout::Layout::new(vec!["resize-fixture".into()], "resize-fixture".into());
        page
    });
    (page, cx, path)
}

#[gpui_kit::gpui::test]
fn sidebar_drag_keeps_content_aligned_and_restores_resized_width(cx: &mut TestAppContext) {
    let (page, cx, path) = fixture(cx);
    assert_width(&page, cx, 220.);
    for (requested, expected) in [(420., 420.), (180., 180.), (500., 420.), (100., 180.), (310.25, 310.25), (310.75, 310.75), (320., 320.)] {
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

