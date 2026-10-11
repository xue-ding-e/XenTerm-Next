//! Real input events with an isolated store; no WebDAV request or keyring access.

use super::*;
use gpui_kit::Focusable as _;
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
    cx.update(|window, _cx| {
        window.resize(gpui_kit::size(px(1100.), px(1800.)));
        // The test platform starts inactive. Real focus/blur callbacks are
        // dispatched only for active windows, just as in the native app.
        window.activate_window();
    });
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
    assert_eq!(fixture.store.borrow().quick_panel_height(), 280.);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(fixture.store.borrow().quick_panel_height(), 300.);
    page(&view, SettingsPageId::TermFont, cx);
    focus_value("14", cx);
    number_change("18", cx);
    assert_eq!(fixture.store.borrow().font_size(), 14);
    cx.simulate_keystrokes("enter");
    draw(cx);
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
fn empty_colour_waits_for_explicit_apply_after_blur(cx: &mut TestAppContext) {
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
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#112233");
    assert!(!fixture.store.borrow().path.exists(), "leaving the colour draft must not defeat Cancel");
    click_colour("cursor-color-apply", cx);
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

fn check_numeric_typing(
    cx: &mut TestAppContext,
    selected: SettingsPageId,
    initial: &str,
    typed: &str,
    read: fn(&ConfigStore) -> f64,
) {
    let fixture = Fixture::new();
    let before = read(&fixture.store.borrow());
    let (_, cx) = open(cx, &fixture, selected);
    focus_value(initial, cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" });
    draw(cx);
    let mut expected = String::new();
    for ch in typed.chars() {
        expected.push(ch);
        cx.simulate_input(&ch.to_string());
        draw(cx);
        assert_eq!(value(cx), expected, "render must retain the whole raw numeric draft");
        assert_eq!(read(&fixture.store.borrow()), before, "typing must not persist/clamp a partial number");
        assert!(!fixture.store.borrow().path.exists(), "typing alone must not save");
    }
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!((read(&fixture.store.borrow()) - typed.parse::<f64>().unwrap()).abs() < 0.000001);
    assert_eq!(value(cx), typed, "committed display must not widen f32 representation noise");
    assert!(fixture.store.borrow().path.exists());
}

#[gpui_kit::gpui::test]
fn numeric_panel_font_keeps_three_digit_draft(cx: &mut TestAppContext) {
    check_numeric_typing(cx, SettingsPageId::Interface, "100", "111", |s| f64::from(s.panel_font()));
}

#[gpui_kit::gpui::test]
fn numeric_font_size_keeps_two_digit_draft(cx: &mut TestAppContext) {
    check_numeric_typing(cx, SettingsPageId::TermFont, "13", "17", |s| f64::from(s.font_size()));
}

#[gpui_kit::gpui::test]
fn numeric_line_spacing_keeps_decimal_draft(cx: &mut TestAppContext) {
    check_numeric_typing(cx, SettingsPageId::TermFont, "1", "0.9", |s| f64::from(s.terminal_line_spacing()));
}

#[gpui_kit::gpui::test]
fn numeric_panel_height_keeps_three_digit_draft(cx: &mut TestAppContext) {
    let initial = Fixture::new().store.borrow().quick_panel_height().to_string();
    check_numeric_typing(cx, SettingsPageId::Files, &initial, "237", |s| f64::from(s.quick_panel_height()));
}

fn type_numeric(text: &str, cx: &mut VisualTestContext) {
    cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-a backspace" } else { "ctrl-a backspace" });
    if !text.is_empty() { cx.simulate_input(text); }
    draw(cx);
    assert_eq!(value(cx), text);
}

#[gpui_kit::gpui::test]
fn numeric_invalid_and_failed_save_preserve_the_raw_draft(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (view, cx) = open(cx, &fixture, SettingsPageId::Interface);
    let key = TextSetting::Number(NumberSetting::PanelFont);
    focus_value("100", cx);
    for invalid in ["", "-", "abc", "NaN", "inf", "100.5"] {
        type_numeric(invalid, cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(value(cx), invalid);
        assert_eq!(fixture.store.borrow().panel_font(), 100);
        assert!(!fixture.store.borrow().path.exists());
        assert!(view.read_with(cx, |view, _| view.text_drafts[&key].error.is_some()));
    }
    page(&view, SettingsPageId::Files, cx);
    page(&view, SettingsPageId::Interface, cx);
    focus_value("100.5", cx);
    assert!(view.read_with(cx, |view, _| view.text_drafts[&key].error.is_some()));
    type_numeric("111", cx);
    let path = fixture.store.borrow().path.clone();
    std::fs::create_dir(&path).unwrap();
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(value(cx), "111");
    assert_eq!(fixture.store.borrow().panel_font(), 100);
    assert!(view.read_with(cx, |view, _| view.text_drafts[&key].error.is_some()));
    std::fs::remove_dir(&path).unwrap();
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(fixture.store.borrow().panel_font(), 111);
    let connection = rusqlite::Connection::open(path).unwrap();
    let raw: String = connection.query_row("SELECT value FROM meta WHERE key='settings'", [], |row| row.get(0)).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(saved["panel_font"], 111);
    assert!(!fixture.store.borrow().mcp_enabled());
    assert!(!fixture.store.borrow().webdav_enabled());
    assert!(!fixture.store.borrow().webdav_accept_invalid_certs());
}

#[gpui_kit::gpui::test]
fn numeric_spacing_buttons_step_by_tenths_and_clamp(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_terminal_line_spacing(0.8);
    let (_, cx) = open(cx, &fixture, SettingsPageId::TermFont);
    let bounds = cx.debug_bounds("settings-number-LineSpacing").unwrap();
    assert!(bounds.size.height > px(20.));
    let increment = gpui_kit::point(bounds.right() - px(12.), bounds.center().y);
    let decrement = gpui_kit::point(bounds.left() + px(12.), bounds.center().y);
    cx.simulate_click(increment, gpui_kit::Modifiers::default());
    draw(cx);
    assert!((fixture.store.borrow().terminal_line_spacing() - 0.9).abs() < 0.000001);
    assert_eq!(value(cx), "0.9");
    for _ in 0..10 { cx.simulate_click(increment, gpui_kit::Modifiers::default()); draw(cx); }
    assert_eq!(fixture.store.borrow().terminal_line_spacing(), 1.5);
    assert_eq!(value(cx), "1.5");
    cx.simulate_click(decrement, gpui_kit::Modifiers::default());
    draw(cx);
    assert!((fixture.store.borrow().terminal_line_spacing() - 1.4).abs() < 0.000001);
    assert_eq!(value(cx), "1.4");
    type_numeric("0.7", cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(fixture.store.borrow().terminal_line_spacing(), 0.8);
    assert_eq!(value(cx), "0.8");
}

#[gpui_kit::gpui::test]
fn numeric_blur_commits_whole_values_without_rebinding_other_pages(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (view, cx) = open(cx, &fixture, SettingsPageId::Interface);
    focus_value("100", cx);
    type_numeric("79", cx);
    let nav = cx.debug_bounds("settings-nav-TermFont").unwrap();
    cx.simulate_click(nav.center(), gpui_kit::Modifiers::default());
    draw(cx);
    assert_eq!(view.read_with(cx, |view, _| view.selected), SettingsPageId::TermFont);
    assert_eq!(fixture.store.borrow().panel_font(), 80);
    focus_value("13", cx);
    type_numeric("33", cx);
    let nav = cx.debug_bounds("settings-nav-Files").unwrap();
    cx.simulate_click(nav.center(), gpui_kit::Modifiers::default());
    draw(cx);
    assert_eq!(fixture.store.borrow().font_size(), 32);
    let initial = fixture.store.borrow().quick_panel_height().to_string();
    focus_value(&initial, cx);
    type_numeric("601.5", cx);
    let nav = cx.debug_bounds("settings-nav-Interface").unwrap();
    cx.simulate_click(nav.center(), gpui_kit::Modifiers::default());
    draw(cx);
    assert_eq!(fixture.store.borrow().quick_panel_height(), 600.);
    focus_value("80", cx);
    assert_eq!(fixture.store.borrow().font_size(), 32);
    assert_eq!(fixture.store.borrow().terminal_line_spacing(), 1.);
    assert!(!fixture.store.borrow().mcp_enabled());
}


fn click_colour(selector: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("missing {selector}"));
    cx.simulate_click(bounds.center(), gpui_kit::Modifiers::default());
    draw(cx);
}

#[gpui_kit::gpui::test]
fn colour_rgba_typing_and_page_switch_retain_draft_until_apply(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_terminal_cursor_color("#112233");
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    focus_value("#112233", cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-a backspace" } else { "ctrl-a backspace" });
    draw(cx);
    let mut expected = String::new();
    for ch in "rgba(18, 52, 86, 0.5)".chars() {
        expected.push(ch);
        cx.simulate_input(&ch.to_string());
        draw(cx);
        assert_eq!(value(cx), expected);
        assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#112233");
        assert!(!fixture.store.borrow().path.exists());
    }
    page(&view, SettingsPageId::TermFont, cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#112233");
    focus_value(&expected, cx);
    click_colour("cursor-color-apply", cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#12345680");
    let appearance = crate::ui::view::TerminalSettings::from_store(&fixture.store.borrow());
    assert!((appearance.cursor_color.unwrap().a - 128. / 255.).abs() < 0.000001);
    assert!(fixture.store.borrow().path.exists());
}

#[gpui_kit::gpui::test]
fn colour_picker_open_preview_selection_and_cancel_do_not_persist(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_terminal_cursor_color("#112233");
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    click_colour("cursor-color-picker", cx);
    let picker = view.read_with(cx, |view, _| view.color_editor.as_ref().unwrap().picker.clone());
    assert!(picker.read_with(cx, |picker, _| picker.is_open()), "real picker trigger opens the toolkit popup");
    cx.update(|window, cx| picker.update(cx, |picker, cx| picker.preview_color(gpui_kit::hsla(0., 1., 0.5, 1.), window, cx)));
    draw(cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#112233");
    // Use the existing component's selection event (the same event its swatches emit).
    cx.update(|window, cx| picker.update(cx, |picker, cx| picker.select_color(gpui_kit::hsla(0., 1., 0.5, 0.5), window, cx)));
    draw(cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#112233");
    assert!(!fixture.store.borrow().path.exists());
    assert_eq!(view.read_with(cx, |view, cx| view.text_drafts[&TextSetting::CursorColor].input.read(cx).value().to_string()), "#FF000080");
    click_colour("cursor-color-cancel", cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#112233");
    assert!(!fixture.store.borrow().path.exists());
    assert_eq!(view.read_with(cx, |view, cx| view.text_drafts[&TextSetting::CursorColor].input.read(cx).value().to_string()), "#112233");
    assert!(!picker.read_with(cx, |picker, _| picker.is_open()));
}

#[gpui_kit::gpui::test]
fn colour_picker_alpha_event_apply_and_default_cancel_are_controlled(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_terminal_cursor_color("#123456");
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    let picker = view.read_with(cx, |view, _| view.color_editor.as_ref().unwrap().picker.clone());
    let alpha = picker.read_with(cx, |picker, _| picker.sliders().alpha().clone());
    cx.update(|window, cx| alpha.update(cx, |slider, cx| {
        slider.set_value(0.5, window, cx);
        cx.emit(gpui_kit::component::slider::SliderEvent::Change(0.5.into()));
    }));
    draw(cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#123456");
    assert!(!fixture.store.borrow().path.exists());
    click_colour("cursor-color-apply", cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#12345680");
    click_colour("cursor-color-default", cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#12345680");
    click_colour("cursor-color-cancel", cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#12345680");
    click_colour("cursor-color-default", cx);
    click_colour("cursor-color-apply", cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "");
}

#[gpui_kit::gpui::test]
fn colour_formats_roundtrip_draft_and_reject_invalid_conversion(cx: &mut TestAppContext) {
    use crate::config::color::{ColorFormat, ColorValue};
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_terminal_cursor_color("#12345680");
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    for format in ColorFormat::ALL {
        cx.update(|window, cx| view.update(cx, |view, cx| view.change_cursor_format(format, window, cx)));
        draw(cx);
        let raw = view.read_with(cx, |view, cx| view.text_drafts[&TextSetting::CursorColor].input.read(cx).value().to_string());
        assert_eq!(ColorValue::parse(&raw).unwrap().canonical_hex(), "#12345680");
        assert!(!fixture.store.borrow().path.exists(), "format-only conversion does not write");
    }
    cx.update(|window, cx| view.update(cx, |view, cx| view.change_cursor_format(ColorFormat::Hex, window, cx)));
    draw(cx);
    focus_value("#12345680", cx);
    replace("rgba(", cx);
    cx.update(|window, cx| view.update(cx, |view, cx| view.change_cursor_format(ColorFormat::Cmyk, window, cx)));
    draw(cx);
    assert_eq!(value(cx), "rgba(");
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#12345680");
    assert!(view.read_with(cx, |view, _| view.text_drafts[&TextSetting::CursorColor].error.is_some()));
    assert_eq!(view.read_with(cx, |view, _| view.color_editor.as_ref().unwrap().format), ColorFormat::Hex);
    click_colour("cursor-color-cancel", cx);
    assert!(view.read_with(cx, |view, _| view.text_drafts[&TextSetting::CursorColor].error.is_none()));
}


#[gpui_kit::gpui::test]
fn colour_popup_unchanged_hex_enter_preserves_every_channel(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_terminal_cursor_color("#12345680");
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    click_colour("cursor-color-picker", cx);
    let picker = view.read_with(cx, |view, _| view.color_editor.as_ref().unwrap().picker.clone());
    assert!(picker.read_with(cx, |picker, _| picker.is_open()));
    let hex = picker.read_with(cx, |picker, _| picker.hex_input().clone());
    assert_eq!(hex.read_with(cx, |input, _| input.value().to_string()), "#12345680");
    cx.update(|window, cx| hex.update(cx, |input, cx| {
        input.focus_handle(cx).focus(window, cx);
    }));
    draw(cx);
    // The toolkit's internal HEX Enter selects a draft. Applying the setting
    // remains the outer Apply button, so popup cancellation never writes.
    assert!(cx.update(|window, cx| hex.read(cx).focus_handle(cx).is_focused(window)));
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#12345680");
    assert!(!fixture.store.borrow().path.exists());
    let raw = view.read_with(cx, |view, cx| view.text_drafts[&TextSetting::CursorColor].input.read(cx).value().to_string());
    assert_eq!(raw, "#12345680");
    click_colour("cursor-color-apply", cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#12345680");
}


#[gpui_kit::gpui::test]
fn colour_picker_retains_hue_through_achromatic_slider_endpoints(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_terminal_cursor_color("#808080");
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    let picker = view.read_with(cx, |view, _| view.color_editor.as_ref().unwrap().picker.clone());
    let sliders = picker.read_with(cx, |picker, _| picker.sliders().clone());
    for (slider, value) in [(sliders.hue(), 0.5), (sliders.saturation(), 1.), (sliders.lightness(), 0.), (sliders.lightness(), 0.5)] {
        cx.update(|window, cx| slider.update(cx, |slider, cx| {
            slider.set_value(value, window, cx);
            cx.emit(gpui_kit::component::slider::SliderEvent::Change(value.into()));
        }));
        draw(cx);
        assert!((sliders.hue().read_with(cx, |slider, _| slider.value().start()) - 0.5).abs() < 0.000001);
        assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#808080");
        assert!(!fixture.store.borrow().path.exists());
    }
    let raw = view.read_with(cx, |view, cx| view.text_drafts[&TextSetting::CursorColor].input.read(cx).value().to_string());
    assert_eq!(raw, "#00FFFF");
    click_colour("cursor-color-apply", cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#00FFFF");
}

#[gpui_kit::gpui::test]
fn colour_oversized_padded_input_keeps_error_and_never_saves(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_terminal_cursor_color("#112233");
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    focus_value("#112233", cx);
    let padded = format!("{}#123456", " ".repeat(256));
    replace(&padded, cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(value(cx), padded);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#112233");
    assert!(!fixture.store.borrow().path.exists());
    assert!(view.read_with(cx, |view, _| view.text_drafts[&TextSetting::CursorColor].dirty));
    assert!(view.read_with(cx, |view, _| view.text_drafts[&TextSetting::CursorColor].error.is_some()));
}


#[gpui_kit::gpui::test]
fn colour_default_picker_starts_with_opaque_resolved_default(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    let picker = view.read_with(cx, |view, _| view.color_editor.as_ref().unwrap().picker.clone());
    let sliders = picker.read_with(cx, |picker, _| picker.sliders().clone());
    assert_eq!(picker.read_with(cx, |picker, _| picker.value().unwrap().a), 1.);
    assert_eq!(sliders.alpha().read_with(cx, |slider, _| slider.value().start()), 1.);
    assert_eq!(sliders.lightness().read_with(cx, |slider, _| slider.value().start()), 1.);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "");
    for (slider, amount) in [(sliders.hue(), 0.5), (sliders.saturation(), 1.), (sliders.lightness(), 0.5)] {
        cx.update(|window, cx| slider.update(cx, |slider, cx| {
            slider.set_value(amount, window, cx);
            cx.emit(gpui_kit::component::slider::SliderEvent::Change(amount.into()));
        }));
        draw(cx);
    }
    let raw = view.read_with(cx, |view, cx| view.text_drafts[&TextSetting::CursorColor].input.read(cx).value().to_string());
    assert_eq!(raw, "#00FFFF");
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "");
    assert!(!fixture.store.borrow().path.exists());
    click_colour("cursor-color-cancel", cx);
    assert_eq!(sliders.alpha().read_with(cx, |slider, _| slider.value().start()), 1.);
    assert_eq!(sliders.lightness().read_with(cx, |slider, _| slider.value().start()), 1.);
    assert_eq!(view.read_with(cx, |view, cx| view.text_drafts[&TextSetting::CursorColor].input.read(cx).value().to_string()), "");
}


fn open_continuous_colour(view: &Entity<SettingsView>, cx: &mut VisualTestContext) -> Entity<super::super::color_area::HsvAreaState> {
    click_colour("cursor-color-picker", cx);
    assert!(view.read_with(cx, |view, cx| view.color_editor.as_ref().unwrap().picker.read(cx).is_open()), "real trigger opens the popup");
    click_colour("cursor-colour-tab-continuous", cx);
    assert_eq!(view.read_with(cx, |view, cx| view.color_editor.as_ref().unwrap().picker.read(cx).active_tab()), 1);
    view.read_with(cx, |view, _| view.color_editor.as_ref().unwrap().area.clone())
}

#[gpui_kit::gpui::test]
fn colour_continuous_pointer_changes_only_draft_and_escape_cancels_capture(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_terminal_cursor_color("#FF0000");
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    let area = open_continuous_colour(&view, cx);
    let bounds = cx.debug_bounds("hsv-sv-plane").unwrap();
    cx.simulate_mouse_down(bounds.center(), gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());
    draw(cx);
    // Successive real pointer events keep capture and the area's authoritative
    // RGBA bytes even near half-byte rounding boundaries.
    for saturation in [0.3, 0.31, 0.32, 0.49, 0.7] {
        let target = gpui_kit::point(bounds.left() + bounds.size.width * saturation, bounds.center().y);
        cx.simulate_mouse_move(target, gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());
        draw(cx);
        assert!(area.read_with(cx, |area, _| area.is_dragging()), "draft synchronization cancelled a held drag at S={saturation}, V=0.5");
        let hsv = area.read_with(cx, |area, _| area.hsv());
        assert!((hsv[1] - saturation).abs() < 0.001 && (hsv[2] - 0.5).abs() < 0.001);
        if saturation == 0.31 {
            assert_eq!(view.read_with(cx, |view, cx| view.text_drafts[&TextSetting::CursorColor].input.read(cx).value().to_string()), "#805858");
        }
    }
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#FF0000");
    assert!(!fixture.store.borrow().path.exists());
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(!view.read_with(cx, |view, cx| view.color_editor.as_ref().unwrap().picker.read(cx).is_open()));
    assert!(!area.read_with(cx, |area, _| area.is_dragging()));
    let stopped = area.read_with(cx, |area, _| area.value());
    cx.simulate_mouse_move(bounds.bottom_right(), gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());
    cx.simulate_mouse_up(bounds.bottom_right(), gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());
    draw(cx);
    assert_eq!(area.read_with(cx, |area, _| area.value()), stopped);
    click_colour("cursor-color-cancel", cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#FF0000");
    assert_eq!(view.read_with(cx, |view, cx| view.text_drafts[&TextSetting::CursorColor].input.read(cx).value().to_string()), "#FF0000");
    assert!(!fixture.store.borrow().path.exists());
}

#[gpui_kit::gpui::test]
fn colour_recent_palette_adds_only_successfully_applied_values(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.store.borrow_mut().set_terminal_cursor_color("#123456");
    let (view, cx) = open(cx, &fixture, SettingsPageId::TermCursor);
    focus_value("#123456", cx);
    replace("#112233", cx);
    click_colour("cursor-color-cancel", cx);
    assert!(view.read_with(cx, |view, _| view.color_editor.as_ref().unwrap().recent.is_empty()));
    focus_value("#123456", cx);
    replace("#FF000080", cx);
    click_colour("cursor-color-apply", cx);
    assert_eq!(view.read_with(cx, |view, _| view.color_editor.as_ref().unwrap().recent.len()), 1);
    focus_value("#FF000080", cx);
    replace("rgba(255, 0, 0, 0.5)", cx);
    click_colour("cursor-color-apply", cx);
    assert_eq!(view.read_with(cx, |view, _| view.color_editor.as_ref().unwrap().recent.len()), 1);
    focus_value("#FF000080", cx);
    replace("#00FF00", cx);
    click_colour("cursor-color-apply", cx);
    assert_eq!(view.read_with(cx, |view, _| view.color_editor.as_ref().unwrap().recent.len()), 2);
    click_colour("cursor-color-picker", cx);
    click_colour("cursor-recent-1", cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#00FF00");
    assert_eq!(view.read_with(cx, |view, cx| view.text_drafts[&TextSetting::CursorColor].input.read(cx).value().to_string()), "#FF000080");
    click_colour("cursor-color-cancel", cx);
    assert_eq!(fixture.store.borrow().terminal_cursor_color(), "#00FF00");
}
