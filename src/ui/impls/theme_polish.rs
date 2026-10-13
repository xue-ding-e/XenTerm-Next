//! The application's global theme customization.
//!
//! Everything here is the layer between the toolkit's stock themes and what
//! this app ships: a slightly rounder corner family, applied at every mode
//! change so it survives light/dark switching (a bare `Theme::global_mut`
//! write is wiped the next time `Theme::change` re-applies the registry
//! config). One door — [`set_mode`] — is the only way this crate changes the
//! theme; the startup path, the settings dropdown and the command palette all
//! go through it.
//!
//! The values are picked for the surfaces this app actually draws: `radius`
//! rounds everything interactive (buttons, inputs, chips, rows — the toolkit
//! reads it through `theme().radius`), and `radius_lg` rounds the things that
//! float (dialogs, notifications, popovers). Two tiers is the whole hierarchy:
//! bigger radius means "floats above the window", smaller means "sits in it".

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{px, Window};

/// Apply the preferred theme mode and this app's customization on top.
///
/// `pref` is the stored setting: `"dark"`, `"light"`, or `"system"` (and any
/// unrecognized value falls back to following the system, which is what a
/// missing preference always meant).
pub(crate) fn set_mode(pref: &str, window: Option<&mut Window>, cx: &mut gpui_kit::App) {
    match pref {
        "dark" => Theme::change(ThemeMode::Dark, window, cx),
        "light" => Theme::change(ThemeMode::Light, window, cx),
        _ => Theme::sync_system_appearance(window, cx),
    }
    polish(cx);
}

/// The customization itself: idempotent, mode-independent.
///
/// Run after every mode change. `Theme::change` re-applies the registry
/// config (radius 6/8 from the stock shadcn defaults), so this writes over
/// it rather than hoping a one-time write survived.
fn polish(cx: &mut gpui_kit::App) {
    let theme = Theme::global_mut(cx);
    // One notch rounder than stock (6/8): at 6 the difference between a
    // rounded input and a square panel edge is hard to name; at 8 the
    // interactive layer reads as its own layer. Large surfaces (dialogs,
    // notifications — everything on `radius_lg`) go one step further so
    // "floats above the window" has a shape of its own.
    theme.radius = px(8.);
    theme.radius_lg = px(10.);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::TestAppContext;

    /// The customization survives a mode change: `Theme::change` re-applies
    /// the registry config (radius 6/8), and `polish` must land after it —
    /// which is exactly what `set_mode` exists to order. This test fails if
    /// anyone calls `Theme::change` directly and skips the polish.
    #[gpui_kit::gpui::test]
    fn mode_change_keeps_the_rounded_family(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            set_mode("dark", None, cx);
            let theme = Theme::global(cx);
            assert_eq!(theme.radius, px(8.), "interactive radius is the app's");
            assert_eq!(theme.radius_lg, px(10.), "floating radius is one step up");
            assert!(theme.is_dark());
        });
        // And the other direction: the polish is mode-independent.
        cx.update(|cx| {
            set_mode("light", None, cx);
            let theme = Theme::global(cx);
            assert_eq!(theme.radius, px(8.));
            assert!(!theme.is_dark());
        });
    }
}
