//! The UI shell: the tab strip, the panes, the pages and the windows —
//! everything the user sees, drawn with GPUI.
//!
//! This began as the target of a migration away from a second frontend; that
//! frontend is gone now, so a bare `xenterm` launch lands here. The explicit
//! `gpui` argument from the migration window is still accepted and means the
//! same thing as no argument at all.
//!
//! The module keeps the shell's old doctrine where the doctrine was earned —
//! the pane width is *derived*, never measured, and a click is carried out at
//! the top of the next frame, never inside the render that received it — and
//! reports to the shell only what crosses the page's border.

#[path = "impls/auth_dialogs.rs"]
mod auth_dialogs;
#[path = "impls/audit_window.rs"]
mod audit_window;
// The folding chevron every group list shares: one icon rotated, not two swapped.
#[path = "impls/chevron.rs"]
mod chevron;
#[path = "impls/dialogs.rs"]
mod dialogs;
#[path = "impls/event_sink.rs"]
mod event_sink;
#[path = "impls/file_viewer.rs"]
mod file_viewer;
#[path = "impls/group_manager.rs"]
mod group_manager;
#[path = "impls/history.rs"]
mod history;
// The navigation rail: one icon per page, and the page switch on click.
#[path = "impls/nav.rs"]
mod nav;
#[path = "impls/panes.rs"]
mod panes;
// The pages. One variant of `PageId` each, one entity each, mounted one at a
// time — this is what makes the shell a set of diffable surfaces instead of
// one view.
#[path = "impls/pages/mod.rs"]
mod pages;
#[path = "impls/process_window.rs"]
mod process_window;
#[path = "impls/quick_commands.rs"]
mod quick_commands;
#[path = "impls/quick_manager.rs"]
mod quick_manager;
#[path = "impls/rule_editor.rs"]
mod rule_editor;
#[path = "impls/jump_chain_editor.rs"]
mod jump_chain_editor;
#[path = "impls/session_editor.rs"]
mod session_editor;
#[path = "impls/session_list.rs"]
mod session_list;
#[path = "impls/session_state.rs"]
mod session_state;
#[path = "impls/settings.rs"]
mod settings;
#[path = "impls/sftp_panel.rs"]
mod sftp_panel;
#[path = "impls/shell.rs"]
mod shell;
#[path = "impls/sidebar.rs"]
mod sidebar;
#[path = "impls/system_info_window.rs"]
mod system_info_window;
#[path = "impls/terminal.rs"]
mod terminal;
#[path = "impls/transfers.rs"]
mod transfers;
#[path = "impls/tunnels.rs"]
mod tunnels;
#[path = "impls/view.rs"]
mod view;

#[cfg(test)]
mod dialog_parity_tests;

#[cfg(test)]
#[path = "../../tests/app/ui_animations/mod.rs"]
mod ui_animation_probes;

pub(crate) use audit_window::AuditWindowHandle;
pub(crate) use shell::run;
// What a detached window — the process monitor, the system-information window — has to be
// able to do for the shell to open it, focus it and point it at a tab.
pub(crate) use shell::TabFollower;
// The action row every dialog in this shell ends with, because a `Dialog` renders its
// button callbacks and not its buttons; and the subscription that keeps a view mounting
// the dialog layer repainting when the queue behind it changes.
pub(crate) use dialogs::{answer_footer, follow_root};
// What the shell names. The grid, the snapshot it draws and the measured cell size
// are the three things a caller has to hold to put a terminal in a window; the
// painters, the image conversion and the colour bridge stayed inside `terminal`.
pub(crate) use terminal::{terminal_grid, CellMetrics, GridSnapshot};
// And the view itself, which owns the buffer those three are derived from.
pub(crate) use view::{TerminalSettings, TerminalView};
// The session list, which the shell lays out beside the terminal.
pub(crate) use session_editor::{EditorOutcome, SessionEditor};
pub(crate) use session_list::{SessionListAction, SessionListEvent, SessionListView};
// The plugin manager: what is installed, what each may do, and the switches that were
// previously only reachable from `xenterm cli grant`.
// The process monitor, which is a window of its own rather than a panel: a table you
// keep open beside the terminal while you work in it.
pub(crate) use process_window::{window_title as process_window_title, ProcessWindowView};
// The settings view, where the switches that govern MCP clients and plugins alike now
// live together under one honest heading.
pub(crate) use settings::{SettingsAction, SettingsView};
// The remote directory panel, and the actions it asks the shell to perform because the
// shell is what holds the session.
pub(crate) use sftp_panel::{PanelAction, SftpPanelView};
// The resource panel: the active session's host, this machine, and the two graphs.
pub(crate) use sidebar::{SidebarAction, SidebarView};
// The transfer manager, which is a panel rather than a window: it belongs to the window
// whose sessions are transferring.
pub(crate) use transfers::{TransferAction, TransferListView};
// The quick-command dock: the commands a session is expected to need, one click away.
pub(crate) use quick_commands::{DockEdge, QuickAction, QuickCommandsView};
// And the manager behind it, where those commands are written.
pub(crate) use quick_manager::{QuickManagerAction, QuickManagerView};
// The command-history dropdown, which is the other half of "what did I just run".
pub(crate) use history::{HistoryAction, HistoryView};
// The group manager, where the session list's folders are made and named.
pub(crate) use group_manager::{GroupManagerAction, GroupManagerView};
// The output-highlight rule editor, opened from the settings page.
pub(crate) use rule_editor::{RuleEditorAction, RuleEditorView};
// The tunnel panel: the active session's forwards, and a form to start one.
pub(crate) use tunnels::{TunnelAction, TunnelsView};
// The detailed-probe window, a sibling of the process monitor.
pub(crate) use system_info_window::{
    window_title as system_info_window_title, SystemInfoWindowView,
};
// What a connect needs: the shared stores, the routes and the runtime. The shell holds
// one and hands it to every session it starts.
pub(crate) use session_state::{SessionState, WINDOW_ID};

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// The code points that are emoji: the pictographic planes, the dingbats and symbols
    /// that most platforms render in colour, and the variation selector that turns a plain
    /// glyph into one.
    fn has_emoji(text: &str) -> bool {
        text.chars().any(|c| {
            let cp = c as u32;
            (0x1F300..=0x1FAFF).contains(&cp)
                || (0x2600..=0x27BF).contains(&cp)
                || (0x2B00..=0x2BFF).contains(&cp)
                || cp == 0xFE0F
        })
    }

    fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("the source tree is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                rs_files(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                out.push(path);
            }
        }
    }

    /// The migrated interface uses icon-font glyphs and words, never emoji.
    ///
    /// This was checked by hand once, and a check that ran once is a fact about that day
    /// rather than a property of the code. It is a test now, so the next person who reaches
    /// for a rocket ship in a label finds out immediately instead of at review.
    ///
    /// The test's own source is in the tree it scans, and does not trip it: the ranges are
    /// written as `0x1F300` rather than as the characters they name.
    #[test]
    fn the_gpui_interface_uses_no_emoji() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("ui");
        let mut files = Vec::new();
        rs_files(&root, &mut files);
        assert!(!files.is_empty(), "no sources found under {root:?}");

        let mut offenders = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).expect("a source file is readable");
            for (index, line) in text.lines().enumerate() {
                if has_emoji(line) {
                    offenders.push(format!("{}:{}: {}", file.display(), index + 1, line.trim()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "emoji in the migrated interface:\n{}",
            offenders.join("\n")
        );
    }
}
