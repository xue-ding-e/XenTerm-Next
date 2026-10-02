//! Tiny runtime internationalisation.
//!
//! Every translatable string in the UI is a pair of literals handed to [`t`],
//! which returns the Chinese or English variant based on the current language
//! flag. There is no translation catalogue to load: both languages ship in the
//! binary, and switching is a flag flip followed by a repaint.

use std::sync::atomic::{AtomicU8, Ordering};

const ZH: u8 = 0;
const EN: u8 = 1;

static LANG: AtomicU8 = AtomicU8::new(ZH);

/// Apply a language code (`"zh"` or `"en"`). Takes effect on the next repaint.
pub fn set_language(code: &str) {
    let en = code.eq_ignore_ascii_case("en");
    LANG.store(if en { EN } else { ZH }, Ordering::Relaxed);
}

/// Current language code, for persisting to config (`"zh"` / `"en"`).

pub fn is_en() -> bool {
    LANG.load(Ordering::Relaxed) == EN
}

/// Pick the variant for the current language: `zh` is Chinese, `en` is English.
pub fn t(zh: &'static str, en: &'static str) -> &'static str {
    if is_en() {
        en
    } else {
        zh
    }
}
