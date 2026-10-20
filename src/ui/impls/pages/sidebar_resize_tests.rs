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
    draw(cx);
    page.read_with(cx, |page, cx| {
        let sidebar = page.sidebar.read(cx);
        assert_eq!(sidebar.width(), expected, "outer sidebar width");
        assert_eq!(sidebar.content_width(), expected, "sidebar content must follow resized column");
        assert_eq!(page.sidebar_width, expected, "workspace observer width");
        let (left, right) = page.sidebar_edge.get();
        assert!((right - left - expected).abs() < 1.1, "rendered column width");
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

#[gpui_kit::gpui::test]
fn sidebar_drag_keeps_content_aligned_and_restores_resized_width(cx: &mut TestAppContext) {
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
    assert_width(&page, cx, 220.);
    for (requested, expected) in [(420., 420.), (180., 180.), (500., 420.), (100., 180.), (320., 320.)] {
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
