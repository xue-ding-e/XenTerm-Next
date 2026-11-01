//! The terminal font size, which is three values rather than one.
//!
//! The Settings stepper owns a persisted size. Each window keeps a live base
//! that Ctrl+Shift+zoom moves without persisting it. Each tab may then pin
//! itself to a size of its own. The three resolve into the size a terminal is
//! actually drawn at: the tab's own size when it has one, the window's base
//! otherwise. This is the Rust side of those values, so the resolution has one
//! home rather than one per toolkit.

use std::collections::HashMap;

/// The Settings stepper's range, and the range every zoom clamps to.
pub const FONT_SIZE_MIN: u8 = 8;
pub const FONT_SIZE_MAX: u8 = 32;

/// One window's terminal font size: a live base, plus per-tab overrides.
///
/// Sizes come in as `i32` because that is what both a toolkit property and a
/// zoom step naturally produce, and go out as `u8` because a size that has been
/// through `clamp_size` cannot be outside 8..=32.
#[derive(Clone, Debug)]
pub struct FontZoom {
    base: u8,
    /// Tabs pinned to their own size. Absent means "follow the base".
    ///
    /// The former projected row carried `0` for that case, because an integer
    /// property has no absent state. Here there is no sentinel to mistake for a
    /// size, which is what the `row.font_size > 0` tests scattered through the
    /// zoom callback were working around.
    overrides: HashMap<String, u8>,
}

/// What a window-wide zoom changed, so only the affected rows are re-projected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseZoom {
    /// The window's new base size.
    pub base: u8,
    /// Tabs that had an override before the zoom and follow the base again now.
    pub cleared: Vec<String>,
}

impl FontZoom {
    pub fn new(base: i32) -> Self {
        Self {
            base: clamp_size(base),
            overrides: HashMap::new(),
        }
    }

    /// The window's base size, before any per-tab override.
    pub fn base(&self) -> u8 {
        self.base
    }

    /// A tab's own size, or `None` when it follows the window base.
    pub fn override_of(&self, tab_id: &str) -> Option<u8> {
        self.overrides.get(tab_id).copied()
    }

    /// Adopt the persisted Settings size as the window base.
    ///
    /// Per-tab overrides survive this. They are the user's explicit choice for
    /// one session, and changing a setting is not a request to discard them —
    /// only the window-wide zoom shortcut does that, and says so.
    pub fn set_base(&mut self, base: i32) {
        self.base = clamp_size(base);
    }

    /// Ctrl+= / Ctrl+- on one tab: step from wherever that tab currently is.
    ///
    /// Stepping from the tab's own size rather than the window base is what
    /// makes repeated presses accumulate. Reading the base instead would pin
    /// the tab one step away from it forever.
    pub fn zoom(&mut self, tab_id: &str, delta: i32) -> u8 {
        let from = self.override_of(tab_id).unwrap_or(self.base);
        let next = clamp_size(from as i32 + delta);
        self.overrides.insert(tab_id.to_string(), next);
        next
    }

    /// Ctrl+0 on one tab: pin it to the Settings size.
    ///
    /// This records an override equal to the Settings size rather than clearing
    /// the tab's override, so the tab stays put even if the window base is
    /// zoomed afterwards. That is what "back to the size I chose in Settings"
    /// has to mean once the window has drifted from it.
    pub fn reset(&mut self, tab_id: &str, settings_size: i32) -> u8 {
        let size = clamp_size(settings_size);
        self.overrides.insert(tab_id.to_string(), size);
        size
    }

    /// Ctrl+Shift+= / Ctrl+Shift+- / Ctrl+Shift+0: move the window base and
    /// drop every per-tab override.
    ///
    /// The overrides have to go or the shortcut would not visibly apply to the
    /// tabs that have one, which defeats the point of a window-wide zoom.
    /// Reporting them lets the caller re-project exactly those rows instead of
    /// walking the whole model looking for tabs whose size changed.
    pub fn zoom_base(&mut self, delta: i32, settings_size: i32) -> BaseZoom {
        self.base = if delta == 0 {
            clamp_size(settings_size)
        } else {
            clamp_size(self.base as i32 + delta)
        };
        let mut cleared: Vec<String> = self.overrides.keys().cloned().collect();
        // Sorted so a window-wide zoom touches rows in a stable order rather
        // than whatever order the map happens to iterate in.
        cleared.sort();
        self.overrides.clear();
        BaseZoom {
            base: self.base,
            cleared,
        }
    }

    /// Take a tab's override out. Used both to drop it when the tab closes and
    /// to hand it to another window when the tab is detached or merged.
    pub fn take_override(&mut self, tab_id: &str) -> Option<u8> {
        self.overrides.remove(tab_id)
    }

    /// Adopt an override arriving from another window.
    pub fn insert_override(&mut self, tab_id: &str, size: i32) {
        self.overrides.insert(tab_id.to_string(), clamp_size(size));
    }
}

/// Clamp to the range the Settings stepper offers.
///
/// Takes the `i32` a zoom step naturally produces so the arithmetic cannot wrap
/// a `u8` before it is clamped — stepping down from the minimum would otherwise
/// have to be caught after the fact.
fn clamp_size(size: i32) -> u8 {
    size.clamp(FONT_SIZE_MIN as i32, FONT_SIZE_MAX as i32) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoom() -> FontZoom {
        FontZoom::new(14)
    }

    #[test]
    fn a_new_tab_has_no_override_and_follows_the_base() {
        let z = zoom();
        assert_eq!(z.base(), 14);
        assert_eq!(z.override_of("t1"), None);
    }

    #[test]
    fn an_out_of_range_base_is_clamped_on_construction() {
        assert_eq!(FontZoom::new(200).base(), FONT_SIZE_MAX);
        assert_eq!(FontZoom::new(1).base(), FONT_SIZE_MIN);
    }

    #[test]
    fn repeated_zooms_on_one_tab_accumulate() {
        let mut z = zoom();
        assert_eq!(z.zoom("t1", 1), 15);
        assert_eq!(
            z.zoom("t1", 1),
            16,
            "the second press steps from 15, not from the base"
        );
        assert_eq!(z.zoom("t1", -1), 15);
        assert_eq!(z.override_of("t1"), Some(15));
    }

    #[test]
    fn zooming_one_tab_leaves_the_base_and_the_other_tabs_alone() {
        let mut z = zoom();
        z.zoom("t1", 4);

        assert_eq!(
            z.base(),
            14,
            "the window base is untouched by a single-tab zoom"
        );
        assert_eq!(z.override_of("t2"), None);
    }

    #[test]
    fn zoom_clamps_at_both_ends_of_the_stepper_range() {
        let mut z = zoom();
        for _ in 0..40 {
            z.zoom("big", 1);
            z.zoom("small", -1);
        }
        assert_eq!(z.override_of("big"), Some(FONT_SIZE_MAX));
        assert_eq!(z.override_of("small"), Some(FONT_SIZE_MIN));
    }

    #[test]
    fn zooming_down_from_the_minimum_does_not_wrap() {
        let mut z = FontZoom::new(FONT_SIZE_MIN as i32);
        assert_eq!(z.zoom("t1", -1), FONT_SIZE_MIN);
    }

    #[test]
    fn reset_pins_a_tab_to_the_settings_size_even_after_the_window_drifted() {
        let mut z = zoom();
        z.zoom("t1", 6);
        z.set_base(20);

        assert_eq!(z.reset("t1", 14), 14);
        assert_eq!(
            z.override_of("t1"),
            Some(14),
            "reset must leave an override behind, not clear it — otherwise the tab would jump to the drifted base of 20"
        );
    }

    #[test]
    fn adopting_the_settings_size_keeps_per_tab_overrides() {
        let mut z = zoom();
        z.zoom("t1", 4);
        assert_eq!(z.override_of("t1"), Some(18));

        z.set_base(20);

        assert_eq!(z.base(), 20);
        assert_eq!(
            z.override_of("t1"),
            Some(18),
            "the tab stays where the user put it rather than snapping to the new base"
        );
    }

    #[test]
    fn a_window_wide_zoom_drops_every_override_and_reports_which() {
        let mut z = zoom();
        z.zoom("t1", 4);
        z.zoom("t2", -2);
        assert_eq!(z.override_of("t3"), None);

        let change = z.zoom_base(1, 14);

        assert_eq!(change.base, 15);
        assert_eq!(change.cleared, ["t1", "t2"], "t3 had no override to clear");
        assert_eq!(z.override_of("t1"), None);
        assert_eq!(z.override_of("t2"), None);
    }

    #[test]
    fn a_window_wide_reset_returns_to_the_settings_size() {
        let mut z = FontZoom::new(30);
        z.zoom("t1", -4);

        let change = z.zoom_base(0, 14);

        assert_eq!(change.base, 14);
        assert_eq!(change.cleared, ["t1"]);
    }

    #[test]
    fn cleared_tabs_are_reported_in_a_stable_order() {
        let mut z = zoom();
        for id in ["delta", "alpha", "charlie", "bravo"] {
            z.zoom(id, 1);
        }

        let change = z.zoom_base(1, 14);

        assert_eq!(change.cleared, ["alpha", "bravo", "charlie", "delta"]);
    }

    #[test]
    fn an_override_moves_to_another_window_with_its_tab() {
        let mut src = zoom();
        src.zoom("t1", 6);
        let mut dst = FontZoom::new(11);

        let moved = src.take_override("t1");
        assert_eq!(moved, Some(20));
        if let Some(size) = moved {
            dst.insert_override("t1", size as i32);
        }

        assert_eq!(src.override_of("t1"), None);
        assert_eq!(dst.override_of("t1"), Some(20));
        assert_eq!(
            dst.base(),
            11,
            "the destination window's own base is not disturbed"
        );
    }

    #[test]
    fn taking_an_override_that_was_never_set_changes_nothing() {
        let mut z = zoom();
        assert_eq!(z.take_override("nope"), None);
        assert_eq!(z.take_override("nope"), None, "and it stays absent");
    }
}
