//! Value types for one window's open tabs.

/// What kind of tab this is.
///
/// Replaces the stringly-typed `kind` the projected row carried
/// (`"welcome" | "terminal"`). Both readers of it were a comparison against a
/// string literal — `tab_transfer` deciding whether a dragged tab is a real
/// session tab that can be torn off into its own window — so a typo there would
/// have silently made every tab undetachable rather than failing to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    /// The start page. One per window, never closed, and no session behind it,
    /// which is why it cannot be detached or renamed.
    Welcome,
    /// A session tab: SSH, local shell, serial or telnet.
    Terminal,
}



/// One tab's identity and title.
///
/// Order is not here. Which pane a tab belongs to, its position within that
/// pane's strip, and which tab the pane shows all live in `crate::layout::Layout`
/// (`Leaf { tabs, active }`), which already owned them before this type existed.
/// Duplicating a second ordering here would give the two something to disagree
/// about; this is keyed by tab id and the layout decides what order to walk it
/// in.
///
/// `connected` is not here either. It is exactly `TabStatus::state == 1` in
/// `crate::resource`, so keeping a copy of it here would mean two owners for one
/// fact; the projection derives it from the status instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabMeta {
    pub kind: TabKind,
    /// The title with no user rename in force: the translated start-page label
    /// for a welcome tab, the saved session's name for a session tab.
    derived_title: String,
    /// The user's rename, while one is in force.
    title_override: Option<String>,
}

impl TabMeta {
    pub fn new(kind: TabKind, derived_title: String) -> Self {
        Self {
            kind,
            derived_title,
            title_override: None,
        }
    }

    /// The title to show. A rename wins over the derived title for as long as it
    /// stands; clearing it falls back rather than freezing what was typed.
    pub fn title(&self) -> &str {
        self.title_override
            .as_deref()
            .unwrap_or(&self.derived_title)
    }

    /// Replace the title that a rename falls back to, leaving any rename in
    /// force untouched — the user's own wording outranks both of these.
    ///
    /// Two callers: the UI language changing retranslates the start-page label,
    /// and clearing a rename writes back the session's name as the config has it
    /// now, which may not be the name the tab opened under.

    /// Put a user rename in force, or lift the existing one with `None`.
    pub fn set_override(&mut self, name: Option<String>) {
        self.title_override = name;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_tab_shows_its_derived_title() {
        let meta = TabMeta::new(TabKind::Terminal, "web-01".into());
        assert_eq!(meta.title(), "web-01");
    }

    #[test]
    fn a_rename_outranks_the_derived_title() {
        let mut meta = TabMeta::new(TabKind::Terminal, "web-01".into());
        meta.set_override(Some("prod".into()));
        assert_eq!(meta.title(), "prod");
    }

    #[test]
    fn clearing_a_rename_falls_back_to_the_derived_title() {
        let mut meta = TabMeta::new(TabKind::Terminal, "web-01".into());
        meta.set_override(Some("prod".into()));
        meta.set_override(None);
        assert_eq!(meta.title(), "web-01");
    }


}
