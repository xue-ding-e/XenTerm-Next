//! The settings page.
//!
//! The full-size overlay asked for 1400×2000 so its rows would clear the
//! toolkit's stacked-layout threshold — a constant tuned against a card that
//! was clamped to a window it could not know the size of. As a page the width
//! is the window's, the clamp is gone, and the rows lay themselves out side by
//! side the way the threshold was meant to give them.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::{div, Entity, Window};

use crate::config::ConfigStore;
use crate::ui::{SettingsAction, SettingsView};

pub(crate) struct SettingsPage {
    view: Entity<SettingsView>,
}

impl SettingsPage {
    pub(crate) fn new(store: Rc<RefCell<ConfigStore>>, cx: &mut Context<Self>) -> Self {
        Self {
            view: cx.new(|_| SettingsView::new(store)),
        }
    }

    /// The next thing the page is asking the shell for, if any.
    pub(crate) fn take_action(&mut self, cx: &mut Context<Self>) -> Option<SettingsAction> {
        self.view.update(cx, |view, _| view.take_action())
    }

    /// A new view, built after the process language changed. The toolkit bakes
    /// its own placeholders in at construction, so a rebuild is the whole fix —
    /// the shell calls this instead of repainting.
    pub(crate) fn rebuild(&mut self, store: Rc<RefCell<ConfigStore>>, cx: &mut Context<Self>) {
        self.view = cx.new(|_| SettingsView::new(store));
        cx.notify();
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().overflow_hidden().child(self.view.clone())
    }
}
