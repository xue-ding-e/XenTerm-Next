//! Exercise the shell's real dialog wrapper and editor at the allowed minimum
//! window size, including resizing an already open dialog. All data is synthetic.
use super::*;
use gpui_kit::{
    gpui::{TestAppContext, VisualTestContext},
    test::TestWindowExt as _,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct Host {
    subscription: Option<Subscription>,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.subscription.is_none() {
            self.subscription = crate::ui::follow_root(window, cx);
        }
        div()
            .size_full()
            .children(Root::render_dialog_layer(window, cx))
    }
}

fn host(cx: &mut TestAppContext, height: f32) -> &mut VisualTestContext {
    cx.update(gpui_kit::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let host = cx.new(|_| Host { subscription: None });
        Root::new(host, window, cx)
    });
    cx.simulate_resize(size(px(1100.), px(height)));
    draw(cx);
    cx
}

fn store() -> Rc<RefCell<crate::config::ConfigStore>> {
    Rc::new(RefCell::new(crate::config::ConfigStore {
        path: std::env::temp_dir().join(format!("xenterm-overlay-{}.db", uuid::Uuid::new_v4())),
        backup_dir: None,
        cache: crate::config::ConfigFile::default(),
        key: [0; 32],
        keyring_enabled: false,
        saved_state: Mutex::new(crate::config::SavedState::default()).into(),
    }))
}

fn open_editor(cx: &mut VisualTestContext) -> (Entity<SessionEditor>, Rc<Cell<usize>>) {
    let closed = Rc::new(Cell::new(0));
    let editor = cx.update(|window, cx| {
        let editor = cx.new(|_| SessionEditor::new_session(store(), String::new()));
        let close_count = closed.clone();
        Shell::overlay_dialog_with_close(
            editor.clone(),
            "Fixture editor".into(),
            940.,
            560.,
            window,
            cx,
            move |_, _| close_count.set(close_count.get() + 1),
        );
        editor
    });
    settle(cx);
    (editor, closed)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
}

fn card(cx: &mut VisualTestContext) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update(|window, _| window.within("dialog").find(0usize).bounds())
}

fn settle(cx: &mut VisualTestContext) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut previous = None;
    let mut stable = 0;
    loop {
        draw(cx);
        let current = card(cx);
        if previous == Some(current) {
            stable += 1;
        } else {
            stable = 0;
        }
        if stable == 3 {
            break;
        }
        assert!(Instant::now() < deadline, "dialog geometry did not settle");
        previous = Some(current);
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn assert_inside(cx: &mut VisualTestContext, bounds: gpui_kit::Bounds<gpui_kit::Pixels>) {
    let viewport = cx.update(|window, _| window.viewport_size());
    assert!(
        bounds.left() >= px(0.)
            && bounds.top() >= px(0.)
            && bounds.right() <= viewport.width
            && bounds.bottom() <= viewport.height,
        "dialog control {bounds:?} is outside viewport {viewport:?}"
    );
}

fn assert_editor_visible(cx: &mut VisualTestContext) {
    let bounds = card(cx);
    assert_inside(cx, bounds);
    for selector in ["editor-cancel", "editor-save"] {
        let button = cx
            .debug_bounds(selector)
            .expect("editor footer is rendered");
        assert_inside(cx, button);
        assert!(button.size.height >= px(24.) && button.size.width >= px(24.));
    }
    let close = cx.update(|window, _| window.within("dialog").find("close").bounds());
    assert_inside(cx, close);
}

#[gpui_kit::gpui::test]
fn editor_opened_at_minimum_window_keeps_footer_reachable(cx: &mut TestAppContext) {
    let cx = host(cx, 520.);
    let (editor, _) = open_editor(cx);
    assert_editor_visible(cx);
    let cancel = cx.debug_bounds("editor-cancel").unwrap();
    cx.simulate_click(cancel.center(), gpui_kit::Modifiers::default());
    editor.update(cx, |editor, _| {
        assert_eq!(editor.take_outcome(), Some(EditorOutcome::Cancelled))
    });
}

#[gpui_kit::gpui::test]
fn open_editor_reflows_after_shrink_and_restore_without_losing_close(cx: &mut TestAppContext) {
    let cx = host(cx, 812.);
    let (_, closed) = open_editor(cx);
    assert_editor_visible(cx);
    let large = card(cx);
    cx.simulate_resize(size(px(1100.), px(520.)));
    settle(cx);
    assert_editor_visible(cx);
    let small = card(cx);
    assert!(small.size.height < large.size.height);
    cx.simulate_resize(size(px(1100.), px(812.)));
    settle(cx);
    assert_editor_visible(cx);
    assert_eq!(card(cx).size, large.size);
    let close = cx.update(|window, _| window.within("dialog").find("close").bounds());
    cx.simulate_click(close.center(), gpui_kit::Modifiers::default());
    draw(cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(closed.get(), 1);
}

#[gpui_kit::gpui::test]
fn other_overlay_views_stay_inside_minimum_window_and_escape_closes(cx: &mut TestAppContext) {
    let cx = host(cx, 520.);
    let store = store();
    for kind in 0..5 {
        cx.update(|window, cx| match kind {
            0 => {
                let view = cx.new(|cx| GroupManagerView::new(store.clone(), window, cx));
                Shell::overlay_dialog_with_close(
                    view,
                    "Groups".into(),
                    620.,
                    520.,
                    window,
                    cx,
                    |_, _| {},
                );
            }
            1 => {
                let view = cx.new(|cx| QuickManagerView::new(store.clone(), window, cx));
                Shell::overlay_dialog_with_close(
                    view,
                    "Quick commands".into(),
                    760.,
                    520.,
                    window,
                    cx,
                    |_, _| {},
                );
            }
            2 => {
                let view = cx.new(|cx| RuleEditorView::new(store.clone(), window, cx));
                Shell::overlay_dialog_with_close(
                    view,
                    "Rule".into(),
                    620.,
                    480.,
                    window,
                    cx,
                    |_, _| {},
                );
            }
            3 => {
                let view = cx.new(|cx| {
                    TunnelsView::new(
                        store.clone(),
                        None,
                        None,
                        Arc::new(Mutex::new(Default::default())),
                        window,
                        cx,
                    )
                });
                Shell::overlay_dialog_with_close(
                    view,
                    "Tunnels".into(),
                    720.,
                    480.,
                    window,
                    cx,
                    |_, _| {},
                );
            }
            _ => {
                let view = cx.new(|cx| {
                    crate::ui::file_viewer::FileViewerView::new(
                        "fixture.txt".into(),
                        "fixture.txt".into(),
                        "synthetic line\n".repeat(100),
                        false,
                        String::new(),
                        window,
                        cx,
                    )
                });
                Shell::overlay_dialog_with_close(
                    view,
                    "File".into(),
                    760.,
                    560.,
                    window,
                    cx,
                    |_, _| {},
                );
            }
        });
        settle(cx);
        let bounds = card(cx);
        assert_inside(cx, bounds);
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }
}
