//! The settings view, and the label fix the capability registry left behind.
//!
//! ## The debt this view pays
//!
//! Four switches used to live under a page titled MODEL CONTEXT PROTOCOL, spelled
//! `mcp-enabled`, `mcp-use-saved-credentials`, `mcp-allow-commands` and
//! `mcp-allow-file-transfers`. Three of those four govern *plugins* as well, because a
//! plugin asking the host to run a command or move a file goes through the same gates —
//! `Frontend::is_unattended` is true for both callers, which is the whole reason that
//! method exists. So a plugin refused by one of them was pointed at a page that never
//! mentioned plugins.
//!
//! They now live under **Unattended access**, worded for both callers, and `mcp-enabled`
//! has moved to a separate MCP group, because it is *not* one of those gates: it is the
//! MCP server's own on-switch, and a plugin does not care whether `xenterm mcp serve`
//! is allowed to start. The persisted keys are unchanged — this is a presentation fix,
//! and rewriting the storage would turn a relabelling into a migration.
//!
//! ## What is here and what is not
//!
//! The original interface panel has thirteen pages. This has the ones that are decisions
//! about how the application behaves, in the groups they belong to; the rest arrive as
//! their settings are needed. A settings view that renders every knob before anything
//! reads them is a view that grows a control nobody wired.

use std::rc::Rc;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
        switch::Switch,
        ActiveTheme as _,
        v_flex, AxisExt as _, Icon, Sizable as _,
    },
    div,
    prelude::*,
    px, Axis, Context, Entity, FontWeight, IntoElement, Render, SharedString, Subscription,
    Window,
};

// The full Lucide catalog: the icons this page's own rows need are not in the curated
// component subset.
use gpui_kit::assets::IconName;

use crate::config::ConfigStore;

/// What the settings page asks the shell to do.
///
/// Recorded rather than done here, because the editor is a full-window overlay and the
/// shell is what owns the overlay slot — the same arrangement the session list uses for
/// its dialogs. The two connection actions are here for the same reason: reading
/// `~/.ssh/config` and writing an export both belong to the shell, which owns the store
/// and the status line they report on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsAction {
    /// Add a custom output-highlighting rule.
    NewHighlightRule,
    /// The window theme changed: "system", "dark" or "light".
    ///
    /// Carried as the value rather than as a resolved mode, because whether "system" means
    /// light or dark is the toolkit's answer and it is asked for at the window, not here.
    ThemeChanged(String),
    /// The interface language changed. The flag itself is already set by the time this is
    /// reported — it is process-global and the page applies it where the click lands — but
    /// only the page repaints on its own, so the shell has to repaint the window.
    LanguageChanged,
    /// Bring in the hosts from `~/.ssh/config`.
    ImportSshConfig,
    /// Write every saved connection to a file the user names.
    ExportSessions,
    /// Bring in the connections in this pasted text, one per line.
    ///
    /// The text travels with the action rather than being read back from the box, because
    /// the box belongs to this view and the import belongs to the shell: what crosses
    /// between them is the decision, not the widget.
    ImportPasted(String),
    /// Send the connection list to the configured WebDAV server.
    WebdavUpload,
    /// Bring the connection list back from the configured WebDAV server.
    WebdavDownload,
    /// Pick font file(s) the user names; they are copied into the import
    /// directory and registered for the rest of this launch. The picker runs
    /// in the shell, which has the file dialog and the text system.
    ImportFont,
    /// Open the import directory in the OS file manager, so imported fonts can
    /// be inspected or removed by deleting files.
    OpenFontFolder,
    /// Open the audit journal folder. The records themselves are not editable
    /// from here — they expire by whole days — but the folder is where they are.
    OpenAuditFolder,
}

/// The settings view.
/// One leaf of the settings navigation. Each leaf is a real page: clicking it
/// replaces the content pane, and there is no scroll-positioning anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsPageId {
    Interface,
    TermFont,
    TermCursor,
    TermInput,
    TermHighlight,
    Connections,
    Files,
    Sync,
    /// The former 权限 and MCP 服务器 pages, merged: one feature's switches,
    /// its server, and its approval gate belong on one page.
    McpPermissions,
}

/// The sidebar's structure: the multi-page sections a click folds, in display
/// order. A section header is the fold; the pages under it are its children.
/// Single-page subjects (界面 / 文件 / 同步) are not sections — a fold over one
/// entry is a click that does two things — so they stay direct entries beside
/// the sections.
const NAV_SECTIONS: &[(&str, &str, &[SettingsPageId])] = &[
    (
        "终端",
        "Terminal",
        &[
            SettingsPageId::TermFont,
            SettingsPageId::TermCursor,
            SettingsPageId::TermInput,
            SettingsPageId::TermHighlight,
        ],
    ),
];

/// The direct entries after the sections, in display order. 界面 sits above
/// the sections as the landing subject and is rendered on its own.
const NAV_DIRECT: &[(SettingsPageId, &str, &str, IconName)] = &[
    // Single-page subject, kept direct like 界面 and 文件: a fold header over
    // one row is navigation noise.
    (SettingsPageId::Connections, "连接", "Connections", IconName::Plug),
    (SettingsPageId::McpPermissions, "MCP 与权限", "MCP & permissions", IconName::ShieldCheck),
    (SettingsPageId::Files, "文件", "Files", IconName::FolderOpen),
    (SettingsPageId::Sync, "同步", "Sync", IconName::RefreshCw),
];

pub(crate) struct SettingsView {
    store: Rc<std::cell::RefCell<ConfigStore>>,
    /// The sub-page showing now. The sidebar offers sections with nested
    /// subjects; every entry — section or subject — switches to its own page.
    selected: SettingsPageId,
    /// Which sidebar sections are folded, by section name. Folding hides only
    /// the nav entries; the page showing is untouched, so a fold never yanks
    /// the content out from under the reader.
    folded_sections: std::collections::HashSet<&'static str>,
    /// How many times each section has turned, which is what makes its
    /// chevron swing rather than swap. Shared with the click handlers, which
    /// receive an `App` and so cannot reach `&mut self`.
    chevron_turn: super::chevron::TurnCounter,
    pending: Option<SettingsAction>,
    /// The paste box for batch import, created on the first frame that draws it.
    ///
    /// Lazily, because an input needs a window and this view is built without one — the
    /// same reason the session editor seeds its tables while rendering.
    paste: Option<Entity<gpui_kit::component::input::TextareaState>>,
    /// The WebDAV password box: masked and write-only (audit N-中5). The saved
    /// password is never read back into the form — typing replaces it, leaving
    /// it empty keeps it, matching the session editor's credential rows.
    webdav_password_input: Option<Entity<InputState>>,
    _webdav_password_subscription: Option<Subscription>,
    /// Raw edits outlive page switches. Normalization belongs at a commit
    /// boundary, not after each character of a colour or URL.
    text_drafts: std::collections::HashMap<TextSetting, TextDraft>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum TextSetting {
    CursorColor,
    WebdavUrl,
    WebdavUsername,
    WebdavPath,
}

struct TextDraft {
    input: Entity<InputState>,
    last_value: String,
    dirty: bool,
    error: Option<&'static str>,
    _subscription: Subscription,
}

impl TextSetting {
    fn read(self, store: &ConfigStore) -> String {
        match self {
            Self::CursorColor => store.terminal_cursor_color().to_string(),
            Self::WebdavUrl => store.webdav_url().to_string(),
            Self::WebdavUsername => store.webdav_username().to_string(),
            Self::WebdavPath => store.webdav_remote_path().to_string(),
        }
    }

    fn validate(self, value: &str) -> Result<(), &'static str> {
        let value = value.trim();
        match self {
            Self::CursorColor if !value.is_empty() && crate::config::hex_to_rgb(value).is_none() => {
                Err(crate::i18n::t(
                    "请输入六位十六进制颜色（例如 #123456），或留空恢复默认。尚未保存。",
                    "Enter six hexadecimal digits (for example #123456), or leave blank for the default. Not saved.",
                ))
            }
            Self::WebdavUrl if !value.is_empty() => {
                // Reuse the HTTP client's URL parser without calling/sending a
                // request. Offline addresses and local servers remain valid.
                let valid = !value.chars().any(char::is_control)
                    && value.split_once("://").is_some_and(|(scheme, _)|
                        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
                    && ureq::get(value).request_url().is_ok();
                if valid { Ok(()) } else {
                    Err(crate::i18n::t(
                        "请输入完整的 http:// 或 https:// 地址，或留空。尚未保存。",
                        "Enter a complete http:// or https:// address, or leave blank. Not saved.",
                    ))
                }
            }
            _ => Ok(()),
        }
    }

    fn apply(self, store: &mut ConfigStore, value: String) {
        match self {
            Self::CursorColor => {
                if value.trim().is_empty() {
                    // Empty is the existing configuration's default-colour
                    // sentinel, as advertised by this field's description.
                    store.cache.terminal_cursor_color.clear();
                } else {
                    store.set_terminal_cursor_color(&value);
                }
            }
            _ => {
                let (enabled, url, user, password, path, certs) = match self {
                    Self::WebdavUrl => webdav_with_url(store, value),
                    Self::WebdavUsername => webdav_with_username(store, value),
                    Self::WebdavPath => webdav_with_remote_path(store, value),
                    Self::CursorColor => unreachable!(),
                };
                store.set_webdav_settings(enabled, url, user, password, path, certs);
            }
        }
    }
}
impl SettingsView {
    pub(crate) fn new(store: Rc<std::cell::RefCell<ConfigStore>>) -> Self {
        Self {
            store,
            selected: SettingsPageId::Interface,
            // Everything starts open: a first-run reader should see the whole
            // map, and a fold is a choice, not a default.
            folded_sections: std::collections::HashSet::new(),
            chevron_turn: super::chevron::new_turn_counter(),
            pending: None,
            paste: None,
            webdav_password_input: None,
            _webdav_password_subscription: None,
            text_drafts: std::collections::HashMap::new(),
        }
    }

    /// Take the next action, if any.
    pub(crate) fn take_action(&mut self) -> Option<SettingsAction> {
        self.pending.take()
    }

    fn text_field(
        &mut self,
        kind: TextSetting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SettingField<SharedString> {
        if !self.text_drafts.contains_key(&kind) {
            let initial = kind.read(&self.store.borrow());
            let input = cx.new(|cx| InputState::new(window, cx).default_value(initial.clone()));
            let subscription = cx.subscribe_in(&input, window, move |view, input, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::Change => {
                        let draft = view.text_drafts.get_mut(&kind).expect("registered draft");
                        let value = input.read(cx).value().to_string();
                        // The toolkit can emit Change again after Enter with
                        // unchanged text. Keep validation feedback until an
                        // actual edit, and do not mark a successful save dirty.
                        if value == draft.last_value { return; }
                        draft.last_value = value;
                        draft.dirty = true;
                        draft.error = None;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        let dirty = view.text_drafts.get(&kind).is_some_and(|draft| draft.dirty);
                        if !dirty { return; }
                        let value = input.read(cx).value().to_string();
                        let result = kind.validate(&value).and_then(|()| {
                            let mut store = view.store.borrow_mut();
                            let before = store.cache.clone();
                            kind.apply(&mut store, value);
                            if store.save().is_err() {
                                store.cache = before;
                                Err(crate::i18n::t(
                                    "保存失败，输入已保留。请检查配置文件后按 Enter 重试。",
                                    "Could not save. Your input is kept; check the profile and press Enter to retry.",
                                ))
                            } else { Ok(kind.read(&store)) }
                        });
                        let draft = view.text_drafts.get_mut(&kind).expect("registered draft");
                        match result {
                            Ok(saved) => {
                                draft.dirty = false;
                                draft.error = None;
                                draft.last_value = saved.clone();
                                input.update(cx, |input, cx| input.set_value(saved, window, cx));
                            }
                            Err(error) => draft.error = Some(error),
                        }
                        cx.notify();
                    }
                    _ => {}
                }
            });
            self.text_drafts.insert(
                kind,
                TextDraft {
                    input,
                    last_value: initial,
                    dirty: false,
                    error: None,
                    _subscription: subscription,
                },
            );
        }
        let draft = self.text_drafts.get_mut(&kind).expect("created above");
        if !draft.dirty {
            let saved = kind.read(&self.store.borrow());
            if draft.input.read(cx).value().as_ref() != saved {
                draft.last_value = saved.clone();
                draft
                    .input
                    .update(cx, |input, cx| input.set_value(saved, window, cx));
            }
        }
        let input = draft.input.clone();
        let error = draft.error;
        SettingField::element(
            move |options: &gpui_kit::component::setting::RenderOptions,
                  _: &mut Window,
                  cx: &mut gpui_kit::App| {
                v_flex()
                    .id(SharedString::from(format!("settings-text-{kind:?}")))
                    .gap_1()
                    .map(|this| {
                        if options.layout().is_horizontal() {
                            this.w_64()
                        } else {
                            this.w_full()
                        }
                    })
                    .child(
                        Input::new(&input)
                            .w_full()
                            .disabled(options.is_disabled())
                            .with_size(options.size()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if error.is_some() {
                                cx.theme().danger
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(error.unwrap_or_else(|| {
                                crate::i18n::t(
                                    "按 Enter 或离开输入框保存。",
                                    "Press Enter or leave the field to save.",
                                )
                            })),
                    )
                    .into_any_element()
            },
        )
    }
}

/// Write one setting and persist it.
///
/// Immediate setters go through here, so "changed a setting" and "saved the file" cannot
/// drift apart: a settings view where one control forgot to save would be a control
/// that silently reverts on restart, which is the worst way for a preference to fail.
/// Text drafts instead validate and report errors at their explicit commit boundary.
///
/// A failure to save is logged rather than dialogued: the in-memory value is already
/// correct and the next change will try again, and interrupting someone mid-adjustment
/// with a modal about a file they did not know existed helps nobody.
fn persist(
    store: &Rc<std::cell::RefCell<ConfigStore>>,
    apply: impl FnOnce(&mut ConfigStore),
    what: &'static str,
) {
    let mut store = store.borrow_mut();
    apply(&mut store);
    if let Err(error) = store.save() {
        tracing::warn!("could not save {what}: {error:#}");
    }
}

/// The settings view.
impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Each page's groups are one builder method; this render is only the
        // navigation and the dispatch. The builders capture the same values
        // the inline sections did — the view's entity and the shared store —
        // only the naming moved.
        // Copied out of the theme up front: the entry closure needs `&mut cx`
        // for its click listeners, and a live `cx.theme()` borrow would fight it.
        let theme = cx.theme();
        let sidebar_bg = theme.sidebar;
        let sidebar_border = theme.sidebar_border;
        let sidebar_fg = theme.sidebar_foreground;
        let accent = theme.sidebar_accent;
        let accent_fg = theme.sidebar_accent_foreground;
        let radius = theme.radius;
        let muted_fg = theme.muted_foreground;
        let foreground = theme.foreground;
        let entry = |this: &mut Self,
                     selected: SettingsPageId,
                     label_zh: &'static str,
                     label_en: &'static str,
                     icon: IconName,
                     cx: &mut Context<Self>| {
            let is_active = this.selected == selected;
            h_flex()
                .id(SharedString::from(format!("settings-nav-{selected:?}")))
                .h_7()
                .w_full()
                .flex_shrink_0()
                .px_2()
                .gap_x_2()
                .rounded(radius)
                .text_sm()
                .cursor_pointer()
                .when(is_active, |this| {
                    this.font_weight(FontWeight::MEDIUM)
                        .bg(accent)
                        .text_color(accent_fg)
                })
                .when(!is_active, |this| {
                    this.text_color(sidebar_fg)
                        .hover(|this| this.bg(accent.opacity(0.8)))
                })
                .child(Icon::new(icon).size_4())
                .child(SharedString::from(crate::i18n::t(label_zh, label_en)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = selected;
                    cx.notify();
                }))
                .into_any_element()
        };

        // A section header is the fold: name at the group level (small, muted,
        // semibold — a different voice from the entries' normal weight), a
        // chevron that swings between states, a count badge while folded, and
        // an accent bar down its left edge while the page showing lives inside
        // it — because a folded section must still say where the selection is.
        let section = |this: &mut Self,
                       name_zh: &'static str,
                       name_en: &'static str,
                       pages: &[SettingsPageId],
                       cx: &mut Context<Self>| {
            let folded = this.folded_sections.contains(name_zh);
            let turn = this
                .chevron_turn
                .try_borrow()
                .ok()
                .and_then(|map| map.get(name_zh).copied());
            let contains_selected = pages.contains(&this.selected);
            let muted = sidebar_fg;
            let count = pages.len();
            let name = name_zh;
            // The bar is outside the rounded row: a hairline hugging the
            // sidebar's left edge reads as "this section holds what you are
            // looking at", not as "this row is hovered". Invisible (not
            // absent) while uninterested, so the row never shifts.
            let indicator = if contains_selected {
                accent
            } else {
                gpui_kit::transparent_black()
            };
            let header = h_flex()
                .id(SharedString::from(format!("settings-section-{name}")))
                .h_6()
                .w_full()
                .flex_shrink_0()
                .mt_1p5()
                .px_2()
                .gap_x_1p5()
                .rounded(radius)
                .border_l_2()
                .border_color(indicator)
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .text_color(if contains_selected {
                    muted
                } else {
                    muted.opacity(0.75)
                })
                .hover(|row| {
                    row.text_color(sidebar_fg)
                        .bg(gpui_kit::hsla(0.0, 0.0, 0.5, 0.08))
                })
                .child(super::chevron::folding_chevron(
                    &format!("settings-{name}"),
                    folded,
                    turn,
                    muted,
                ))
                .child(SharedString::from(crate::i18n::t(
                    name_zh,
                    name_en,
                )))
                .when(folded, |row| {
                    row.child(
                        div()
                            .text_xs()
                            .text_color(muted.opacity(0.7))
                            .child(SharedString::from(count.to_string())),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    // The view folds it and swings the chevron; the page
                    // showing is untouched — a fold is navigation housekeeping,
                    // not navigation.
                    if !this.folded_sections.remove(name) {
                        this.folded_sections.insert(name);
                    }
                    super::chevron::bump_turn(&this.chevron_turn, name);
                    cx.notify();
                }));
            if folded {
                header.into_any_element()
            } else {
                // The guide rail: one left border drawn by the container, so
                // the children read as *inside* the section rather than as a
                // stack of unrelated rows that happen to be indented.
                v_flex()
                    .w_full()
                    .flex_shrink_0()
                    .ml_2()
                    .border_l_1()
                    .border_color(sidebar_border)
                    .pl_1()
                    .gap_0p5()
                    .children(pages.iter().map(|page| match page {
                        SettingsPageId::TermFont => entry(
                            this,
                            SettingsPageId::TermFont,
                            "字体",
                            "Font",
                            IconName::Type,
                            cx,
                        ),
                        SettingsPageId::TermCursor => entry(
                            this,
                            SettingsPageId::TermCursor,
                            "光标",
                            "Cursor",
                            IconName::TextCursor,
                            cx,
                        ),
                        SettingsPageId::TermInput => entry(
                            this,
                            SettingsPageId::TermInput,
                            "输入",
                            "Input",
                            IconName::Keyboard,
                            cx,
                        ),
                        SettingsPageId::TermHighlight => entry(
                            this,
                            SettingsPageId::TermHighlight,
                            "输出高亮",
                            "Highlight",
                            IconName::Highlighter,
                            cx,
                        ),
                        // Sections only ever name their own children; the
                        // direct subjects never appear in NAV_SECTIONS.
                        _ => div().into_any_element(),
                    }))
                    .into_any_element()
            }
        };


        let sidebar_bg = theme.sidebar;
        let sidebar_border = theme.sidebar_border;
        let sidebar_fg = theme.sidebar_foreground;
        let accent = theme.sidebar_accent;
        let muted_fg = theme.muted_foreground;
        let foreground = theme.foreground;
        let sidebar = v_flex()
            .w(px(170.))
            .h_full()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(sidebar_bg)
            .border_r_1()
            .border_color(sidebar_border)
            .p_2()
            .gap_0p5()
            // 界面 — a subject of its own, no fold over one entry.
            .child(entry(
                self,
                SettingsPageId::Interface,
                "界面",
                "Interface",
                IconName::SlidersHorizontal,
                cx,
            ))
            .children(
                NAV_SECTIONS
                    .iter()
                    .map(|(name_zh, name_en, pages)| section(self, name_zh, name_en, pages, cx)),
            )
            // 文件 / 同步 — single-page subjects, kept direct for the same
            // reason 界面 is.
            .children(NAV_DIRECT.iter().map(|(page, zh, en, icon)| {
                entry(self, *page, zh, en, *icon, cx)
            }));

        let page = match self.selected {
            SettingsPageId::Interface => {
                SettingPage::new(crate::i18n::t("界面", "Interface"))
                    .group(self.appearance_group(window, cx))
            }
            SettingsPageId::TermFont => {
                SettingPage::new(crate::i18n::t("字体", "Font"))
                    .groups(self.font_and_cursor_groups(window, cx))
            }
            SettingsPageId::TermCursor => {
                // 光标 shares the font page's groups: its fields read the same
                // appearance values, so one builder builds both and this arm
                // filters to the cursor's half by rebuilding it.
                SettingPage::new(crate::i18n::t("光标", "Cursor"))
                    .groups(self.cursor_group_only(window, cx))
            }
            SettingsPageId::TermInput => {
                SettingPage::new(crate::i18n::t("输入", "Input")).group(self.input_group(window, cx))
            }
            SettingsPageId::TermHighlight => {
                SettingPage::new(crate::i18n::t("输出高亮", "Highlight")).group(self.highlight_group(window, cx))
            }
            SettingsPageId::Connections => {
                // The paste-a-list importer lives here, not on its own page:
                // it imports connections, and it used to sit under a "粘贴"
                // heading where nobody looking to add connections would find
                // it — and nobody looking for paste *settings* wanted it.
                SettingPage::new(crate::i18n::t("连接", "Connections"))
                    .group(self.connections_group(window, cx))
                    .group(self.paste_group(window, cx))
            }
            SettingsPageId::Files => {
                SettingPage::new(crate::i18n::t("文件", "Files")).group(self.download_group(window, cx))
            }
            SettingsPageId::Sync => SettingPage::new(crate::i18n::t("同步", "Sync")).group(self.webdav_group(window, cx)),
            SettingsPageId::McpPermissions => {
                SettingPage::new(crate::i18n::t("MCP 与权限", "MCP & permissions"))
                    .groups(self.mcp_permissions_groups(window, cx))
            }
        };

        // The toolkit's own sidebar is collapsed to zero: it would only repeat
        // the single selected entry next to ours, and its group menu is a
        // scroll jump.
        h_flex()
            .size_full()
            .min_w_0()
            .child(sidebar)
            .child(
                div()
                    // Every leaf is rendered as toolkit page 0. Separate its
                    // keyed field state so an input cannot retain another
                    // page's cached setter at the same group/row position.
                    .id(SharedString::from(format!("settings-content-{:?}", self.selected)))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(
                        Settings::new("settings")
                            .sidebar_width(px(0.))
                            .sidebar_size_range(px(0.)..px(0.))
                            .page(page)
                            .into_any_element(),
                    ),
            )
            .into_any_element()
    }
}

impl SettingsView {
    /// The 界面 page: theme, language, and the window chrome.
    fn appearance_group(&mut self, window: &mut Window, cx: &mut Context<Self>) -> SettingGroup {
        let store = self.store.clone();

        // ---- Appearance ---------------------------------------------------
        //
        // The window theme. One dropdown rather than a palette per surface, because the
        // terminal is dark either way: it is the window around it that follows this.
        let store_for_theme = store.clone();
        let theme = {
            let current = SharedString::from(store.borrow().theme_pref().to_string());
            let view = cx.entity().downgrade();
            SettingField::dropdown(
                vec![
                    (
                        SharedString::from("system"),
                        SharedString::from(crate::i18n::t("跟随系统", "System")),
                    ),
                    (
                        SharedString::from("dark"),
                        SharedString::from(crate::i18n::t("深色", "Dark")),
                    ),
                    (
                        SharedString::from("light"),
                        SharedString::from(crate::i18n::t("浅色", "Light")),
                    ),
                ],
                move |_| current.clone(),
                move |value, cx| {
                    persist(
                        &store_for_theme,
                        |s| s.set_theme_pref(value.to_string()),
                        "the theme",
                    );
                    if let Some(view) = view.upgrade() {
                        let _ = view.update(cx, |view, cx| {
                            view.pending = Some(SettingsAction::ThemeChanged(value.to_string()));
                            cx.notify();
                        });
                    }
                },
            )
        };
        let store_for_panel = store.clone();
        let panel_font = {
            let current = store.borrow().panel_font();
            SettingField::number_input(
                Default::default(),
                move |_| f64::from(current),
                move |value, _| {
                    persist(
                        &store_for_panel,
                        |s| s.set_panel_font(value as u32),
                        "the panel font size",
                    )
                },
            )
        };

        let store_for_lang = store.clone();
        let language = {
            let current = SharedString::from(store.borrow().language().to_string());
            let view = cx.entity().downgrade();
            SettingField::dropdown(
                vec![
                    (SharedString::from("zh"), SharedString::from("中文")),
                    (SharedString::from("en"), SharedString::from("English")),
                ],
                move |_| current.clone(),
                move |value, cx| {
                    // The flag every `crate::i18n::t` call reads, applied here rather than
                    // routed through an action: it is process-global, so there is nothing for
                    // the shell to decide, and applying it at the click is what makes this
                    // page come back in the new language on the very next frame.
                    crate::i18n::set_language(&value);
                    // The toolkit's own strings come from a different catalog with a different
                    // tag, and that one is process-global too. Widgets built before this still
                    // hold the placeholders they were constructed with; the rest re-read.
                    gpui_kit::component::set_locale(if value.as_ref() == "en" {
                        "en"
                    } else {
                        "zh-CN"
                    });
                    persist(
                        &store_for_lang,
                        |s| s.set_language(value.to_string()),
                        "the language",
                    );
                    // And the window: this view repaints itself, but the session list, the tab
                    // strip and every panel are separate entities that would keep the old
                    // language until something else happened to redraw them.
                    if let Some(view) = view.upgrade() {
                        let _ = view.update(cx, |view, cx| {
                            view.pending = Some(SettingsAction::LanguageChanged);
                            cx.notify();
                        });
                    }
                },
            )
        };
        let appearance = SettingGroup::new()
            .title(crate::i18n::t("外观", "Appearance"))
            .item(
                SettingItem::new(crate::i18n::t("主题", "Theme"), theme).description(
                    crate::i18n::t(
                        "窗口配色。终端始终为暗色。",
                        "The window colours. The terminal stays dark.",
                    ),
                ),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("面板字号 (%)", "Panel font size (%)"),
                    panel_font,
                )
                .description(crate::i18n::t(
                    "侧栏与其他面板的字号,百分比(80–160)。",
                    "The sidebar and other panels' font size, as a percentage.",
                )),
            )
            .item(
                SettingItem::new(crate::i18n::t("语言", "Language"), language)
                    .description(crate::i18n::t("界面语言。", "The interface language.")),
            );

        appearance
    }

    /// The MCP server's own switches.
    fn mcp_permissions_groups(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<SettingGroup> {
        let store = self.store.clone();

        // ---- MCP server ---------------------------------------------------
        let store_for_mcp = store.clone();
        let mcp_enabled = {
            let current = store.borrow().mcp_enabled();
            SettingField::switch(
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_mcp,
                        |s| s.set_mcp_enabled(value),
                        "the MCP server switch",
                    )
                },
            )
        };

        // The four switches, reworded for both callers and with the MCP server's own
        // on-switch separated out. See this module's doc for why.
        let store_for_saved = store.clone();
        let use_saved = {
            let current = store.borrow().mcp_use_saved_credentials();
            SettingField::switch(
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_saved,
                        |s| s.set_mcp_use_saved_credentials(value),
                        "the saved-credentials switch",
                    )
                },
            )
        };

        let store_for_commands = store.clone();
        let allow_commands = {
            let current = store.borrow().mcp_allow_commands();
            SettingField::switch(
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_commands,
                        |s| s.set_mcp_allow_commands(value),
                        "the commands switch",
                    )
                },
            )
        };

        let store_for_transfers = store.clone();
        let allow_transfers = {
            let current = store.borrow().mcp_allow_file_transfers();
            SettingField::switch(
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_transfers,
                        |s| s.set_mcp_allow_file_transfers(value),
                        "the file-transfers switch",
                    )
                },
            )
        };

        let unattended = SettingGroup::new()
            .title(crate::i18n::t("无人值守访问", "Unattended access"))
            .description(crate::i18n::t(
                "以下开关管着 MCP 客户端:它是在无人盯着键盘时替你做事的调用方。全部默认关闭。",
                "These switches govern MCP clients: callers that act without someone \
                 at the keyboard. All default to off.",
            ))
            .item(
                SettingItem::new(
                    crate::i18n::t("允许使用已保存的凭据", "Allow use of saved credentials"),
                    use_saved,
                )
                .description(crate::i18n::t(
                    "可以用已保存的密码和私钥完成认证并查看会话列表,但永远不能读取其内容。",
                    "May authenticate with saved passwords and private keys and list \
                     sessions, but can never read them.",
                )),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("允许执行任意 SSH 命令", "Allow arbitrary SSH commands"),
                    allow_commands,
                )
                .description(crate::i18n::t(
                    "允许对已保存的 SSH 会话执行命令。",
                    "Permits running commands on saved SSH sessions.",
                )),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("允许文件传输", "Allow file transfers"),
                    allow_transfers,
                )
                .description(crate::i18n::t(
                    "允许上传/下载文件,以及列出、读取远程文件。",
                    "Permits uploading and downloading files, and listing or reading \
                     remote files.",
                )),
            );

        // The MCP server's own on-switch, deliberately *not* in the group above: a
        // plugin does not care whether `xenterm mcp serve` may start, and filing it
        // with the gates is what made the page read as being about MCP only.
        let mcp_server = SettingGroup::new()
            .title(crate::i18n::t("MCP 服务", "MCP server"))
            .description(crate::i18n::t(
                "启动命令:xenterm mcp serve   传输方式:stdio(仅本机进程)",
                "Command: xenterm mcp serve   Transport: stdio (local process only)",
            ))
            .item(
                SettingItem::new(
                    crate::i18n::t("启用 MCP 服务", "Enable MCP server"),
                    mcp_enabled,
                )
                .description(crate::i18n::t(
                    "允许本机 MCP 客户端通过 stdio 启动服务。插件不受此开关影响。",
                    "Lets local MCP clients start the server over stdio. Plugins are not \
                         affected by this switch.",
                )),
            );

        // The risk-approval gate. A caller nobody watches can already ask for
        // anything it has permission for; these fields decide which of those
        // asks a human at the main window gets to veto first.
        let store_for_approval = store.clone();
        let approval_enabled = {
            let current = store.borrow().mcp_approval_enabled();
            SettingField::switch(
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_approval,
                        |s| s.set_mcp_approval_enabled(value),
                        "the risky-command approval switch",
                    )
                },
            )
        };
        let store_for_timeout = store.clone();
        let approval_timeout = {
            let current = store.borrow().mcp_approval_timeout_secs() as f64;
            SettingField::number_input(
                Default::default(),
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_timeout,
                        |s| s.set_mcp_approval_timeout_secs(value as u64),
                        "the approval timeout",
                    )
                },
            )
        };
        // The pattern and directory lists are single-line inputs holding
        // comma-separated entries: a settings field framework built around
        // rows has no place for a list editor, and commas are what the
        // defaults read like anyway. Parsed on every read; stored verbatim.
        let store_for_patterns = store.clone();
        let risky_patterns = {
            let current = store.borrow().mcp_risky_patterns().join(", ");
            SettingField::input(
                move |_| SharedString::from(current.clone()),
                move |value, _| {
                    let patterns: Vec<String> = value
                        .split(|c| c == ',' || c == '，' || c == '\n')
                        .map(|p| p.trim().to_string())
                        .filter(|p| !p.is_empty())
                        .collect();
                    persist(
                        &store_for_patterns,
                        |s| s.set_mcp_risky_patterns(patterns),
                        "the risky command patterns",
                    )
                },
            )
        };
        let store_for_dirs = store.clone();
        let risky_dirs = {
            let current = store.borrow().mcp_risky_dirs().join(", ");
            SettingField::input(
                move |_| SharedString::from(current.clone()),
                move |value, _| {
                    let dirs: Vec<String> = value
                        .split(|c| c == ',' || c == '，' || c == '\n')
                        .map(|p| p.trim().to_string())
                        .filter(|p| !p.is_empty())
                        .collect();
                    persist(
                        &store_for_dirs,
                        |s| s.set_mcp_risky_dirs(dirs),
                        "the high-risk directories",
                    )
                },
            )
        };
        let store_for_retention = store.clone();
        let audit_retention = {
            let current =
                f64::from(u32::try_from(store.borrow().mcp_audit_retention_days()).unwrap_or(30));
            SettingField::number_input(
                Default::default(),
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_retention,
                        |s| s.set_mcp_audit_retention_days(value as u64),
                        "the audit retention",
                    )
                },
            )
        };
        let for_audit_folder = cx.entity();
        let audit_folder_field = SettingField::element(
            move |_: &gpui_kit::component::setting::RenderOptions,
                  _: &mut Window,
                  _: &mut gpui_kit::App| {
                let settings = for_audit_folder.clone();
                Button::new("settings-open-audit-folder")
                    .icon(Icon::new(IconName::FolderOpen))
                    .label(crate::i18n::t("查看审批记录", "View audit records"))
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        let _ = settings.update(cx, |view, cx| {
                            view.pending = Some(SettingsAction::OpenAuditFolder);
                            cx.notify();
                        });
                    })
                    .into_any_element()
            },
        );
        let approval_group = SettingGroup::new()
            .title(crate::i18n::t("风险命令审批", "Risky command approval"))
            .description(crate::i18n::t(
                "MCP 执行命中下列模式或目录的命令时,先在主窗口弹出人工审批;无人确认或超时一律拒绝。",
                "When an MCP command hits a pattern or directory below, the main window asks \
                 a human first. No answer or a timeout means no.",
            ))
            .item(
                SettingItem::new(
                    crate::i18n::t("启用风险审批", "Require approval"),
                    approval_enabled,
                )
                .description(crate::i18n::t(
                    "对下面命中的命令,先等人工批准再执行;关闭后所有命令直接执行。",
                    "Hold matching commands for approval; off runs everything at once.",
                )),
            )
            .item(
                SettingItem::new(crate::i18n::t("审批超时(秒)", "Approval timeout (s)"), approval_timeout)
                    .description(crate::i18n::t(
                        "等待人工确认的秒数(10–600),超时视为拒绝。",
                        "Seconds to wait for a human (10-600); a timeout refuses.",
                    )),
            )
            .item(
                SettingItem::new(crate::i18n::t("高危命令模式", "Risky patterns"), risky_patterns)
                    .description(crate::i18n::t(
                        "逗号分隔,大小写不敏感的子串匹配,如 rm -rf, mkfs, shutdown。",
                        "Comma-separated, case-insensitive substrings: rm -rf, mkfs, shutdown.",
                    )),
            )
            .item(
                SettingItem::new(crate::i18n::t("高风险目录", "High-risk dirs"), risky_dirs)
                    .description(crate::i18n::t(
                        "逗号分隔的路径前缀,命令中出现即触发,如 /etc, C:\\Windows。",
                        "Comma-separated path prefixes: /etc, C:\\Windows.",
                    )),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("审批记录保留(天)", "Audit retention (days)"),
                    audit_retention,
                )
                .description(crate::i18n::t(
                    "每次审批(时间、命令、结果、人工或自动)按天记入审计,到期整日删除。",
                    "Every approval — time, command, outcome, human or automatic — is                      journaled by day and whole days expire.",
                )),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("审批记录", "Audit records"),
                    audit_folder_field,
                )
                .description(crate::i18n::t(
                    "在独立窗口中浏览每次审批的时间、命令、结果与方式。",
                    "Browse every approval's time, command, outcome and how it was                      decided, in a dedicated window.",
                )),
            );

        vec![mcp_server, unattended, approval_group]
    }

    /// The 字体 and 光标 pages' groups, built together: the cursor's fields read
    /// the same appearance values the font fields write.
    /// The 光标 page's group: the cursor's fields ride the font builder (they
    /// read the same appearance values), so this is the combined builder's
    /// second group. Two `SettingGroup`s for the price of one build.
    fn cursor_group_only(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<SettingGroup> {
        self.font_and_cursor_groups(window, cx)
            .into_iter()
            .skip(1)
            .collect()
    }

    fn font_and_cursor_groups(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<SettingGroup> {
        let store = self.store.clone();

        // ---- Font page -----------------------------------------------------
        //
        // The terminal's own appearance. Every control here is read by the terminal: the
        // shell hands the settings to each tab and pushes a change to the ones already
        // open, so a size typed here lands on the grid rather than in a file.
        let store_for_family = store.clone();
        let font_family = {
            let current = {
                let configured = store.borrow().font_family().to_string();
                if configured.is_empty() {
                    SharedString::from(crate::core::fonts::BUILT_IN_MONO)
                } else {
                    SharedString::from(configured)
                }
            };
            // The embedded families lead, then whatever the user has imported
            // into the data directory's fonts/ folder, then this machine's
            // installed faces. Scrollable rather than a plain dropdown: a
            // machine with fonts from several language packs has dozens, and a
            // list that runs off the dialog is a list whose end cannot be
            // reached.
            let options: Vec<(SharedString, SharedString)> =
                crate::core::fonts::available_families()
                    .into_iter()
                    .map(|name| {
                        let value = SharedString::from(name.clone());
                        (value, SharedString::from(name))
                    })
                    .collect();
            SettingField::scrollable_dropdown(
                options,
                move |_| current.clone(),
                move |value, _| {
                    persist(
                        &store_for_family,
                        |s| {
                            // An empty family is what this shell writes for "the
                            // default font" — it keeps an older config's meaning
                            // stable — so picking the bundled default stores
                            // empty; every other family stores its name.
                            let chosen = value.to_string();
                            s.set_font_family(if chosen == crate::core::fonts::BUILT_IN_MONO {
                                String::new()
                            } else {
                                chosen
                            });
                        },
                        "the terminal font family",
                    )
                },
            )
        };
        // The imports the picker above draws from: a click opens the file
        // dialog in the shell, which copies the picks into the data directory's
        // fonts/ folder and registers them for the rest of this launch; the
        // other opens that folder in the file manager, where deleting a file is
        // how an import is removed.
        let for_font_import = cx.entity();
        let font_import_field = SettingField::element(
            move |_: &gpui_kit::component::setting::RenderOptions,
                  _: &mut Window,
                  _: &mut gpui_kit::App| {
                let settings = for_font_import.clone();
                Button::new("settings-import-font")
                    .icon(Icon::new(IconName::FilePlus))
                    .label(crate::i18n::t("导入字体", "Import fonts"))
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        let _ = settings.update(cx, |view, cx| {
                            view.pending = Some(SettingsAction::ImportFont);
                            cx.notify();
                        });
                    })
                    .into_any_element()
            },
        );
        let for_font_folder = cx.entity();
        let font_folder_field = SettingField::element(
            move |_: &gpui_kit::component::setting::RenderOptions,
                  _: &mut Window,
                  _: &mut gpui_kit::App| {
                let settings = for_font_folder.clone();
                Button::new("settings-open-font-folder")
                    .icon(Icon::new(IconName::FolderOpen))
                    .label(crate::i18n::t("打开字体文件夹", "Open fonts folder"))
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        let _ = settings.update(cx, |view, cx| {
                            view.pending = Some(SettingsAction::OpenFontFolder);
                            cx.notify();
                        });
                    })
                    .into_any_element()
            },
        );

        let store_for_font = store.clone();
        let font_size = {
            let current = store.borrow().font_size();
            // The widget works in `f64`; the config stores whole pixels. The `as u32` on
            // the way back is a truncation of a value that has no fractional part to
            // lose — a terminal font size is a whole number of pixels.
            SettingField::number_input(
                Default::default(),
                move |_| f64::from(current),
                move |value, _| {
                    persist(
                        &store_for_font,
                        |s| s.set_font_size(value as u32),
                        "the terminal font size",
                    )
                },
            )
        };

        let store_for_bold = store.clone();
        let font_bold = {
            let current = store.borrow().terminal_bold();
            SettingField::switch(
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_bold,
                        |s| s.set_terminal_bold(value),
                        "the bold terminal text setting",
                    )
                },
            )
        };

        let store_for_padding = store.clone();
        let terminal_padding = {
            let current = store.borrow().terminal_padding();
            SettingField::switch(
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_padding,
                        |s| s.set_terminal_padding(value),
                        "the terminal padding setting",
                    )
                },
            )
        };

        let store_for_spacing = store.clone();
        let line_spacing = {
            let current = f64::from(store.borrow().terminal_line_spacing());
            SettingField::number_input(
                Default::default(),
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_spacing,
                        |s| s.set_terminal_line_spacing(value as f32),
                        "the terminal line spacing",
                    )
                },
            )
        };

        let store_for_cursor = store.clone();
        let cursor_style = {
            let current = SharedString::from(store.borrow().terminal_cursor_style().to_string());
            SettingField::dropdown(
                vec![
                    (
                        SharedString::from("block"),
                        SharedString::from(crate::i18n::t("方块", "Block")),
                    ),
                    (
                        SharedString::from("bar"),
                        SharedString::from(crate::i18n::t("竖线", "Bar")),
                    ),
                    (
                        SharedString::from("underline"),
                        SharedString::from(crate::i18n::t("下划线", "Underline")),
                    ),
                ],
                move |_| current.clone(),
                move |value, _| {
                    persist(
                        &store_for_cursor,
                        |s| s.set_terminal_cursor_style(value.to_string()),
                        "the terminal cursor style",
                    )
                },
            )
        };

        // The store refuses partial hex colours. Keep that safety boundary,
        // but retain incomplete text until the user commits the field.
        let cursor_color = self.text_field(TextSetting::CursorColor, window, cx);

        let font_group = SettingGroup::new()
            .title(crate::i18n::t("字体", "Font"))
            .description(crate::i18n::t(
                "终端只使用等宽字体,默认为内置的 Meatshell Mono。",
                "The terminal uses monospace fonts only; the bundled Meatshell Mono is the default.",
            ))
            .item(
                SettingItem::new(crate::i18n::t("字体", "Family"), font_family).description(
                    crate::i18n::t(
                        "终端使用的字体,内置与导入的都在列表里。",
                        "The face the terminal draws with; bundled and imported both appear.",
                    ),
                ),
            )
            .item(
                SettingItem::new(crate::i18n::t("导入字体", "Import fonts"), font_import_field)
                    .description(crate::i18n::t(
                        "从文件导入 ttf/otf 字体,导入后即可在字体列表中选用。",
                        "Import ttf/otf files into the picker; they stay across launches.",
                    )),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("字体文件夹", "Fonts folder"),
                    font_folder_field,
                )
                .description(crate::i18n::t(
                    "打开导入字体所在的文件夹,删除文件即移除该字体。",
                    "Open the import folder; deleting a file removes that font.",
                )),
            )
            .item(
                SettingItem::new(crate::i18n::t("字号", "Size"), font_size).description(
                    crate::i18n::t(
                        "终端网格的字号,单位像素(8–32)。",
                        "The terminal grid's font size in pixels (8-32).",
                    ),
                ),
            )
            .item(
                SettingItem::new(crate::i18n::t("粗体", "Bold text"), font_bold).description(
                    crate::i18n::t(
                        "关闭后,程序标记为粗体的文字也按常规字重绘制。",
                        "With this off, text a program marks bold is drawn at the regular \
                         weight.",
                    ),
                ),
            )
            .item(
                SettingItem::new(crate::i18n::t("内边距", "Grid inset"), terminal_padding)
                    .description(crate::i18n::t(
                        "让终端输出与面板边缘留出几像素,而不是顶格贴边。",
                        "Inset the terminal output a few pixels from the pane's edge                          instead of drawing it flush.",
                    )),
            )
            .item(
                SettingItem::new(crate::i18n::t("行距", "Line spacing"), line_spacing).description(
                    crate::i18n::t(
                        "行高的倍数,1.0 为字体自身行高(0.8–1.5)。",
                        "A multiplier on the line box, 1.0 being the font's own (0.8-1.5).",
                    ),
                ),
            );

        let cursor_group = SettingGroup::new()
            .title(crate::i18n::t("光标", "Cursor"))
            .item(
                SettingItem::new(crate::i18n::t("形状", "Shape"), cursor_style).description(
                    crate::i18n::t("插入光标的形状。", "The insertion cursor's shape."),
                ),
            )
            .item(
                SettingItem::new(crate::i18n::t("颜色", "Colour"), cursor_color).description(
                    crate::i18n::t(
                        "十六进制颜色,如 #2D2D2F。留空为默认的浅白色。",
                        "A hex colour such as #2D2D2F. Empty means the default light grey.",
                    ),
                ),
            );
        vec![font_group, cursor_group]
    }

    /// The 文件 page: where downloads land and how big the dock is.
    fn download_group(&mut self, window: &mut Window, cx: &mut Context<Self>) -> SettingGroup {
        let store = self.store.clone();

        // ---- Download page -------------------------------------------------
        //
        // Both controls are read by the transfer flow: a download goes to the configured
        // directory unless it is told to ask, and the directory the user picks becomes the
        // one the transfer panel's Open folder goes to.
        let store_for_dir = store.clone();
        let download_dir = {
            let current = SharedString::from(store.borrow().download_dir().to_string());
            SettingField::input(
                move |_| current.clone(),
                move |value, _| {
                    persist(
                        &store_for_dir,
                        |s| s.set_download_dir(value.to_string()),
                        "the download directory",
                    )
                },
            )
        };

        let store_for_ask = store.clone();
        let download_ask = {
            let current = store.borrow().download_always_ask();
            SettingField::switch(
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_ask,
                        |s| s.set_download_always_ask(value),
                        "the download prompt setting",
                    )
                },
            )
        };

        // Read per frame by the shell, so a change is visible without
        // reopening anything; the dock's top edge is draggable too, and both
        // doors write the same setting.
        let store_for_panel_height = store.clone();
        let panel_height = {
            let current = f64::from(store.borrow().quick_panel_height());
            SettingField::number_input(
                Default::default(),
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_panel_height,
                        |s| s.set_quick_panel_height(value as f32),
                        "the panel strip's height",
                    )
                },
            )
        };
        let download_group = SettingGroup::new()
            .title(crate::i18n::t("下载", "Downloads"))
            .item(
                SettingItem::new(
                    crate::i18n::t("下载目录", "Download directory"),
                    download_dir,
                )
                .description(crate::i18n::t(
                    "下载文件的保存位置。留空则每次询问。",
                    "Where downloads are saved. Empty asks every time.",
                )),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("每次都询问保存位置", "Always ask where to save"),
                    download_ask,
                )
                .description(crate::i18n::t(
                    "开启后忽略上面的目录,每次下载都打开选择框。",
                    "With this on the directory above is ignored and every download opens a \
                     picker.",
                )),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("启动时隐藏文件面板", "Hide file panel on startup"),
                    {
                        let store = store.clone();
                        let current = store.borrow().collapse_sftp_default();
                        SettingField::switch(
                            move |_| current,
                            move |value, _| {
                                persist(
                                    &store,
                                    |s| s.set_collapse_sftp_default(value),
                                    "the file panel startup visibility",
                                )
                            },
                        )
                    },
                )
                .description(crate::i18n::t(
                    "新窗口默认隐藏文件面板；可从终端标签栏的文件夹按钮重新显示。",
                    "New windows start with the file panel hidden. Reopen it with the folder button in the terminal tab bar.",
                )),
            )

            .item(
                SettingItem::new(
                    crate::i18n::t("面板区域高度", "Panel strip height"),
                    panel_height,
                )
                .description(crate::i18n::t(
                    "终端下方文件面板的高度,单位像素(120–600);也可直接拖拽面板的上边缘调整。",
                    "The height of the file panel under the terminal, in pixels (120-600);                      dragging the panel's top edge works too.",
                )),
            );
        download_group
    }

    /// The 输出高亮 page: the preset and the user's rules.
    fn highlight_group(&mut self, window: &mut Window, cx: &mut Context<Self>) -> SettingGroup {
        let store = self.store.clone();

        // ---- Output highlighting -------------------------------------------
        //
        // What the terminal marks up as it draws. Both controls are read by every tab:
        // the shell hands the settings to the buffers and pushes a change to the ones
        // already open, so switching the preset recolours output that is already on
        // screen rather than only the next command's.
        let store_for_highlight = store.clone();
        let highlight_enabled = {
            let current = store.borrow().output_highlight_enabled();
            SettingField::switch(
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_highlight,
                        |s| s.set_output_highlight_enabled(value),
                        "the output highlighting switch",
                    )
                },
            )
        };

        let store_for_preset = store.clone();
        let highlight_preset = {
            let current = SharedString::from(store.borrow().output_highlight_preset().to_string());
            SettingField::dropdown(
                vec![
                    (
                        SharedString::from("log"),
                        SharedString::from(crate::i18n::t("日志级别", "Log levels")),
                    ),
                    (
                        SharedString::from("devops"),
                        SharedString::from(crate::i18n::t("运维输出", "DevOps output")),
                    ),
                ],
                move |_| current.clone(),
                move |value, _| {
                    persist(
                        &store_for_preset,
                        |s| s.set_output_highlight_preset(value.to_string()),
                        "the output highlighting preset",
                    )
                },
            )
        };

        // The view's own handle, needed by the handlers below: a settings row's control
        // receives an `App` rather than this view's context, so the row asks through the
        // entity — for the add button so the shell can open the editor, and for the
        // delete button so the page redraws without the row.
        let this = cx.entity();
        let settings_for_rules = this.clone();

        let mut highlight_group = SettingGroup::new()
            .title(crate::i18n::t("输出高亮", "Output highlighting"))
            .description(crate::i18n::t(
                "按预设或自定义规则为终端输出着色,只影响显示。",
                "Colours terminal output by preset or by your own rules. Display only.",
            ))
            .item(
                SettingItem::new(
                    crate::i18n::t("启用输出高亮", "Enable output highlighting"),
                    highlight_enabled,
                )
                .description(crate::i18n::t(
                    "关闭后终端按程序给出的颜色显示,不做任何标记。",
                    "Off draws exactly the colours the program asked for.",
                )),
            )
            .item(
                SettingItem::new(crate::i18n::t("预设", "Preset"), highlight_preset).description(
                    crate::i18n::t(
                        "内置的识别规则,与下面的自定义规则同时生效。",
                        "The built-in rules. Your own rules below apply as well.",
                    ),
                ),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("自定义规则", "Your own rules"),
                    // A button in a settings row, because a rule is a form of four fields
                    // and a colour: this page lists rules and switches them, and writing
                    // one happens where there is room to say why a pattern was refused.
                    SettingField::element(
                        move |_: &gpui_kit::component::setting::RenderOptions,
                              _: &mut Window,
                              _: &mut gpui_kit::App| {
                            let settings = settings_for_rules.clone();
                            Button::new("add-highlight-rule")
                                .icon(Icon::new(IconName::Plus))
                                .label(crate::i18n::t("新增规则", "Add a rule"))
                                .small()
                                .outline()
                                .on_click(move |_, _, cx| {
                                    let _ = settings.update(cx, |view, cx| {
                                        view.pending = Some(SettingsAction::NewHighlightRule);
                                        cx.notify();
                                    });
                                })
                                .into_any_element()
                        },
                    ),
                )
                .description(crate::i18n::t(
                    "按关键词或正则给输出着色,最多 128 条。",
                    "Colour output by keyword or regular expression. Up to 128 rules.",
                )),
            );

        // One row per custom rule: what it matches, whether it is on, and the way to
        // delete it. The fields are built here rather than in a helper because each holds
        // its own index, and an index captured by the wrong closure deletes the wrong rule.
        let rules: Vec<_> = store
            .borrow()
            .output_highlight_rules()
            .iter()
            .cloned()
            .collect();
        for (index, rule) in rules.into_iter().enumerate() {
            let store_for_switch = store.clone();
            let store_for_delete = store.clone();
            let settings = this.clone();
            let enabled = rule.enabled;
            let field = SettingField::element(
                move |_: &gpui_kit::component::setting::RenderOptions,
                      _: &mut Window,
                      _: &mut gpui_kit::App| {
                    // Cloned inside the body, because this closure is `Fn`: it runs again
                    // every time the row is drawn, and moving a capture out of it would
                    // make it `FnOnce`.
                    let settings = settings.clone();
                    let store_for_delete = store_for_delete.clone();
                    let store_for_switch = store_for_switch.clone();
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Switch::new(SharedString::from(format!("rule-enabled-{index}")))
                                .checked(enabled)
                                .on_click(move |checked, _, _| {
                                    persist(
                                        &store_for_switch,
                                        |s| s.set_output_highlight_rule_enabled(index, *checked),
                                        "the output highlighting rule",
                                    )
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("rule-delete-{index}")))
                                .icon(Icon::new(IconName::Trash))
                                .ghost()
                                .small()
                                .tooltip(crate::i18n::t("删除规则", "Delete this rule"))
                                .accessibility_label(crate::i18n::t("删除规则", "Delete this rule"))
                                .on_click(move |_, _, cx| {
                                    persist(
                                        &store_for_delete,
                                        |s| s.remove_output_highlight_rule(index),
                                        "the output highlighting rule",
                                    );
                                    // The overlay redraws because the row is gone; the
                                    // shell notices the new rules on its own, since it
                                    // reads the settings every frame.
                                    let _ = settings.update(cx, |_, cx| cx.notify());
                                }),
                        )
                        .into_any_element()
                },
            );
            // The pattern is the row's title, and a description that says how it matches:
            // "does this rule fire on a line or on a word" is the question a list of
            // patterns leaves you asking.
            let mut description = crate::i18n::t("普通文本匹配", "Plain-text match").to_string();
            if rule.regex {
                description = crate::i18n::t("正则表达式", "Regular expression").to_string();
            }
            if rule.case_sensitive {
                description.push_str(&crate::i18n::t(" · 区分大小写", " · case sensitive"));
            }
            if rule.whole_line {
                description.push_str(&crate::i18n::t(" · 整行", " · whole line"));
            }
            if !rule.color.is_empty() {
                description.push_str(&format!(" · {}", rule.color));
            }
            highlight_group = highlight_group.item(
                SettingItem::new(SharedString::from(rule.pattern.clone()), field)
                    .description(description),
            );
        }
        highlight_group
    }

    /// The 输入 page: the paste review and the keyboard habits.
    fn input_group(&mut self, window: &mut Window, cx: &mut Context<Self>) -> SettingGroup {
        let store = self.store.clone();

        // ---- Input ----------------------------------------------------------
        //
        // Both switches are read by every terminal: the shell hands the settings to each
        // tab and pushes a change to the ones already open, so a shortcut turned off stops
        // pasting in a session that is already running.
        let store_for_paste_confirm = store.clone();
        let review_paste = {
            let current = store.borrow().paste_confirm_enabled();
            SettingField::switch(
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_paste_confirm,
                        |s| s.set_paste_confirm_enabled(value),
                        "the multi-line paste setting",
                    )
                },
            )
        };

        let store_for_shortcuts = store.clone();
        let extra_shortcuts = {
            let current = store.borrow().extra_paste_shortcuts_enabled();
            SettingField::switch(
                move |_| current,
                move |value, _| {
                    persist(
                        &store_for_shortcuts,
                        |s| s.set_extra_paste_shortcuts_enabled(value),
                        "the extra paste shortcuts setting",
                    )
                },
            )
        };

        // The panel strip under the terminal: a size the settings own, because the window

        let input_group = SettingGroup::new()
            .title(crate::i18n::t("输入", "Input"))
            .item(
                SettingItem::new(
                    crate::i18n::t("确认多行粘贴", "Confirm multi-line paste"),
                    review_paste,
                )
                .description(crate::i18n::t(
                    "粘贴内容含换行时先显示确认框,避免一次执行多条命令。",
                    "Shows the text for review when a paste contains line breaks, which would \
                     otherwise run several commands at once.",
                )),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("额外粘贴快捷键", "Additional paste shortcuts"),
                    extra_shortcuts,
                )
                .description(crate::i18n::t(
                    "启用 Ctrl+Alt+V、Shift+Insert 与中键粘贴。",
                    "Enables Ctrl+Alt+V, Shift+Insert and middle-click paste.",
                )),
            );

        input_group
    }

    /// The 连接 page: import and export, and the list's maintenance buttons.
    fn connections_group(&mut self, window: &mut Window, cx: &mut Context<Self>) -> SettingGroup {
        let store = self.store.clone();
        let this = cx.entity();
        // ---- Connections page ----------------------------------------------
        //
        // Importing `~/.ssh/config` and exporting the list used to sit in the session
        // list's own menu. Neither is something a user does while working — they are
        // brought out once on a new machine, or once before moving to another — so they
        // belong in Settings, and the list's menu keeps the one entry it uses all the
        // time.
        let for_import = this.clone();
        let import_field = SettingField::element(
            move |_: &gpui_kit::component::setting::RenderOptions,
                  _: &mut Window,
                  _: &mut gpui_kit::App| {
                let settings = for_import.clone();
                Button::new("settings-import-ssh")
                    .icon(Icon::new(IconName::FileDown))
                    .label(crate::i18n::t("导入", "Import"))
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        let _ = settings.update(cx, |view, cx| {
                            view.pending = Some(SettingsAction::ImportSshConfig);
                            cx.notify();
                        });
                    })
                    .into_any_element()
            },
        );
        let for_export = this.clone();
        let export_field = SettingField::element(
            move |_: &gpui_kit::component::setting::RenderOptions,
                  _: &mut Window,
                  _: &mut gpui_kit::App| {
                let settings = for_export.clone();
                Button::new("settings-export-sessions")
                    .icon(Icon::new(IconName::FileUp))
                    .label(crate::i18n::t("导出", "Export"))
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        let _ = settings.update(cx, |view, cx| {
                            view.pending = Some(SettingsAction::ExportSessions);
                            cx.notify();
                        });
                    })
                    .into_any_element()
            },
        );
        let connections_group = SettingGroup::new()
            .title(crate::i18n::t("连接", "Connections"))
            .item(
                SettingItem::new(
                    crate::i18n::t("从 ~/.ssh/config 导入", "Import from ~/.ssh/config"),
                    import_field,
                )
                .description(crate::i18n::t(
                    "把 ssh 配置里的主机加进连接列表,已经存在的会跳过。",
                    "Adds the hosts from the ssh config to the connection list, skipping the \
                     ones already there.",
                )),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("导出连接", "Export connections"),
                    export_field,
                )
                .description(crate::i18n::t(
                    "把全部连接写进一个文件,密码用内置密钥重新加密,可以在另一台机器导入。",
                    "Writes every connection to one file, with passwords re-encrypted under \
                     the built-in key, so another machine can import it.",
                )),
            );

        // A pasted list: the third way in, for the case the other two do not cover — a
        // handful of hosts written in a chat message or a wiki page. The box says what it
        // wants and counts what it understood while you type, because a parser that only
        // reports after the fact makes the user guess which line it disliked.
        connections_group
    }

    /// The 粘贴 page: what a multi-line paste does before it reaches the session.
    fn paste_group(&mut self, window: &mut Window, cx: &mut Context<Self>) -> SettingGroup {
        let store = self.store.clone();
        let paste_group = {
            if self.paste.is_none() {
                self.paste = Some(cx.new(|cx| {
                    TextareaState::new(window, cx).placeholder(crate::i18n::t(
                        "每行一个：主机|端口|用户名|密码|名称",
                        "One per line: host|port|user|password|name",
                    ))
                }));
            }
            let boxed = self.paste.clone();
            let field = SettingField::element({
                let view = cx.entity();
                move |_: &gpui_kit::component::setting::RenderOptions,
                      _: &mut Window,
                      _: &mut gpui_kit::App| {
                    let for_click = view.clone();
                    let for_read = boxed.clone();
                    v_flex()
                        .w_full()
                        .gap_2()
                        .when_some(boxed.clone(), |this, state| {
                            this.child(div().w_full().h(px(120.)).child(Textarea::new(&state)))
                        })
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .items_center()
                                .child(
                                    Button::new("settings-import-pasted")
                                        .debug_selector(|| "settings-import-pasted".to_string())
                                        .icon(Icon::new(IconName::FileDown))
                                        .label(crate::i18n::t("导入这些连接", "Import these"))
                                        .small()
                                        .outline()
                                        .on_click(move |_, _, cx| {
                                            let text = for_read
                                                .as_ref()
                                                .map(|state| state.read(cx).value().to_string())
                                                .unwrap_or_default();
                                            let _ = for_click.update(cx, |view, cx| {
                                                view.pending =
                                                    Some(SettingsAction::ImportPasted(text));
                                                cx.notify();
                                            });
                                        }),
                                )
                                .child(div().flex_1()),
                        )
                        .into_any_element()
                }
            });
            SettingGroup::new()
                .title(crate::i18n::t("批量导入", "Paste a list"))
                .item(
                    SettingItem::new(crate::i18n::t("粘贴连接", "Pasted connections"), field)
                        // Stacked rather than side by side: a paste box in the narrow
                        // control column of a settings row is a box whose own contents
                        // cannot be read back.
                        .layout(Axis::Vertical)
                        .description(crate::i18n::t(
                            "除主机外都可省略；空行、# 注释和表头会被跳过。",
                            "Only the host is required; blank lines, # comments and a header row \
                         are skipped.",
                        )),
                )
        };        paste_group
    }

    /// The 同步 page: the WebDAV mirror.
    fn webdav_group(&mut self, window: &mut Window, cx: &mut Context<Self>) -> SettingGroup {
        let store = self.store.clone();
        // The WebDAV settings: six fields that write the whole configuration at once,
        // because the store's setter takes all of it — half-written is a state nobody
        // meant to save, such as an enabled sync with the previous password.
        // These setters trim/normalize their values. Doing that while typing
        // destroys intermediate URL slashes and path/username edits.
        let webdav_url = self.text_field(TextSetting::WebdavUrl, window, cx);
        let webdav_user = self.text_field(TextSetting::WebdavUsername, window, cx);
        // The password is masked and write-only: the stored value never comes
        // back into the form (audit N-中5). Created lazily on the first render
        // of the sync section, because an input needs a window.
        let webdav_password = {
            if self.webdav_password_input.is_none() {
                let store_for_password = store.clone();
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .placeholder(crate::i18n::t(
                            "留空保留已保存的密码",
                            "Leave empty to keep the saved password",
                        ))
                });
                let subscription = cx.subscribe_in(
                    &input,
                    window,
                    move |_: &mut Self, input, event: &InputEvent, _, cx| {
                        if !matches!(event, InputEvent::Change) {
                            return;
                        }
                        let value = input.read(cx).value().to_string();
                        // Empty keeps the stored password: the form never
                        // shows it, so "cleared the box" cannot mean "erase
                        // the credentials" — replacing them is the only
                        // write this field performs.
                        if value.is_empty() {
                            return;
                        }
                        persist(
                            &store_for_password,
                            |s| {
                                s.set_webdav_settings(
                                    s.webdav_enabled(),
                                    s.webdav_url().to_string(),
                                    s.webdav_username().to_string(),
                                    value,
                                    s.webdav_remote_path().to_string(),
                                    s.webdav_accept_invalid_certs(),
                                )
                            },
                            "the WebDAV password",
                        );
                    },
                );
                self.webdav_password_input = Some(input);
                self._webdav_password_subscription = Some(subscription);
            }
            // Same defect as the session editor's password box: an `element`
            // field gets none of the framework's field styling, so without this
            // the box is only as wide as the input's own intrinsic width.
            let state = self
                .webdav_password_input
                .clone()
                .expect("created just above when missing");
            SettingField::element(
                move |options: &gpui_kit::component::setting::RenderOptions,
                      _: &mut Window,
                      _: &mut gpui_kit::App| {
                    Input::new(&state)
                        .disabled(options.is_disabled())
                        .with_size(options.size())
                        .map(|this| {
                            if options.layout().is_horizontal() {
                                this.w_64()
                            } else {
                                this.w_full()
                            }
                        })
                        .into_any_element()
                },
            )
        };
        let webdav_path = self.text_field(TextSetting::WebdavPath, window, cx);
        let webdav_enabled = {
            let read = store.clone();
            let write = store.clone();
            SettingField::switch(
                move |_| read.borrow().webdav_enabled(),
                move |value, _| {
                    persist(
                        &write,
                        |s| {
                            s.set_webdav_settings(
                                value,
                                s.webdav_url().to_string(),
                                s.webdav_username().to_string(),
                                s.webdav_password().to_string(),
                                s.webdav_remote_path().to_string(),
                                s.webdav_accept_invalid_certs(),
                            )
                        },
                        "the WebDAV switch",
                    )
                },
            )
        };
        let webdav_certs = {
            let read = store.clone();
            let write = store.clone();
            SettingField::switch(
                move |_| read.borrow().webdav_accept_invalid_certs(),
                move |value, _| {
                    persist(
                        &write,
                        |s| {
                            s.set_webdav_settings(
                                s.webdav_enabled(),
                                s.webdav_url().to_string(),
                                s.webdav_username().to_string(),
                                s.webdav_password().to_string(),
                                s.webdav_remote_path().to_string(),
                                value,
                            )
                        },
                        "the WebDAV certificate setting",
                    )
                },
            )
        };
        let webdav_actions = {
            let view = cx.entity();
            let upload = view.clone();
            let download = view;
            SettingField::element(
                move |_: &gpui_kit::component::setting::RenderOptions,
                      _: &mut Window,
                      _: &mut gpui_kit::App| {
                    let up = upload.clone();
                    let down = download.clone();
                    h_flex()
                        .w_full()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("webdav-upload")
                                .icon(Icon::new(IconName::ArrowUpFromLine))
                                .label(crate::i18n::t("上传", "Upload"))
                                .small()
                                .outline()
                                .on_click(move |_, _, cx| {
                                    let _ = up.update(cx, |view, cx| {
                                        view.pending = Some(SettingsAction::WebdavUpload);
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            Button::new("webdav-download")
                                .icon(Icon::new(IconName::ArrowDownToLine))
                                .label(crate::i18n::t("下载", "Download"))
                                .small()
                                .outline()
                                .on_click(move |_, _, cx| {
                                    let _ = down.update(cx, |view, cx| {
                                        view.pending = Some(SettingsAction::WebdavDownload);
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(div().flex_1())
                        .into_any_element()
                },
            )
        };
        let webdav_group = SettingGroup::new()
            .title(crate::i18n::t("WebDAV 同步", "WebDAV sync"))
            .item(
                SettingItem::new(crate::i18n::t("启用同步", "Enable sync"), webdav_enabled)
                    .description(crate::i18n::t(
                        "启用后可以把连接列表上传到 WebDAV，或在另一台机器上下载。",
                        "Lets the connection list be uploaded to WebDAV, or downloaded on \
                         another machine.",
                    )),
            )
            .item(
                SettingItem::new(crate::i18n::t("地址", "Address"), webdav_url).description(
                    crate::i18n::t(
                        "WebDAV 服务器地址，例如 https://dav.example.com/remote.php/dav。",
                        "The WebDAV server's address, for example \
                         https://dav.example.com/remote.php/dav.",
                    ),
                ),
            )
            .item(SettingItem::new(
                crate::i18n::t("用户名", "Username"),
                webdav_user,
            ))
            .item(
                SettingItem::new(crate::i18n::t("密码", "Password"), webdav_password).description(
                    crate::i18n::t(
                        "已保存的密码不回显；输入新值即更换，留空表示保留。",
                        "The saved password is not shown back; typing replaces it, \
                         leaving it empty keeps it.",
                    ),
                ),
            )
            .item(
                SettingItem::new(crate::i18n::t("远程路径", "Remote path"), webdav_path)
                    .description(crate::i18n::t(
                        "留空默认为 xenterm-connections.json，缺失的目录会自动创建。",
                        "Blank means xenterm-connections.json; missing directories are \
                         created.",
                    )),
            )
            .item(
                SettingItem::new(
                    crate::i18n::t("接受无效证书", "Accept invalid certificates"),
                    webdav_certs,
                )
                .description(crate::i18n::t(
                    "自签名证书的服务器需要打开。",
                    "Needed for a server with a self-signed certificate.",
                )),
            )
            .item(
                SettingItem::new(crate::i18n::t("手动同步", "Sync now"), webdav_actions)
                    .description(crate::i18n::t(
                        "上传会覆盖服务器上的文件；下载会合并进现有连接，重复的跳过。",
                        "Uploading overwrites the file on the server; downloading merges into \
                         the connections you have, skipping duplicates.",
                    )),
            );

        // Six pages, each answering one question, because the previous shape answered several
        // on one page and none of them clearly.
        //
        // The sidebar is narrower than the toolkit's default on purpose, and it is a layout
        // decision rather than a taste one. `Settings` lays a row out side by side only while
        // the width available to it is above 480 logical pixels (`STACKED_LAYOUT_MAX_WIDTH` in
        // the toolkit's settings module); below that it stacks title, description and control
        // in a column. The card is capped at about 760 whatever width the dialog asks for, and
        // the sidebar and padding come out of it — at the default sidebar width our rows were
        // 442, which is why every setting in the app was stacked. These page names are two
        // characters each, so 150 is ample and the extra hundred pixels is what carries the
        // rows over the line.
        //
        // The appearance page carried three unrelated things: how the window looks, the four
        // switches that decide what may act without you watching, and the MCP server. The
        // first is decoration and the other two are permissions — the switches' own comments
        // have said for months that they govern "MCP clients and plugins alike", which is the
        // argument for the two of them sharing a page called Automation.
        //
        // The terminal page carried five groups, one of which was the download directory: not
        // a terminal setting at all, and now on a Files page with room for the rest of that
        // subject.
        //
        // The navigation is ours: sections with nested subjects, styled after the
        // toolkit's `sidebar` components (which follow shadcn/ui): a faintly
        // tinted panel, group labels in small muted text, and rows with an icon,
        // a rounded hover accent and a quiet filled selected state — the
        // full-width primary pill the buttons used to grow into read as a
        // terminal prompt, not a selection. Every entry — section or subject —
        // switches to its own page. The toolkit's Settings component renders the
        // content for the selected page only, and its own sidebar is collapsed
        // to zero width: an anchor menu there would be a scroll jump, and this
        // screen does not do those.
        // Copied out of the theme up front: the entry closure needs `&mut cx`
        // for its click listeners, and a live `cx.theme()` borrow would fight it.
        webdav_group
    }
}

/// A group heading in the settings navigation, in the `SidebarGroup` shape:
/// small muted text on its own full-height row, clickable as nothing.
/// One WebDAV setting, with all six values the store's setter takes.
///
/// The store takes the whole configuration at once — half-written is a state nobody meant to
/// save, such as an enabled sync with the previous password — so each field has to name the
/// other five, and the only way to get that wrong is to name one of them twice. These are
/// functions rather than closures so a test can call them and check exactly that.
type WebDavSettings = (bool, String, String, String, String, bool);

fn webdav_with_url(store: &ConfigStore, value: String) -> WebDavSettings {
    (
        store.webdav_enabled(),
        value,
        store.webdav_username().to_string(),
        store.webdav_password().to_string(),
        store.webdav_remote_path().to_string(),
        store.webdav_accept_invalid_certs(),
    )
}

fn webdav_with_username(store: &ConfigStore, value: String) -> WebDavSettings {
    (
        store.webdav_enabled(),
        store.webdav_url().to_string(),
        value,
        store.webdav_password().to_string(),
        store.webdav_remote_path().to_string(),
        store.webdav_accept_invalid_certs(),
    )
}

fn webdav_with_remote_path(store: &ConfigStore, value: String) -> WebDavSettings {
    (
        store.webdav_enabled(),
        store.webdav_url().to_string(),
        store.webdav_username().to_string(),
        store.webdav_password().to_string(),
        value,
        store.webdav_accept_invalid_certs(),
    )
}

#[cfg(test)]
#[path = "settings_input_tests.rs"]
mod input_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every settings page is reachable from the sidebar data: each id appears
    /// exactly once across the foldable sections and the direct entries. The
    /// nav is drawn from these tables, so a page missing here is a page the
    /// sidebar can never offer — reachable only by whoever remembers its id.
    #[test]
    fn every_page_appears_in_the_nav_tables_exactly_once() {
        let all = [
            SettingsPageId::Interface,
            SettingsPageId::TermFont,
            SettingsPageId::TermCursor,
            SettingsPageId::TermInput,
            SettingsPageId::TermHighlight,
            SettingsPageId::Connections,
            SettingsPageId::Files,
            SettingsPageId::Sync,
            SettingsPageId::McpPermissions,
        ];
        let mut listed: Vec<SettingsPageId> = Vec::new();
        for (_, _, pages) in NAV_SECTIONS {
            listed.extend(pages.iter().copied());
        }
        for (page, _, _, _) in NAV_DIRECT {
            listed.push(*page);
        }
        // 界面 is rendered on its own, above the sections.
        listed.push(SettingsPageId::Interface);
        let mut sorted = listed.clone();
        sorted.sort_by_key(|page| format!("{page:?}"));
        let mut expected = all;
        expected.sort_by_key(|page| format!("{page:?}"));
        assert_eq!(listed.len(), all.len(), "a page is listed twice");
        assert_eq!(sorted, expected, "the nav tables and the page enum drifted");
    }

    /// Changing one WebDAV field leaves the other five exactly as they were.
    ///
    /// Every field writes the *whole* configuration, because the store's setter takes all six
    /// — and the failure that invites is the quiet one: a field that names the wrong slot, so
    /// editing the address silently clears the password. Nothing on screen would look wrong;
    /// the sync would simply stop working on the next machine.
    ///
    /// The store is a loaded one, written to in memory and never saved, so the machine's
    /// configuration is untouched.
    #[test]
    fn a_field_change_carries_the_other_five_unchanged() {
        let mut store = ConfigStore::load().expect("the configuration this machine already has");
        store.set_webdav_settings(
            true,
            "https://dav.example.com".into(),
            "me".into(),
            "secret".into(),
            "sync.json".into(),
            true,
        );

        // Each setter takes one new value and must return the other five from the store.
        assert_eq!(
            webdav_with_url(&store, "https://other.example.com".into()),
            (
                true,
                "https://other.example.com".into(),
                "me".into(),
                "secret".into(),
                "sync.json".into(),
                true
            ),
            "a new address keeps the credentials"
        );
        assert_eq!(
            webdav_with_username(&store, "someone".into()),
            (
                true,
                "https://dav.example.com".into(),
                "someone".into(),
                "secret".into(),
                "sync.json".into(),
                true
            )
        );
        assert_eq!(
            webdav_with_remote_path(&store, "other.json".into()),
            (
                true,
                "https://dav.example.com".into(),
                "me".into(),
                "secret".into(),
                "other.json".into(),
                true
            )
        );
    }
}
