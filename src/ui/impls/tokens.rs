//! The window's layout metrics, in one place.
//!
//! These are the numbers two or more surfaces have to agree on: the rail the
//! pages hang off, the columns the workspace subtracts to size the terminal,
//! the drag band the resize gestures hit-test against. A constant that only one
//! file owns stays in that file — this module is for the ones whose second copy
//! would be a disagreement waiting to happen.
//!
//! Colors and type sizes are *not* here: they come from `cx.theme()` and the
//! toolkit's type scale, which are the token systems this crate already uses.

/// The navigation rail: one icon per page, always this wide.
pub(crate) const RAIL_WIDTH: f32 = 56.0;

/// Below this width the resource panel's CPU/Memory/Swap rows stop fitting
/// their three columns. There is no fixed maximum: the current viewport and
/// its other columns determine how much room the panel can take.
pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 180.0;
/// The column's width when collapsed: enough for one icon button and a hairline.
pub(crate) const SIDEBAR_COLLAPSED_WIDTH: f32 = 36.0;

/// Keep a usable terminal/management area beside either resizable sidebar.
/// This is reserved space, not a cap on how wide a sidebar may grow.
pub(crate) const SIDEBAR_MAIN_MIN_WIDTH: f32 = 280.0;

/// Available logical pixels exclude the navigation rail and any right dock.
/// Tiny or transient invalid viewports must never put the divider off-screen.
pub(crate) fn sidebar_width_limit(available: f32) -> f32 {
    if available.is_finite() {
        (available - SIDEBAR_MAIN_MIN_WIDTH).max(0.0)
    } else {
        0.0
    }
}

/// The SFTP dock's width when it docks to the right: a column wide enough for
/// the tree, the name and the two figure columns at once.
pub(crate) const DOCK_STRIP_WIDTH: f32 = 420.0;
/// Bounds on the bottom dock's height: a dock of zero rows is a divider, and
/// one taller than the terminal is a file list wearing the terminal's clothes.
pub(crate) const DOCK_MIN_HEIGHT: f32 = 120.0;
pub(crate) const DOCK_MAX_HEIGHT: f32 = 600.0;

/// The drag band's half-width around the sidebar's right edge and the bottom
/// dock's top edge, in window pixels. Wide enough to grab, narrow enough not
/// to eat clicks meant for the panel beside it.
pub(crate) const RESIZE_BAND: f32 = 5.0;

/// The SFTP listing's columns, in both the headings and the rows — the two
/// have to agree for the table to line up, which is why they are named rather
/// than repeated.
pub(crate) const SFTP_TREE_WIDTH: f32 = 160.0;
pub(crate) const SFTP_SIZE_WIDTH: f32 = 96.0;
pub(crate) const SFTP_MTIME_WIDTH: f32 = 128.0;
/// The tick box's column: a button, so its width is its own.
pub(crate) const SFTP_TICK_WIDTH: f32 = 24.0;
