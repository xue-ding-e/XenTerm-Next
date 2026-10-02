//! The pages: one variant of [`PageId`] each, one `Entity` each, mounted one at
//! a time.
//!
//! The rule every page follows is the same, and it is what keeps the shell from
//! growing back into the single view it came from:
//!
//! - A page is a view of its own with its own state. It is created the first
//!   time it is entered and **kept** after that — switching away unmounts the
//!   element tree, never the entity — so a half-typed filter, an expanded tree
//!   or a scroll position survive a round trip.
//! - A page reports what it cannot do through a `take_action` queue, drained
//!   by the shell at the top of the frame. The shell is what holds the session
//!   handles, the dialogs and the detached windows; a page holds everything
//!   else.
//! - The shell renders exactly one page per frame. That is the differential:
//!   there is no anchor to scroll to and no hidden panel to keep in step.
//!
//! Only what earns the whole content area is a page — the terminal workspace,
//! the connections, the plugins and the settings. Everything glanceable (the
//! resource sidebar, the file panel, the transfers, the tunnels) is a panel
//! or a dialog beside the work, where the original put it and where a page
//! would only waste space.

// The full Lucide catalog lives in the assets crate; the component's own
// `IconName` is a deliberately short list that omits most of these.
use gpui_kit::assets::IconName;
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, Entity};

pub(crate) mod sessions_page;
pub(crate) mod settings_page;
pub(crate) mod terminal_page;

pub(crate) use sessions_page::{SessionsAction, SessionsPage};
pub(crate) use settings_page::SettingsPage;
pub(crate) use terminal_page::{TerminalAction, TerminalPage};

/// A page the navigation rail can switch to. The order of the rail is the
/// declaration order of [`NAV_PAGES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageId {
    /// The terminal workspace: the tab strip, the panes, the command bar, the
    /// resource sidebar and the file dock.
    Terminal,
    /// The saved connections and the built-in shells.
    Sessions,
    /// The preferences.
    Settings,
}

/// The rail's contents, in rail order.
pub(crate) const NAV_PAGES: &[PageId] = &[
    PageId::Terminal,
    PageId::Sessions,
    PageId::Settings,
];

impl PageId {
    pub(crate) fn icon(self) -> IconName {
        match self {
            Self::Terminal => IconName::Terminal,
            Self::Sessions => IconName::Server,
            Self::Settings => IconName::Settings,
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Terminal => crate::i18n::t("终端", "Terminal"),
            Self::Sessions => crate::i18n::t("连接", "Sessions"),
            Self::Settings => crate::i18n::t("设置", "Settings"),
        }
    }
}

/// Every page's entity, whether or not it has been entered yet.
///
/// The terminal page is the exception: it is the window's reason to exist, it
/// holds the first tab, and it is built with the window. The rest are built on
/// first entry, by the shell's `ensure_page`, and live for the rest of the
/// window's life.
pub(crate) struct Pages {
    pub(crate) active: PageId,
    pub(crate) terminal: Entity<TerminalPage>,
    pub(crate) sessions: Option<Entity<SessionsPage>>,
    pub(crate) settings: Option<Entity<SettingsPage>>,
}

impl Pages {
    pub(crate) fn new(terminal: Entity<TerminalPage>) -> Self {
        Self {
            active: PageId::Terminal,
            terminal,
            sessions: None,
            settings: None,
        }
    }

    /// The page to mount this frame. The caller has run `ensure_page` first,
    /// so the entity exists; the empty `div` is for the one frame where that
    /// has not happened yet, and shows nothing at all.
    pub(crate) fn render_active(&self) -> AnyElement {
        match self.active {
            PageId::Terminal => Some(self.terminal.clone().into_any_element()),
            PageId::Sessions => self
                .sessions
                .as_ref()
                .map(|page| page.clone().into_any_element()),
            PageId::Settings => self
                .settings
                .as_ref()
                .map(|page| page.clone().into_any_element()),
        }
        .unwrap_or_else(|| div().size_full().into_any_element())
    }
}
