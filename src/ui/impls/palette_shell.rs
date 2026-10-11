//! The command palette's shell side: the command list, the dialog, and the
// dispatch. The palette's own view lives in `command_palette.rs`; what lives
// here is everything that needs the window — opening it over the work,
// carrying a pick out at the top of the next frame, and running the command
// through the same methods its menu entry and its chord use.

use gpui_kit::component::{
    v_flex,
    // `open_dialog` / `close_dialog` / `has_active_dialog` are trait methods,
    // not inherent ones — same import the shell's own render names.
    WindowExt as _,
};
use gpui_kit::{div, prelude::*, px, Context, Window};

use super::command_palette::CommandId;
use super::pages::PageId;
// `Overlay` and `TerminalAction` are crate-visible names the shell module owns;
// re-exported through it for the sibling modules rather than made pub for the
// whole crate.
use super::pages::TerminalAction;
use super::shell::{Overlay, Shell};

impl Shell {
    pub(crate) fn commands(&self) -> Vec<super::command_palette::Command> {
        use super::command_palette::{Command, CommandId};
        let t = crate::i18n::t;
        vec![
            Command { label: t("快速连接", "Quick connect").into(), id: CommandId::QuickConnect },
            Command { label: t("新建连接", "New connection").into(), id: CommandId::NewSession },
            Command { label: t("导入配置", "Import config").into(), id: CommandId::ImportConfig },
            Command { label: t("连接管理页", "Connections page").into(), id: CommandId::ConnectionsPage },
            Command { label: t("终端页", "Terminal page").into(), id: CommandId::TerminalPage },
            Command { label: t("设置页", "Settings page").into(), id: CommandId::SettingsPage },
            Command { label: t("端口转发", "Port forwarding").into(), id: CommandId::Tunnels },
            Command { label: t("进程监视器", "Process monitor").into(), id: CommandId::Processes },
            Command { label: t("系统信息", "System information").into(), id: CommandId::SystemInfo },
            Command { label: t("审批记录", "Approval audit").into(), id: CommandId::AuditLog },
            Command { label: t("向右分屏", "Split right").into(), id: CommandId::SplitRight },
            Command { label: t("向下分屏", "Split down").into(), id: CommandId::SplitDown },
            Command { label: t("关闭标签", "Close tab").into(), id: CommandId::CloseTab },
            Command { label: t("下一个标签", "Next tab").into(), id: CommandId::NextTab },
            Command { label: t("上一个标签", "Previous tab").into(), id: CommandId::PrevTab },
            Command { label: t("重连当前会话", "Reconnect session").into(), id: CommandId::Reconnect },
            Command { label: t("显示/隐藏文件面板", "Show/hide file panel").into(), id: CommandId::ToggleDock },
            Command { label: t("显示/隐藏资源栏", "Show/hide resource panel").into(), id: CommandId::ToggleSidebar },
            Command { label: t("主题：深色", "Theme: dark").into(), id: CommandId::ThemeDark },
            Command { label: t("主题：浅色", "Theme: light").into(), id: CommandId::ThemeLight },
            Command { label: t("主题：跟随系统", "Theme: follow system").into(), id: CommandId::ThemeSystem },
        ]
    }

    /// Open the command palette: the same light dialog the quick-connect
    /// palette opens as, over whatever is on screen.
    pub(crate) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The action listener already holds Shell's update lease. Snapshot the
        // commands through `self`; the child must not read that same entity.
        let commands = self.commands();
        // `AppContext::new` — the trait is what carries entity construction, so
        // it is named in scope rather than resolved as an inherent method.
        use gpui_kit::AppContext as _;
        let palette = cx.new(|cx| super::command_palette::CommandPalette::new(commands, window, cx));
        self.overlay = Overlay::Commands(palette.clone());
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        let margin_top =
            ((f32::from(window.bounds().size.height) - (440. + 104.)) / 2.0).max(24.0);
        let palette_for_dialog = palette.clone();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            dialog
                .title(crate::i18n::t("命令", "Commands"))
                .width(px(460.))
                .margin_top(px(margin_top))
                .close_button(true)
                .overlay_closable(true)
                .child(
                    v_flex()
                        .w_full()
                        .h(px(440.))
                        .child(palette_for_dialog.clone().into_any_element()),
                )
        });
        palette.update(cx, |view, cx| view.focus(window, cx));
        cx.notify();
    }

    /// Carry the palette's pick out, or take the palette down if it went away
    /// without one (a click outside, or Escape).
    pub(crate) fn drain_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Overlay::Commands(_)) {
            return;
        }
        let Overlay::Commands(palette) = self.overlay.clone() else {
            return;
        };
        let picked = palette.update(cx, |view, _| view.take_pick());
        match picked {
            Some(id) => {
                // The dialog goes down before the command runs, because a
                // command that opens another dialog would otherwise queue
                // behind the palette instead of replacing it.
                window.close_dialog(cx);
                self.overlay = Overlay::None;
                self.run_command(id, window, cx);
            }
            None => {
                // No pick and the dialog is gone: a click outside, or Escape.
                // The overlay follows the dialog, the way the quick-connect
                // palette's drain does.
                if !window.has_active_dialog(cx) {
                    self.overlay = Overlay::None;
                }
            }
        }
        cx.notify();
    }

    /// Run one palette command, through the same methods its menu entry and
    /// its chord use — a command discovered here is the same command
    /// everywhere else.
    fn run_command(
        &mut self,
        id: super::command_palette::CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use super::command_palette::CommandId;
        match id {
            CommandId::QuickConnect => self.open_quick_connect(window, cx),
            CommandId::NewSession => self.open_editor(None, window, cx),
            CommandId::ImportConfig => self.import_connections(cx),
            CommandId::ConnectionsPage => self.open_page(PageId::Sessions, window, cx),
            CommandId::TerminalPage => self.open_page(PageId::Terminal, window, cx),
            CommandId::SettingsPage => self.open_page(PageId::Settings, window, cx),
            CommandId::Tunnels => {
                self.pages
                    .terminal
                    .update(cx, |page, cx| page.request(TerminalAction::OpenTunnels, cx));
            }
            CommandId::Processes => self.open_process_window(cx),
            CommandId::SystemInfo => self.open_system_info_window(cx),
            CommandId::AuditLog => self.open_audit_viewer(cx),
            CommandId::SplitRight => {
                self.pages
                    .terminal
                    .update(cx, |page, cx| page.split_active_tab(false, cx));
            }
            CommandId::SplitDown => {
                self.pages
                    .terminal
                    .update(cx, |page, cx| page.split_active_tab(true, cx));
            }
            CommandId::CloseTab => {
                let terminal_visible = self.pages.active == PageId::Terminal;
                self.pages.terminal.update(cx, |page, cx| {
                    if let Some(id) = page.active_tab_id() {
                        if terminal_visible {
                            page.close_tab_and_focus(&id, window, cx);
                        } else {
                            page.close_tab(&id, cx);
                        }
                    }
                });
            }
            CommandId::NextTab => {
                self.pages
                    .terminal
                    .update(cx, |page, cx| page.cycle_tab(false, cx));
            }
            CommandId::PrevTab => {
                self.pages
                    .terminal
                    .update(cx, |page, cx| page.cycle_tab(true, cx));
            }
            CommandId::Reconnect => self.reconnect_ended_session(cx),
            CommandId::ToggleDock => {
                self.pages.terminal.update(cx, |page, cx| {
                    page.set_sftp_collapsed(!page.sftp_collapsed, cx);
                });
            }
            CommandId::ToggleSidebar => {
                let sidebar = self.pages.terminal.read(cx).sidebar_entity().clone();
                let collapsed = sidebar.read(cx).is_collapsed();
                sidebar.update(cx, |sidebar, cx| sidebar.set_collapsed(!collapsed, cx));
            }
            CommandId::ThemeDark => self.apply_theme_pref("dark", window, cx),
            CommandId::ThemeLight => self.apply_theme_pref("light", window, cx),
            CommandId::ThemeSystem => self.apply_theme_pref("system", window, cx),
        }
    }

    /// Apply a theme from the palette and *persist* it.
    ///
    /// The settings page's dropdown writes the preference and applies the mode;
    /// these commands are the same decision from the other door, and a palette
    /// change that reverted on restart (and left the dropdown showing the old
    /// value) was two answers to one question.
    fn apply_theme_pref(&mut self, pref: &str, window: &mut Window, cx: &mut Context<Self>) {
        {
            let mut store = self.state.store.borrow_mut();
            store.set_theme_pref(pref.to_string());
            let _ = store.save();
        }
        match pref {
            "dark" => gpui_kit::component::Theme::change(
                gpui_kit::component::ThemeMode::Dark,
                Some(window),
                cx,
            ),
            "light" => gpui_kit::component::Theme::change(
                gpui_kit::component::ThemeMode::Light,
                Some(window),
                cx,
            ),
            _ => gpui_kit::component::Theme::sync_system_appearance(Some(window), cx),
        }
        window.refresh();
        cx.notify();
    }
}
