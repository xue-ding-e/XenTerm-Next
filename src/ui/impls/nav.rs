//! The navigation rail: one icon per page, and the page switch on click.
//!
//! This is the window's spine. Every surface the shell used to juggle — the
//! session list, the resource panel, the file panel, the settings and plugin
//! overlays — is a page now, and the rail is the only way between them. A
//! click *switches* the page: nothing scrolls, nothing slides, the active
//! page's element tree is replaced wholesale, which is what makes the pages
//! diffable — only one of them is mounted in any frame.

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    v_flex,
    ActiveTheme as _,
    Sizable as _,
};
use gpui_kit::prelude::*;
use gpui_kit::{px, Context, SharedString};

use super::pages::{PageId, NAV_PAGES};
use super::shell::Shell;

/// How wide the rail is. A constant rather than a measured value because the
/// terminal's pane arithmetic needs the number at build time: the pane area's
/// width is *derived* from the window's — the canvas's own report cannot be
/// trusted (see `TerminalPage`) — and a derived width needs every fixed column
/// written down.
pub(crate) const RAIL_WIDTH: f32 = 56.0;

/// The rail itself.
pub(crate) fn render_nav_rail(active: PageId, cx: &mut Context<Shell>) -> impl IntoElement {
    let border = cx.theme().border;

    v_flex()
        .w(px(RAIL_WIDTH))
        .h_full()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .py_2()
        .border_r_1()
        .border_color(border)
        .children(NAV_PAGES.iter().map(|page| {
            let id = *page;
            let selected = active == id;
            Button::new(SharedString::from(format!("nav-{:?}", id)))
                .icon(id.icon())
                .small()
                .when(selected, |button| button.primary())
                .when(!selected, |button| button.ghost())
                .tooltip(id.title())
                .accessibility_label(id.title())
                .on_click(cx.listener(move |shell, _, window, cx| {
                    shell.open_page(id, window, cx);
                }))
        }))
}
