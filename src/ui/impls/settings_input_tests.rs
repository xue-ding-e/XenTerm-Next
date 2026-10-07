//! Real input events with an isolated store; no WebDAV request or keyring access.

use super::*;
use gpui_kit::component::{Root, WindowExt as _};
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use std::{cell::RefCell, path::PathBuf};

struct Fixture {
    directory: PathBuf,
    store: Rc<RefCell<ConfigStore>>,
}

impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("xenterm-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let cache = crate::config::ConfigFile::default();
        Self {
            store: Rc::new(RefCell::new(ConfigStore {
                path: directory.join("sessions.db"),
                backup_dir: None,
                saved_state: std::sync::Mutex::new(crate::config::SavedState::of_cache(&cache))
                    .into(),
                cache,
                key: [7; 32],
                keyring_enabled: false,
            })),
            directory,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
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
    page: SettingsPageId,
) -> (Entity<SettingsView>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let mut handle = None;
    let store = fixture.store.clone();
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| {
            let mut view = SettingsView::new(store);
            view.selected = page;
            view
        });
        handle = Some(view.clone());
        Root::new(view, window, cx)
    });
    cx.update(|window, _cx| window.resize(gpui_kit::size(px(1100.), px(1800.))));
    draw(cx);
    (handle.unwrap(), cx)
}

fn page(view: &Entity<SettingsView>, page: SettingsPageId, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.blur(cx);
        view.update(cx, |view, cx| {
            view.selected = page;
            cx.notify();
        });
    });
    draw(cx);
}

fn focus_value(value: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.blur(cx));
    for _ in 0..60 {
        let found = cx.update(|window, cx| {
            window.focus_next(cx);
            window.draw(cx).clear(cx);
            window
                .focused_input(cx)
                .is_some_and(|input| input.value(cx).as_ref() == value)
        });
        if found {
            draw(cx);
            return;
        }
    }
    panic!("no focusable fixture input with value {value:?}");
}

fn value(cx: &mut VisualTestContext) -> String {
    cx.update(|window, cx| {
        window
            .focused_input(cx)
            .expect("focused input")
            .value(cx)
            .to_string()
    })
}

fn replace(text: &str, cx: &mut VisualTestContext) {
    number_change(text, cx);
}

fn number_change(text: &str, cx: &mut VisualTestContext) {
    // Exercise the real field subscription, independently of selection and
    // number formatting. The character-by-character tests use native input.
    cx.update(|window, cx| {
        let input = window
            .focused_input(cx)
            .unwrap()
            .as_input()
            .unwrap()
            .clone();
        input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            cx.emit(InputEvent::Change);
        });
    });
    draw(cx);
}

#[gpui_kit::gpui::test]
fn cursor_keeps_partial_text_without_persisting_it(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture
        .store
        .borrow_mut()
        .set_terminal_cursor_color("#ABCDEF");
    let (_, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    focus_value("#ABCDEF", cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    draw(cx);
    let mut typed = String::new();
    for ch in "#123456".chars() {
        typed.push(ch);
        cx.simulate_input(&ch.to_string());
        draw(cx);
        assert_eq!(
            value(cx),
            typed,
            "a repaint must preserve partial colour text"
        );
        assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#ABCDEF");
        assert!(
            !fixture.store.borrow().path.exists(),
            "typing alone must not save"
        );
    }
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#123456");
    assert!(fixture.store.borrow().path.exists());
}

#[gpui_kit::gpui::test]
fn number_fields_keep_their_setter_after_page_switch(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_quick_panel_height(280.);
    fixture.store.borrow_mut().set_font_size(14);
    let (view, cx) = open(cx, &fixture, SettingsPageId::Files);
    focus_value("280", cx);
    number_change("300", cx);
    assert_eq!(fixture.store.borrow().quick_panel_height(), 300.);
    page(&view, SettingsPageId::TermFont, cx);
    focus_value("14", cx);
    number_change("18", cx);
    assert_eq!(fixture.store.borrow().font_size(), 18);
    assert_eq!(
        fixture.store.borrow().quick_panel_height(),
        300.,
        "font edit must not write the old page's field"
    );
}

#[gpui_kit::gpui::test]
fn sync_address_keeps_url_separators_and_does_not_edit_cursor(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture
        .store
        .borrow_mut()
        .set_terminal_cursor_color("#123456");
    fixture.store.borrow_mut().set_webdav_settings(
        false,
        "https://before.invalid".into(),
        String::new(),
        String::new(),
        "fixture.json".into(),
        false,
    );
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    focus_value("#123456", cx);
    page(&view, SettingsPageId::Sync, cx);
    focus_value("https://before.invalid", cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    draw(cx);
    let mut typed = String::new();
    for ch in "https://ui-qa.invalid/webdav".chars() {
        typed.push(ch);
        cx.simulate_input(&ch.to_string());
        draw(cx);
        assert_eq!(
            value(cx),
            typed,
            "URL typing must keep :// and intermediate text"
        );
        assert_eq!(
            fixture.store.borrow().webdav_url(),
            "https://before.invalid"
        );
        assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#123456");
    }
    cx.simulate_keystrokes("enter");
    draw(cx);
    let store = fixture.store.borrow();
    assert_eq!(store.webdav_url(), typed);
    assert_eq!(store.terminal_cursor_color(), "#123456");
    assert!(!store.webdav_enabled());
    assert!(!store.webdav_accept_invalid_certs());
    assert!(store.webdav_password().is_empty());
    assert!(view.update(cx, |view, _| view.take_action()).is_none());
}

#[gpui_kit::gpui::test]
fn invalid_colour_and_failed_save_keep_the_draft_for_retry(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture
        .store
        .borrow_mut()
        .set_terminal_cursor_color("#112233");
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    focus_value("#112233", cx);
    replace("#12", cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(value(cx), "#12");
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#112233");
    assert!(!fixture.store.borrow().path.exists());
    assert!(
        view.read_with(cx, |view, _| view.text_drafts[&TextSetting::CursorColor]
            .error
            .is_some())
    );
    page(&view, SettingsPageId::Sync, cx);
    page(&view, SettingsPageId::TermCursor, cx);
    focus_value("#12", cx);
    replace("#a1b2c3", cx);
    // A directory at the database path deterministically fails, even as root.
    let path = fixture.store.borrow().path.clone();
    std::fs::create_dir(&path).unwrap();
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(value(cx), "#a1b2c3");
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#112233");
    assert!(
        view.read_with(cx, |view, _| view.text_drafts[&TextSetting::CursorColor]
            .error
            .is_some())
    );
    std::fs::remove_dir(&path).unwrap();
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(value(cx), "#A1B2C3");
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#A1B2C3");
    let connection = rusqlite::Connection::open(path).unwrap();
    let raw: String = connection
        .query_row("SELECT value FROM meta WHERE key = 'settings'", [], |row| {
            row.get(0)
        })
        .unwrap();
    let saved: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(saved["terminal_cursor_color"], "#A1B2C3");
}

#[gpui_kit::gpui::test]
fn empty_colour_commits_the_advertised_default_on_blur_event(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture
        .store
        .borrow_mut()
        .set_terminal_cursor_color("#112233");
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    focus_value("#112233", cx);
    replace("", cx);
    assert_eq!(value(cx), "");
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#112233");
    assert!(!fixture.store.borrow().path.exists());
    // Dispatch the toolkit event to the registered input. Actual focus-away
    // behaviour must also be checked by the separate real-window QA run.
    view.update(cx, |view, cx| {
        let input = view.text_drafts[&TextSetting::CursorColor].input.clone();
        input.update(cx, |_, cx| cx.emit(InputEvent::Blur));
    });
    draw(cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "");
    assert!(fixture.store.borrow().path.exists());
}

#[gpui_kit::gpui::test]
fn url_validation_waits_for_commit_and_never_changes_sync_permissions(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_webdav_settings(
        false,
        "https://before.invalid".into(),
        "fixture-user".into(),
        String::new(),
        "fixture.json".into(),
        false,
    );
    let (view, cx) = open(cx, &fixture, SettingsPageId::Sync);
    focus_value("https://before.invalid", cx);
    replace("https://", cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(value(cx), "https://");
    assert_eq!(
        fixture.store.borrow().webdav_url(),
        "https://before.invalid"
    );
    assert!(!fixture.store.borrow().path.exists());
    assert!(
        view.read_with(cx, |view, _| view.text_drafts[&TextSetting::WebdavUrl]
            .error
            .is_some())
    );
    replace("https://ui-qa.invalid/webdav/", cx);
    assert_eq!(value(cx), "https://ui-qa.invalid/webdav/");
    cx.simulate_keystrokes("enter");
    draw(cx);
    let store = fixture.store.borrow();
    assert_eq!(store.webdav_url(), "https://ui-qa.invalid/webdav");
    assert_eq!(store.webdav_username(), "fixture-user");
    assert_eq!(store.webdav_remote_path(), "fixture.json");
    assert!(!store.webdav_enabled());
    assert!(!store.webdav_accept_invalid_certs());
    assert!(store.webdav_password().is_empty());
    assert!(view.update(cx, |view, _| view.take_action()).is_none());
}
