//! Real settings events: copying guidance must never enable access or edit it.

use super::*;
use gpui_kit::component::Root;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use gpui_kit::Focusable as _;
use std::cell::RefCell;

struct Fixture {
    directory: tempfile::TempDir,
    store: Rc<RefCell<ConfigStore>>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let cache = crate::config::ConfigFile::default();
        let store = Rc::new(RefCell::new(ConfigStore {
            path: directory.path().join("sessions.db"),
            backup_dir: None,
            saved_state: std::sync::Mutex::new(crate::config::SavedState::of_cache(&cache)).into(),
            cache,
            key: [7; 32],
            keyring_enabled: false,
        }));
        Self { directory, store }
    }

    fn config(&self) -> String {
        client_config(
            &self.directory.path().join("XenTerm tools").join("xenterm"),
            Some(self.directory.path()),
        )
        .unwrap()
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn open<'a>(
    cx: &'a mut TestAppContext,
    fixture: &Fixture,
    json: Result<String, ConfigError>,
) -> (Entity<SettingsView>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let mut handle = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let preview = json.as_ref().ok().map(|json| {
                cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .rows(12)
                        .default_value(json.clone())
                })
            });
            let mut view = SettingsView::new(fixture.store.clone());
            view.selected = SettingsPageId::McpPermissions;
            view.mcp_setup = Some(McpSetup {
                json,
                preview,
                data_dir: fixture.directory.path().to_path_buf(),
                explicit_profile: true,
                copied: false,
            });
            view
        });
        handle = Some(view.clone());
        Root::new(view, window, cx)
    });
    cx.update(|window, _| {
        window.resize(gpui_kit::size(px(1100.), px(1800.)));
        window.activate_window();
    });
    draw(cx);
    (handle.unwrap(), cx)
}

fn click_copy(cx: &mut VisualTestContext) {
    let bounds = cx
        .debug_bounds("settings-copy-mcp-config")
        .expect("copy control is visible");
    cx.simulate_click(bounds.center(), gpui_kit::Modifiers::default());
    draw(cx);
}

fn shortcut(key: &str) -> String {
    format!(
        "{}-{key}",
        if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        }
    )
}

#[gpui_kit::gpui::test]
fn repeated_copy_writes_exact_json_without_enabling_mcp_or_saving(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let expected = fixture.config();
    let before = serde_json::to_value(&fixture.store.borrow().cache).unwrap();
    let (view, cx) = open(cx, &fixture, Ok(expected.clone()));
    for _ in 0..2 {
        cx.update(|_, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("old clipboard".into()))
        });
        click_copy(cx);
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some(expected.clone())
        );
        assert!(view.read_with(cx, |view, _| view.mcp_setup.as_ref().unwrap().copied));
    }
    assert_eq!(
        serde_json::to_value(&fixture.store.borrow().cache).unwrap(),
        before
    );
    assert!(!fixture.directory.path().join("sessions.db").exists());
}

#[gpui_kit::gpui::test]
fn json_preview_allows_selection_but_rejects_typing_cut_and_paste(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let expected = fixture.config();
    let (view, cx) = open(cx, &fixture, Ok(expected.clone()));
    let preview = view.read_with(cx, |view, _| {
        view.mcp_setup.as_ref().unwrap().preview.clone().unwrap()
    });
    cx.update(|window, cx| {
        let focus = preview.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
    draw(cx);
    cx.simulate_keystrokes(&shortcut("a"));
    assert_eq!(
        preview.read_with(cx, |preview, _| preview.selected_value().to_string()),
        expected
    );
    cx.simulate_keystrokes(&shortcut("c"));
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some(expected.clone())
    );
    cx.simulate_input("attempted replacement");
    cx.simulate_keystrokes("backspace delete");
    cx.simulate_keystrokes(&shortcut("x"));
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("paste attempt".into()))
    });
    cx.simulate_keystrokes(&shortcut("v"));
    draw(cx);
    assert_eq!(
        preview.read_with(cx, |preview, _| preview.value().to_string()),
        expected
    );
    assert!(!fixture.store.borrow().mcp_enabled());
    assert!(!fixture.directory.path().join("sessions.db").exists());
}

#[gpui_kit::gpui::test]
fn unavailable_executable_disables_copy_and_preserves_clipboard(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (view, cx) = open(cx, &fixture, Err(ConfigError::ExecutableUnavailable));
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("keep clipboard".into()))
    });
    click_copy(cx);
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("keep clipboard".into())
    );
    assert!(view.read_with(cx, |view, _| view
        .mcp_setup
        .as_ref()
        .unwrap()
        .preview
        .is_none()));
    assert!(!view.read_with(cx, |view, _| view.mcp_setup.as_ref().unwrap().copied));
    assert!(!fixture.store.borrow().mcp_enabled());
}
