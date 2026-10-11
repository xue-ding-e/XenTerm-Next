//! First-frame language follows the ConfigStore already selected and loaded by startup.
//! The app intentionally has one process-global language shared by its windows.
use super::*;
use gpui_kit::gpui::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestSupportExt as _;
use std::sync::Mutex;

struct RestoreLanguage(bool, String);
impl Drop for RestoreLanguage {
    fn drop(&mut self) {
        crate::i18n::set_language(if self.0 { "en" } else { "zh" });
        gpui_kit::component::set_locale(&self.1);
    }
}

fn profile(language: &str) -> crate::config::ConfigStore {
    let mut store = crate::config::ConfigStore {
        path: std::env::temp_dir().join(format!("startup-language-{}.db", uuid::Uuid::new_v4())),
        backup_dir: None,
        cache: crate::config::ConfigFile::default(),
        key: [7; 32],
        keyring_enabled: false,
        saved_state: Mutex::new(crate::config::SavedState::default()).into(),
    };
    store.set_language(language.into());
    store
}

struct FirstFrame;
impl Render for FirstFrame {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let label = crate::i18n::t("状态", "Status");
        div()
            .id("startup-language-probe")
            .test_support()
            .aria_label(label)
            .child(label)
    }
}

fn assert_first_window(cx: &mut TestAppContext, english: bool) {
    let (_, cx) = cx.add_window_view(|_, _| FirstFrame);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let actual = cx.update(|window, _| {
        window
            .find("startup-language-probe")
            .label()
            .unwrap()
            .to_string()
    });
    assert_eq!(actual, if english { "Status" } else { "状态" });
    assert_eq!(
        &*gpui_kit::component::locale(),
        if english { "en" } else { "zh-CN" }
    );
}

fn exercise(language: &str, english: bool, cx: &mut TestAppContext) {
    let _restore = RestoreLanguage(
        crate::i18n::is_en(),
        gpui_kit::component::locale().to_string(),
    );
    // These are the actual defaults of a fresh app process before profile loading.
    crate::i18n::set_language("zh");
    gpui_kit::component::set_locale("en");
    let store = profile(language);
    initialize_ui_language(&store);
    cx.update(gpui_kit::init);
    assert_first_window(cx, english);
    assert_eq!(
        store.language(),
        if language.is_empty() { "zh" } else { language }
    );
}

#[gpui_kit::gpui::test]
fn saved_english_is_applied_before_first_window(cx: &mut TestAppContext) {
    exercise("en", true, cx);
}
#[gpui_kit::gpui::test]
fn saved_chinese_is_applied_before_first_window(cx: &mut TestAppContext) {
    exercise("zh", false, cx);
}
#[gpui_kit::gpui::test]
fn empty_language_keeps_existing_chinese_default(cx: &mut TestAppContext) {
    exercise("", false, cx);
}
#[gpui_kit::gpui::test]
fn unknown_language_keeps_existing_chinese_fallback(cx: &mut TestAppContext) {
    exercise("not-a-supported-locale", false, cx);
}
#[gpui_kit::gpui::test]
fn selected_profile_initializes_the_shared_language_for_multiple_windows(cx: &mut TestAppContext) {
    let _restore = RestoreLanguage(
        crate::i18n::is_en(),
        gpui_kit::component::locale().to_string(),
    );
    crate::i18n::set_language("zh");
    let selected = profile("en");
    let unrelated = profile("zh");
    assert_ne!(selected.path, unrelated.path);
    initialize_ui_language(&selected);
    cx.update(gpui_kit::init);
    assert_first_window(cx, true);
    assert_first_window(cx, true);
    // A second window shares the chosen app language; it is not an implicit profile switch.
    assert_eq!(unrelated.language(), "zh");
    assert_eq!(selected.language(), "en");
}
