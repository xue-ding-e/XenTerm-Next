//! The SFTP panel: the remote directory, its columns, and what you can do to a row.
//!
//! The second view of the migration, and the first one that reads state a *session*
//! fills rather than state the user typed. `crate::core::SftpListing` already holds the
//! path, the files, the current sort and the selection — it lives in the core rather
//! than in the view — so this module owns no state of its own beyond what the panel
//! itself needs to look alive (a spinner and a status line, both of which come from
//! events).
//!
//! What it deliberately does not do yet: the left-hand directory tree, drag-and-drop
//! upload, and the file viewer/editor. Each is a view of its own and the panel is
//! useful without them; the buttons that would open them are absent rather than
//! present-and-dead, because a button that does nothing is worse than no button.
//!
//! **The original's callbacks, checked one by one against this panel.** Not a guess and not a
//! memory: the original declared the list, and each entry is either here or named below as
//! missing. The check was worth doing once, because it is the only way to answer "is anything
//! absent" without reading two implementations side by side and hoping.
//!
//! Present: navigate, refresh, toggle-select, sort (the original also has `clear-sort`, which
//! its three-way cycle folds in), download, download-selected, upload, delete,
//! delete-selected, new-folder, new-file, rename-request, chmod-request, edit-external (as
//! `OpenTemp`), dock collapse, and the tunnel actions, which live in their own panel here.
//!
//! Missing, and each one is a real difference rather than a naming one:
//!
//! - **the directory tree** (`tree-expand`, `persist-tree-width`), the left-hand folder pane.
//!   The core even has `toggle_tree_node`; nothing in this frontend calls it.
//! - **dock dragging** (`dock-drag-move`, `dock-drag-end`) — moving this panel between the
//!   bottom edge and the right one. Not a grip's worth of work: where the panel lives is
//!   decided by the shell's pane layout, so it is a layout change with a setting behind it,
//!   not a drag handler.
//!
//! **Closed since this list was written**, which is the point of keeping it: the built-in
//! viewer and editor (`edit` / `view`, now a dialog of its own in `file_viewer.rs`), the
//! single-row `copy-to-target` (the row menu's 复制到 entries), `open-external` (用默认程序打开,
//! the same session command as `edit-external` with the watching turned off), and `copy-path`
//! (复制路径, offered for directories too — a folder's path is the one row action a folder
//! wants), and `copy-selected-to-target` (the batch toolbar's 复制到其他会话 dropdown, which
//! carries every ticked row and appears only when something is ticked).
//!
//! Row-menu entries live in the toolkit's popup layer rather than under the row's debug
//! selectors. Event tests open that native menu and activate its entries by keyboard;
//! they do not assume `debug_bounds` can find every popup item. The viewer has its own
//! module and tests for the behavior after a row's action reaches it.

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants, DropdownButton},
        h_flex,
        menu::{ContextMenuExt, PopupMenuItem},
        spinner::Spinner,
        v_flex, ActiveTheme as _, Icon, Sizable as _, Size,
    },
    div,
    prelude::*,
    px,
    uniform_list, AnyElement, Context, Entity, FontWeight, Hsla, IntoElement, ListSizingBehavior,
    Point, Render, SharedString, UniformListScrollHandle, Window,
};

// The full Lucide catalog rather than the component library's curated subset: a file
// panel wants an upload arrow, a download arrow and a bin, and the subset has none of
// the three.
use gpui_kit::assets::IconName;

use crate::core::{SftpColumn, SftpFile, SftpListing};
use crate::session::protocol::RemoteTreeNode;

// The listing's column widths, named once in `tokens.rs` because the headings
// and the rows have to agree for the table to line up.
use super::tokens::{
    SFTP_MTIME_WIDTH as MTIME_WIDTH, SFTP_SIZE_WIDTH as SIZE_WIDTH, SFTP_TICK_WIDTH as TICK_WIDTH,
    SFTP_TREE_WIDTH as TREE_WIDTH,
};

/// How long a listing request may stay unanswered before the spinner gives up.
/// Generous on purpose: a huge directory over a slow link is a real listing, and
/// the only path this covers is the reply that is never coming.
const LOADING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// What the panel wants the shell to do.
///
/// The shell owns the session handles, so the panel reports intent and the shell sends
/// the command. Same reason the session list reports a click rather than connecting:
/// the view that draws is not the view that owns the connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PanelAction {
    /// Go to this directory path.
    Navigate(String),
    /// Re-read the current directory, resyncing the tree (#189).
    Refresh,
    /// Ask the OS for a local file and upload it here.
    Upload,
    /// Download these remote paths into a chosen local directory.
    Download(Vec<String>),
    /// Delete these remote paths.
    Delete(Vec<String>),
    /// Toggle a row's tick box.
    ToggleRow(usize),
    /// Sort by this column, cycling asc → desc → default.
    Sort(SftpColumn),
    /// Open this remote file in whatever the desktop uses for it.
    ///
    /// The file is copied to a temporary directory first and watched while it is edited,
    /// so a save is uploaded back. All of that is the SFTP session's — this is one
    /// command and no logic here, because the panel's part is to say *which* file.
    OpenTemp(String),
    /// Open this remote file with whatever the desktop uses for it, without watching it.
    ///
    /// The original offers both this and OpenTemp: one to look at a file, one to edit it
    /// and have the save come back. Handing someone the watching version when they wanted to
    /// look at a log is a file that gets uploaded by surprise.
    OpenDefault(String),
    /// Put this row's remote path on the clipboard.
    CopyPath(String),
    /// Copy this file into another open session, named by its tab id.
    CopyTo { paths: Vec<String>, target: String },
    /// Expand or collapse a directory in the tree.
    TreeToggle(String),
    /// Dock the panel to an edge: right when true, bottom when false.
    Redock { right: bool },
    /// Hide the dock without closing its session or cancelling transfers.
    Collapse,
    /// Open this remote file in the built-in viewer, read-only.
    View(String),
    /// Open it in the built-in editor, where a save goes back to the server.
    Edit(String),
    /// Make a directory in the current directory. The name is asked for by the shell.
    MkDir,
    /// Make an empty file in the current directory.
    TouchFile,
    /// Rename the ticked entry, keeping it in the same directory.
    Rename(Vec<String>),
    /// Change the mode of the ticked entries, given as octal digits.
    Chmod(Vec<String>),
}

/// The SFTP panel view.
pub(crate) struct SftpPanelView {
    /// The listing as of the last refresh, which is what the header and rows draw.
    listing: SftpListing,
    /// When the current listing request went out, which drives the spinner. A
    /// timestamp rather than a flag because the reply can be lost — the session
    /// can die, or answer with an error the panel never hears about — and a flag
    /// would then spin forever. `LOADING_TIMEOUT` bounds that: past it the panel
    /// assumes nothing is coming and settles.
    loading_since: Option<std::time::Instant>,
    /// Which tab's listing generation this panel's copy reflects. The shell's
    /// drain compares it against the store each frame; the `None` a fresh panel
    /// starts with is what forces the first sync.
    synced: Option<(String, u64)>,
    /// The last status or error text from the session, shown under the toolbar.
    status: Option<String>,
    /// The last action an interaction produced, drained by the shell's caller.
    pending: Option<PanelAction>,
    /// The other open sessions a file can be copied into, as (tab id, label).
    /// Owned by the panel rather than looked up, because the panel cannot see the
    /// shell tabs and the row menu needs one entry per target.
    targets: Vec<(String, String)>,
    /// The directory tree as the session last reported it: one row per node, already
    /// flattened with a depth, which is the shape the original binds too.
    tree: Vec<RemoteTreeNode>,
    /// Where a press on the toolbar landed, while it is still down.
    ///
    /// The original moves this panel between edges by dragging its grip. The gesture lives
    /// here rather than in the shell because the toolbar is this view's, and what it reports
    /// is an action like any other.
    dock_press: Option<(f32, f32)>,
    /// The scroll position of the virtualized file list.
    ///
    /// The rows are a `uniform_list` — only the visible window is mounted, so a
    /// ten-thousand-entry directory costs one screenful of elements — and the handle
    /// is what the panel holds so a new directory can start scrolled to the top.
    list_scroll: UniformListScrollHandle,
}

impl SftpPanelView {
    pub(crate) fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            listing: SftpListing::default(),
            loading_since: None,
            synced: None,
            status: None,
            pending: None,
            targets: Vec::new(),
            tree: Vec::new(),
            dock_press: None,
            list_scroll: UniformListScrollHandle::new(),
        }
    }

    /// Whether a listing is still genuinely in flight.
    ///
    /// The timeout exists for the replies that never come: a session that died
    /// mid-list sends no error the panel can hear, and a spinner that outlives the
    /// session by minutes reads as a hang. Long enough that a real — slow — listing
    /// never gets its spinner pulled out from under it.
    fn is_loading(&self) -> bool {
        self.loading_since
            .map(|since| since.elapsed() < LOADING_TIMEOUT)
            .unwrap_or(false)
    }

    /// Whether a listing request is in flight *and* has outlived its timeout:
    /// the spinner is gone, and this is the moment the panel says why.
    fn loading_timed_out(&self) -> bool {
        self.loading_since
            .map(|since| since.elapsed() >= LOADING_TIMEOUT)
            .unwrap_or(false)
    }

    /// Mark a listing as in flight, without touching the path.
    pub(crate) fn set_loading(&mut self, cx: &mut Context<Self>) {
        self.loading_since = Some(std::time::Instant::now());
        cx.notify();
    }

    /// Record which tab's listing generation this panel's copy is in step with.
    ///
    /// The shell's per-frame drain compares this against the store's current
    /// generation; when they differ, a session event (entries, an error, a tree)
    /// has landed since the last sync and the panel is re-pointed at the store.
    pub(crate) fn mark_synced(&mut self, tab: &str, generation: u64, cx: &mut Context<Self>) {
        let synced = Some((tab.to_string(), generation));
        if self.synced != synced {
            self.synced = synced;
            cx.notify();
        }
    }

    /// The generation this panel is in step with, if it is the named tab's.
    /// Another tab's generation is not an answer: the comparison is only
    /// meaningful for the tab the dock is showing.
    pub(crate) fn synced_generation(&self, tab: &str) -> Option<u64> {
        self.synced
            .as_ref()
            .filter(|(synced_tab, _)| synced_tab == tab)
            .map(|(_, generation)| *generation)
    }

    /// Show a freshly arrived tree.
    ///
    /// Pushed rather than built here: the session owns which directories are expanded, and
    /// every toggle answers with the whole flattened list rather than a delta.
    pub(crate) fn set_tree(
        &mut self,
        tree: Vec<RemoteTreeNode>,
        cx: &mut Context<Self>,
    ) {
        self.tree = tree;
        cx.notify();
    }

    /// Tell the panel which other sessions a file can be copied into.
    pub(crate) fn set_targets(&mut self, targets: Vec<(String, String)>, cx: &mut Context<Self>) {
        if self.targets != targets {
            self.targets = targets;
            cx.notify();
        }
    }

    /// Take the next action the user asked for, if any.
    ///
    /// Polled by the shell rather than pushed, because the shell owns the session
    /// handle and a callback would have to reach it through the entity that owns this
    /// view — the same cycle the session list avoids by reporting an event.
    pub(crate) fn take_action(&mut self) -> Option<PanelAction> {
        self.pending.take()
    }

    /// Show a freshly arrived listing.
    pub(crate) fn set_listing(&mut self, listing: SftpListing, cx: &mut Context<Self>) {
        self.listing = listing;
        self.loading_since = None;
        // A new listing starts scrolled to the top: the virtualized list would
        // otherwise keep the old offset, which can land past the end of a
        // shorter directory and read as an empty panel.
        self.list_scroll
            .0
            .borrow()
            .base_handle
            .set_offset(Point::new(px(0.), px(0.)));
        cx.notify();
    }

    /// The path just changed, before its entries arrive.
    pub(crate) fn set_path(&mut self, path: String, cx: &mut Context<Self>) {
        self.listing.set_path(path);
        self.loading_since = Some(std::time::Instant::now());
        cx.notify();
    }

    /// The left directory tree: one row per node, indented by its depth.
    ///
    /// The original puts this beside the listing and hides it when the panel is narrow; this
    /// panel is wide, which is exactly the case the original keeps it shown for. A node with
    /// children carries a chevron that toggles it, and the session answers with a fresh list.
    fn tree_column(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .tree
            .iter()
            .map(|node| {
                let path = node.path.clone();
                let selector = format!("sftp-tree-{}", node.path);
                h_flex()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .w_full()
                    .pl(px(6.0 + node.depth as f32 * 12.0))
                    .pr_2()
                    .py_1()
                    .gap_1()
                    .items_center()
                    .cursor_pointer()
                    .hover(|this| this.bg(cx.theme().accent.opacity(0.12)))
                    .child(
                        Icon::new(if node.has_children {
                            if node.expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            }
                        } else {
                            IconName::Folder
                        })
                        .size_3(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .child(SharedString::from(node.name.clone())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pending = Some(PanelAction::TreeToggle(path.clone()));
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();

        v_flex()
            .id("sftp-tree")
            .w(px(TREE_WIDTH))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(cx.theme().border)
            .py_1()
            .children(rows)
            .into_any_element()
    }

    /// The panel's current directory.
    pub(crate) fn path(&self) -> &str {
        self.listing.path()
    }

    /// One column's header: a label you can click to sort by it.
    ///
    /// Text, not a button. A table's headings are a table's headings — the whole panel
    /// was wearing a row of outlined buttons across its top, which reads as five controls
    /// where there is one label and three sorts, and it is the first thing the eye lands
    /// on. The arrow appears only on the column that is actually sorting.
    ///
    /// The width is passed rather than measured, because the header has to line up with
    /// the rows and the rows decide their own widths.
    fn column_header(
        &self,
        column: SftpColumn,
        label: &str,
        width: Option<f32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The direction is read from the listing's own sort rather than guessed from the
        // label; `SftpSortDir` is not exported from `crate::core`, so it is read as its
        // wire integer — 1 ascending, -1 descending.
        let sort = self.listing.sort();
        let direction = match (sort.column(), sort.dir().map(|d| d.wire_dir())) {
            (Some(sorted), Some(1)) if sorted == column => Some(IconName::ArrowUp),
            (Some(sorted), Some(-1)) if sorted == column => Some(IconName::ArrowDown),
            _ => None,
        };
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let label: SharedString = SharedString::from(label.to_string());
        let column_for_click = column;
        let column_for_debug = column;
        h_flex()
            // An id is required before an interaction handler: GPUI's hit test needs to
            // know which element owns the click.
            .id(SharedString::from(format!("sftp-col-{label}")))
            // Keyed on the column rather than the label, so a test names a column and not a
            // translated word. An id and a debug selector are two different things.
            .debug_selector(move || format!("sftp-col-{column_for_debug:?}"))
            .when_some(width, |this, width| this.w(px(width)))
            .when(width.is_none(), |this| this.flex_1())
            .min_w_0()
            .gap_1()
            .items_center()
            .text_xs()
            .text_color(muted)
            .cursor_pointer()
            .hover(move |this| this.text_color(foreground))
            .child(div().truncate().child(label))
            .when_some(direction, |this, icon| this.child(Icon::new(icon).size_3()))
            .on_click(cx.listener(move |this, _, _, _| {
                this.pending = Some(PanelAction::Sort(column_for_click));
            }))
            .into_any_element()
    }

}

impl Render for SftpPanelView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Copied out rather than held as `cx.theme()`: the column headers below need
        // `&mut Context`, and a live borrow of `cx` would conflict with them. `Hsla` is
        // `Copy`, so this costs nothing.
        let sidebar = cx.theme().sidebar;
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        let path = self.listing.path().to_string();
        let files: Vec<SftpFile> = self.listing.files().to_vec();
        let selected_count = files.iter().filter(|f| f.selected).count();

        let header = self.column_header(SftpColumn::Name, "名称", None, cx);
        let size_header = self.column_header(SftpColumn::Size, "大小", Some(SIZE_WIDTH), cx);
        let mtime_header =
            self.column_header(SftpColumn::Modified, "修改时间", Some(MTIME_WIDTH), cx);

        // The rows are virtual: `uniform_list` mounts only the visible window, so a
        // ten-thousand-entry directory costs one screenful of elements rather than ten
        // thousand. Row heights are uniform by construction — every row is one line of
        // the same columns — which is the one thing `uniform_list` asks for.
        let rows: Option<AnyElement> = if files.is_empty() {
            None
        } else {
            let panel = cx.entity();
            let scroll = self.list_scroll.clone();
            Some(
                uniform_list("sftp-rows", files.len(), move |range, _window, cx| {
                    let view = panel.read(cx);
                    let muted = cx.theme().muted_foreground;
                    let targets = view.targets.clone();
                    range
                        .clone()
                        .filter_map(|index| {
                            let file = view.listing.files().get(index)?;
                            Some(file_row(
                                &panel,
                                index,
                                file,
                                targets.clone(),
                                muted,
                            ))
                        })
                        .collect()
                })
                // Infer is what makes the list fill its flex container: the
                // toolkit's default (`Auto`) sizes the element from its style
                // alone, which in a flex column resolves to zero height and
                // mounts no rows at all.
                .with_sizing_behavior(ListSizingBehavior::Infer)
                .track_scroll(&scroll)
                .into_any_element(),
            )
        };

        // The directory's own actions, behind one menu: they are occasional, and four
        // more icons in a toolbar of five would double its width to say what a menu says
        // in one word. The shell asks for the name or the mode — a form belongs in a
        // dialog, not in a toolbar row.
        let chosen = self.listing.selected_paths();
        let for_rename = chosen.clone();
        let for_chmod = chosen;
        let more_menu = DropdownButton::new("sftp-more")
            .button(
                Button::new("sftp-more-trigger")
                    .icon(IconName::Ellipsis)
                    .ghost()
                    .tooltip(crate::i18n::t("更多操作", "More actions"))
                    .accessibility_label(crate::i18n::t("更多操作", "More actions")),
            )
            .dropdown_menu({
                let panel = cx.entity().downgrade();
                move |menu, _, _| {
                    let mkdir = panel.clone();
                    let touch = panel.clone();
                    let rename = panel.clone();
                    let chmod = panel.clone();
                    let renaming = for_rename.clone();
                    let moding = for_chmod.clone();
                    menu.item(
                        PopupMenuItem::new(crate::i18n::t("新建文件夹", "New folder")).on_click(
                            move |_, _, cx| {
                                if let Some(panel) = mkdir.upgrade() {
                                    let _ = panel.update(cx, |panel, cx| {
                                        panel.pending = Some(PanelAction::MkDir);
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .item(
                        PopupMenuItem::new(crate::i18n::t("新建文件", "New file")).on_click(
                            move |_, _, cx| {
                                if let Some(panel) = touch.upgrade() {
                                    let _ = panel.update(cx, |panel, cx| {
                                        panel.pending = Some(PanelAction::TouchFile);
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new(crate::i18n::t("重命名", "Rename")).on_click(
                            move |_, _, cx| {
                                if renaming.is_empty() {
                                    return;
                                }
                                if let Some(panel) = rename.upgrade() {
                                    let targets = renaming.clone();
                                    let _ = panel.update(cx, |panel, cx| {
                                        panel.pending = Some(PanelAction::Rename(targets));
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .item(
                        PopupMenuItem::new(crate::i18n::t("权限", "Permissions")).on_click(
                            move |_, _, cx| {
                                if moding.is_empty() {
                                    return;
                                }
                                if let Some(panel) = chmod.upgrade() {
                                    let targets = moding.clone();
                                    let _ = panel.update(cx, |panel, cx| {
                                        panel.pending = Some(PanelAction::Chmod(targets));
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                }
            });

        h_flex()
            // The grip is the panel rather than its toolbar, which is what the pane splitter
            // taught: a container receives the drag, while a row full of buttons hands the
            // press to whichever button is under it. The threshold below is what keeps a
            // click on a row from moving the panel, and the toolbar keeps its own handlers
            // so the gesture still works where a user would reach for it.
            //
            // Verified by dragging with this window raised: a synthetic drag aimed at a window
            // that is behind goes to whatever is in front instead, which looks exactly like a
            // handler that never fires. That cost two rounds of probes before the process was
            // checked rather than the code.
            .id("sftp-panel")
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, _| {
                    this.dock_press =
                        Some((f32::from(event.position.x), f32::from(event.position.y)));
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseUpEvent, _, cx| {
                    let Some((from_x, from_y)) = this.dock_press.take() else {
                        return;
                    };
                    let (dx, dy) = (
                        f32::from(event.position.x) - from_x,
                        f32::from(event.position.y) - from_y,
                    );
                    if dx.abs().max(dy.abs()) < 24.0 {
                        return;
                    }
                    this.pending = Some(PanelAction::Redock {
                        right: dx.abs() > dy.abs() && dx > 0.0,
                    });
                    cx.notify();
                }),
            )
            .size_full()
            // Stretched, not centred: an `h_flex` centres its items on the cross axis,
            // which is fine in a strip off the bottom and wrong in a column at the side,
            // where it left the toolbar, the headers and the rows floating in the middle
            // of a full-height panel with empty space above and below them.
            .items_stretch()
            .bg(sidebar)
            // The tree and the listing side by side. The original hides the tree when the
            // panel is narrow; this panel is wide, which is its shown case. A panel with
            // no tree — a local shell tab has no session to send one — hides the column
            // rather than reserving a blank strip that reads as broken.
            .when(!self.tree.is_empty(), |this| this.child(self.tree_column(cx)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    // The hairline faces the tree; with the tree hidden there is
                    // nothing left to separate, and the panel edge is already a frame.
                    .when(!self.tree.is_empty(), |this| this.border_l_1())
                    .border_color(border)
                    .child(
                        // The toolbar: where you are, and the actions that apply to the directory
                        // as a whole. Icons with tooltips rather than labelled buttons — five
                        // words across the top of a file list is a toolbar that reads as a form,
                        // and the icons are the ones the file manager beside it uses.
                        h_flex()
                            // The grip is the whole toolbar, not a thin band: a six-pixel
                            // element never receives a press in this window — measured when
                            // the pane splitter was built — so the gesture needs a real
                            // target. A click on a button inside still reaches the button,
                            // because a press that does not travel reports nothing.
                            .id("sftp-toolbar")
                            .w_full()
                            .gap_0p5()
                            .px_2()
                            .py_1()
                            .border_b_1()
                            .border_color(border)
                            .on_mouse_down(
                                gpui_kit::MouseButton::Left,
                                cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, _| {
                                    this.dock_press = Some((
                                        f32::from(event.position.x),
                                        f32::from(event.position.y),
                                    ));
                                }),
                            )
                            .on_mouse_up(
                                gpui_kit::MouseButton::Left,
                                cx.listener(|this, event: &gpui_kit::MouseUpEvent, _, cx| {
                                    let Some((from_x, from_y)) = this.dock_press.take() else {
                                        return;
                                    };
                                    let (dx, dy) = (
                                        f32::from(event.position.x) - from_x,
                                        f32::from(event.position.y) - from_y,
                                    );
                                    // A deliberate drag rather than a click: the toolbar is
                                    // full of buttons, and every one of them is also a
                                    // press-and-release in this row.
                                    if dx.abs().max(dy.abs()) < 24.0 {
                                        return;
                                    }
                                    this.pending = Some(PanelAction::Redock {
                                        right: dx.abs() > dy.abs() && dx > 0.0,
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(more_menu)
                            .child(
                                Button::new("sftp-up")
                                    .debug_selector(|| "sftp-up".to_string())
                                    // A corner arrow, not a plain up arrow: the toolbar
                                    // also carries upload — an up arrow too — and two
                                    // adjacent "up" glyphs that differ only in a line
                                    // under them read as the same button.
                                    .icon(Icon::new(IconName::CornerLeftUp))
                                    .ghost()
                                    .tooltip(crate::i18n::t("上级目录", "Parent directory"))
                                    .accessibility_label(crate::i18n::t(
                                        "上级目录",
                                        "Parent directory",
                                    ))
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.pending =
                                            Some(PanelAction::Navigate(this.listing.parent()));
                                    })),
                            )
                            .child(
                                Button::new("sftp-refresh")
                                    .debug_selector(|| "sftp-refresh".to_string())
                                    .icon(Icon::new(IconName::RotateCw))
                                    .ghost()
                                    .tooltip(crate::i18n::t("刷新", "Refresh"))
                                    .accessibility_label(crate::i18n::t("刷新", "Refresh"))
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.pending = Some(PanelAction::Refresh);
                                    })),
                            )
                            .child(
                                Button::new("sftp-upload")
                                    .icon(Icon::new(IconName::ArrowUpFromLine))
                                    .ghost()
                                    .tooltip(crate::i18n::t("上传文件", "Upload files"))
                                    .accessibility_label(crate::i18n::t("上传文件", "Upload files"))
                                    .on_click(cx.listener(|this, _, _, _| {
                                        this.pending = Some(PanelAction::Upload);
                                    })),
                            )
                            // Batch actions appear only when something is ticked, so the toolbar
                            // does not offer to download nothing.
                            .when(selected_count > 0, |this| {
                                let chosen = self.listing.selected_paths();
                                let for_download = chosen.clone();
                                let for_delete = chosen.clone();
                                this.child(
                                    Button::new("sftp-download-selected")
                                        .debug_selector(|| "sftp-download-selected".to_string())
                                        .icon(Icon::new(IconName::FileDown))
                                        .ghost()
                                        .tooltip(SharedString::from(format!(
                                            "{} ({selected_count})",
                                            crate::i18n::t("下载选中", "Download selected")
                                        )))
                                        .accessibility_label(crate::i18n::t(
                                            "下载选中",
                                            "Download selected",
                                        ))
                                        .on_click(cx.listener(move |this, _, _, _| {
                                            this.pending =
                                                Some(PanelAction::Download(for_download.clone()));
                                        })),
                                )
                                // The batch half of the cross-session copy: one entry per other
                                // open session, carrying everything ticked. It appears with the
                                // other batch actions, so the toolbar never offers to copy nothing.
                                .child(
                                    DropdownButton::new("sftp-copy-to")
                                        .button(
                                            Button::new("sftp-copy-to-trigger")
                                                .icon(Icon::new(IconName::Copy))
                                                .ghost()
                                                .tooltip(crate::i18n::t(
                                                    "复制到其他会话",
                                                    "Copy to another session",
                                                ))
                                                .accessibility_label(crate::i18n::t(
                                                    "复制到其他会话",
                                                    "Copy to another session",
                                                )),
                                        )
                                        .dropdown_menu({
                                            let targets = self.targets.clone();
                                            let chosen = chosen.clone();
                                            let panel = cx.entity().downgrade();
                                            move |menu, _, _| {
                                                let mut menu = menu;
                                                for (id, label) in targets.iter() {
                                                    let target = id.clone();
                                                    let paths = chosen.clone();
                                                    let panel = panel.clone();
                                                    let text = SharedString::from(format!(
                                                        "{} {label}",
                                                        crate::i18n::t("复制到", "Copy to")
                                                    ));
                                                    menu = menu.item(
                                                        PopupMenuItem::new(text).on_click(
                                                            move |_, _, cx| {
                                                                if let Some(panel) = panel.upgrade()
                                                                {
                                                                    let target = target.clone();
                                                                    let paths = paths.clone();
                                                                    let _ = panel.update(
                                                                        cx,
                                                                        |panel, cx| {
                                                                            panel.pending =
                                                                Some(PanelAction::CopyTo {
                                                                    paths,
                                                                    target,
                                                                });
                                                                            cx.notify();
                                                                        },
                                                                    );
                                                                }
                                                            },
                                                        ),
                                                    );
                                                }
                                                menu
                                            }
                                        }),
                                )
                                .child(
                                    Button::new("sftp-delete-selected")
                                        .debug_selector(|| "sftp-delete-selected".to_string())
                                        .icon(Icon::new(IconName::Trash))
                                        .ghost()
                                        .tooltip(SharedString::from(format!(
                                            "{} ({selected_count})",
                                            crate::i18n::t("删除选中", "Delete selected")
                                        )))
                                        .accessibility_label(crate::i18n::t(
                                            "删除选中",
                                            "Delete selected",
                                        ))
                                        .on_click(cx.listener(move |this, _, _, _| {
                                            this.pending =
                                                Some(PanelAction::Delete(for_delete.clone()));
                                        })),
                                )
                            })
                            .child(div().flex_1())
                            // The count sits where the eye already is when it has just ticked
                            // something, and says the number the batch buttons would act on.
                            .when(selected_count > 0, |this| {
                                this.child(div().text_xs().text_color(muted).child(
                                    SharedString::from(format!(
                                        "{} {selected_count}",
                                        crate::i18n::t("已选", "Selected")
                                    )),
                                ))
                            })
                            .when(self.is_loading(), |this| {
                                // A real spinner, not a static loader glyph: a still
                                // icon reads as decoration, a turning one as "working".
                                this.child(
                                    Spinner::new()
                                        .with_size(Size::Medium)
                                        .color(muted),
                                )
                            })
                            .child(
                                Button::new("sftp-collapse")
                                    .debug_selector(|| "sftp-collapse".to_string())
                                    .icon(IconName::X)
                                    .ghost()
                                    .tooltip(crate::i18n::t("隐藏文件面板", "Hide file panel"))
                                    .accessibility_label(crate::i18n::t(
                                        "隐藏文件面板",
                                        "Hide file panel",
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pending = Some(PanelAction::Collapse);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        // Where you are, with the icon that says this is a path rather than a
                        // stray line of text above the table.
                        h_flex()
                            .w_full()
                            .gap_1()
                            .items_center()
                            .px_2()
                            .py_1()
                            .child(Icon::new(IconName::FolderOpen).size_3().text_color(muted))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(SharedString::from(path)),
                            ),
                    )
                    .child(
                        // A hairline under the headings: what makes the row above the list read as
                        // a table's headings rather than as another row of the list.
                        h_flex()
                            .w_full()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .border_b_1()
                            .border_color(border)
                            // The tick box's width, so the name heading sits over the names.
                            .child(div().w(px(TICK_WIDTH)).flex_shrink_0())
                            .child(header)
                            .child(size_header)
                            .child(mtime_header),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .when(files.is_empty(), |this| {
                                this.child(
                                    div()
                                        .p_3()
                                        .text_sm()
                                        .text_color(muted)
                                        .child(crate::i18n::t("目录为空", "Empty directory")),
                                )
                            })
                            .children(rows),
                    )
                    // A listing that outlived its timeout is a fact the panel owns
                    // saying out loud: the spinner gave up, and "目录为空" over a
                    // silent panel would claim an answer that never arrived. A
                    // permission refusal arrives as the session's own error text
                    // and shows below as it is. The warning colour rides a bar
                    // rather than the text: yellow type on the light theme's
                    // near-white sidebar measured 1.84:1 — the warning was the
                    // one thing the panel did not show. Grey type carries the
                    // words; the bar carries the alarm.
                    .when(self.loading_timed_out(), |this| {
                        this.child(
                            h_flex()
                                .w_full()
                                .px_2()
                                .py_1()
                                .gap_1p5()
                                .items_center()
                                .child(
                                    div()
                                        .w(px(3.))
                                        .h(px(14.))
                                        .rounded(px(1.))
                                        .bg(warning),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(crate::i18n::t(
                                            "目录列表无响应——会话可能已断开,可刷新或重连",
                                            "The directory listing never answered — the                                              session may be gone; refresh or reconnect",
                                        )),
                                ),
                        )
                    })
                    .when_some(self.status.clone(), |this, status| {
                        this.child(
                            div()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(muted)
                                .child(status),
                        )
                    })
                    .into_any_element(),
            )
            .into_any_element()
    }
}

/// The default action shared by the name cell and its Open menu entry.
/// Only a confirmed directory navigates; other entries use the existing
/// read-only viewer, which owns file-size and unsupported-content handling.
fn file_row_action(file: &SftpFile) -> PanelAction {
    if file.is_dir {
        PanelAction::Navigate(file.full_path.clone())
    } else {
        PanelAction::View(file.full_path.clone())
    }
}

/// Selection replaces the type icon temporarily, without losing file type.
fn file_row_icon(file: &SftpFile) -> IconName {
    if file.selected {
        IconName::CircleCheck
    } else if file.is_dir {
        IconName::Folder
    } else {
        IconName::File
    }
}

/// One file row, built without the view's `Context`: the virtualized list renders
/// rows from inside a `&mut App` closure, so the click handlers carry an `Entity`
/// handle rather than a listener. Same actions, same element ids and debug
/// selectors as before — only who holds the handle changed.
fn file_row(
    panel: &Entity<SftpPanelView>,
    index: usize,
    file: &SftpFile,
    targets: Vec<(String, String)>,
    muted: Hsla,
) -> AnyElement {
    let name = if file.is_dir {
        format!("{}/", file.name)
    } else {
        file.name.clone()
    };
    let row_for_toggle = index;

    // Opening and selection stay separate: ticking a file must not preview it,
    // and opening a directory must not change the batch-selection set.
    let open_action = file_row_action(file);
    let tick_icon = file_row_icon(file);

    h_flex()
        .w_full()
        .gap_2()
        .py_0p5()
        .child(
            Button::new(SharedString::from(format!("sftp-tick-{index}")))
                // An id and a debug selector are two different things: the id names the
                // element for the hit test, and only the selector makes it findable by
                // `debug_bounds`. The name cell taught this the hard way last round.
                .debug_selector({
                    let selector = format!("sftp-tick-{index}");
                    move || selector.clone()
                })
                .icon(Icon::new(tick_icon))
                .ghost()
                .on_click({
                    let panel = panel.clone();
                    move |_, _, cx| {
                        let _ = panel.update(cx, |this, _| {
                            this.pending = Some(PanelAction::ToggleRow(row_for_toggle));
                        });
                    }
                }),
        )
        .child(
            div()
                // An id is required before an interaction handler: GPUI's hit test
                // needs to know which element owns the click, and only a stateful
                // element has an identity to name.
                .id(SharedString::from(format!("sftp-name-{index}")))
                .debug_selector({
                    let selector = format!("sftp-name-{index}");
                    move || selector.clone()
                })
                .flex_1()
                // The panel's width is the user's to drag: without the
                // floor-and-clip pair a long file name pushes the size and
                // mtime columns out of the pane instead of trimming.
                .min_w_0()
                .cursor_pointer()
                .when(file.is_dir, |this| this.font_weight(FontWeight::MEDIUM))
                .child(div().truncate().child(SharedString::from(name)))
                .on_click({
                    let action = open_action.clone();
                    let panel = panel.clone();
                    move |event, _, cx| {
                        // One click activates. The second half of a double-click
                        // must not queue another preview or directory request.
                        if event.click_count() > 1 {
                            return;
                        }
                        let _ = panel.update(cx, |this, cx| {
                            this.pending = Some(action.clone());
                            cx.notify();
                        });
                    }
                })
                // The name and Open use the same safe default: directories
                // navigate and files preview read-only. Editing or opening an
                // external application remains an explicit menu choice.
                .context_menu({
                    let open_path = file.full_path.clone();
                    let enter_action = open_action.clone();
                    let is_dir = file.is_dir;
                    let panel = panel.downgrade();
                    let other_tabs = targets;
                    move |menu, _, _| {
                        // One entry per other open session: the original copies a file
                        // by naming the target tab, and a menu is where its targets belong.
                        let mut menu = menu;
                        // Copying a path is the one row action a directory wants too.
                        // Build it before the early return so both kinds share it.
                        let copy_path_item =
                            PopupMenuItem::new(crate::i18n::t("复制路径", "Copy path"))
                                .on_click({
                                    let panel = panel.clone();
                                    let target = open_path.clone();
                                    move |_, _, cx| {
                                        if let Some(panel) = panel.upgrade() {
                                            let _ = panel.update(cx, |panel, cx| {
                                                panel.pending =
                                                    Some(PanelAction::CopyPath(target.clone()));
                                                cx.notify();
                                            });
                                        }
                                    }
                                });
                        for (id, label) in other_tabs.iter() {
                            let panel = panel.clone();
                            let target = id.clone();
                            let source = open_path.clone();
                            let text = SharedString::from(format!(
                                "{} {label}",
                                crate::i18n::t("复制到", "Copy to")
                            ));
                            menu = menu.item(PopupMenuItem::new(text).on_click(
                                move |_, _, cx| {
                                    if let Some(panel) = panel.upgrade() {
                                        let target = target.clone();
                                        let source = source.clone();
                                        let _ = panel.update(cx, |panel, cx| {
                                            panel.pending = Some(PanelAction::CopyTo {
                                                paths: vec![source],
                                                target,
                                            });
                                            cx.notify();
                                        });
                                    }
                                },
                            ));
                        }
                        {
                            // Cloned inside the body: this closure runs once per frame,
                            // so it may not move what it captured.
                            let action = enter_action.clone();
                            let panel_for_enter = panel.clone();
                            let menu = menu.item(
                                PopupMenuItem::new(crate::i18n::t("打开", "Open")).on_click(
                                    move |_, _, cx| {
                                        if let Some(panel) = panel_for_enter.upgrade() {
                                            let _ = panel.update(cx, |panel, cx| {
                                                panel.pending =
                                                    Some(action.clone());
                                                cx.notify();
                                            });
                                        }
                                    },
                                ),
                            );
                            if is_dir {
                                return menu.item(copy_path_item);
                            }
                            let target = open_path.clone();
                            let panel_for_open = panel.clone();
                            menu.item(
                                PopupMenuItem::new(crate::i18n::t(
                                    "用系统编辑器打开",
                                    "Open in the system editor",
                                ))
                                .on_click(
                                    move |_, _, cx| {
                                        if let Some(panel) = panel_for_open.upgrade() {
                                            let _ = panel.update(cx, |panel, cx| {
                                                panel.pending =
                                                    Some(PanelAction::OpenTemp(target.clone()));
                                                cx.notify();
                                            });
                                        }
                                    },
                                ),
                            )
                            .item(
                                // The unwatched half of the pair: look at the file with the
                                // desktop's default application and leave it alone.
                                PopupMenuItem::new(crate::i18n::t(
                                    "用默认程序打开",
                                    "Open with the default application",
                                ))
                                .on_click({
                                    let panel = panel.clone();
                                    let target = open_path.clone();
                                    move |_, _, cx| {
                                        if let Some(panel) = panel.upgrade() {
                                            let _ = panel.update(cx, |panel, cx| {
                                                panel.pending = Some(PanelAction::OpenDefault(
                                                    target.clone(),
                                                ));
                                                cx.notify();
                                            });
                                        }
                                    }
                                }),
                            )
                        }
                        .item(
                            PopupMenuItem::new(crate::i18n::t("查看", "View")).on_click({
                                let panel = panel.clone();
                                let target = open_path.clone();
                                move |_, _, cx| {
                                    if let Some(panel) = panel.upgrade() {
                                        let _ = panel.update(cx, |panel, cx| {
                                            panel.pending =
                                                Some(PanelAction::View(target.clone()));
                                            cx.notify();
                                        });
                                    }
                                }
                            }),
                        )
                        .item(
                            PopupMenuItem::new(crate::i18n::t("编辑", "Edit")).on_click({
                                let panel = panel.clone();
                                let target = open_path.clone();
                                move |_, _, cx| {
                                    if let Some(panel) = panel.upgrade() {
                                        let _ = panel.update(cx, |panel, cx| {
                                            panel.pending =
                                                Some(PanelAction::Edit(target.clone()));
                                            cx.notify();
                                        });
                                    }
                                }
                            }),
                        )
                        .item(copy_path_item)
                    }
                }),
        )
        .child(
            div()
                .w_24()
                .text_xs()
                .text_color(muted)
                .child(SharedString::from(file.size_text())),
        )
        .child(
            div()
                .w_32()
                .text_xs()
                .text_color(muted)
                .child(SharedString::from(file.modified_text())),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::RemoteEntry;
    use gpui_kit::gpui::{Modifiers, TestAppContext};

    /// A directory with a subdirectory, a big file and a recent one, so the rows carry the
    /// values the columns are supposed to show.
    fn listing() -> SftpListing {
        let entries = vec![
            RemoteEntry {
                name: "logs".into(),
                full_path: "/var/logs".into(),
                is_dir: true,
                size: 0,
                modified: 1_700_000_000,
                mode: 0o755,
            },
            RemoteEntry {
                name: "dump.sql".into(),
                full_path: "/var/dump.sql".into(),
                is_dir: false,
                size: 3 * 1024 * 1024,
                modified: 1_700_000_100,
                mode: 0o644,
            },
            RemoteEntry {
                name: "notes.txt".into(),
                full_path: "/var/notes.txt".into(),
                is_dir: false,
                size: 512,
                modified: 1_700_000_200,
                mode: 0o644,
            },
        ];
        let mut listing = SftpListing::default();
        listing.load("/var".into(), &entries);
        listing
    }

    /// Draws the panel over a listing and hands back the context to click in.
    /// A window whose panel already holds the listing.
    ///
    /// Deliberately not a helper that *returns* the context: `debug_bounds` and
    /// `simulate_click` live on the visual test context, which borrows the one the test
    /// owns, so that has to stay in the test's own scope. The listing is a helper because it
    /// is only data.
    fn panel_with_listing(
        _window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<SftpPanelView>,
    ) -> SftpPanelView {
        let mut panel = SftpPanelView::new(cx);
        panel.set_listing(listing(), cx);
        panel
    }

    /// The first thing that has ever exercised this panel with rows in it: every session on
    /// this machine is a local shell or a serial port, so the SFTP panel has always been
    /// empty in every check, and seven changes have gone into it unseen.
    ///
    /// Directories navigate; files use the existing read-only viewer. A
    /// filename must never be sent to the directory-listing command.
    #[gpui_kit::gpui::test]
    fn clicking_rows_navigates_directories_and_previews_files(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(panel_with_listing);
        cx.update(|window, cx| window.draw(cx).clear(cx));

        for (index, file) in listing().files().iter().enumerate() {
            let bounds = cx
                .debug_bounds(leaked(format!("sftp-name-{index}")))
                .expect("name cell");
            cx.simulate_click(bounds.center(), Modifiers::default());
            let action = view.update(cx, |panel, _| panel.take_action());
            let expected = if file.is_dir {
                PanelAction::Navigate(file.full_path.clone())
            } else {
                PanelAction::View(file.full_path.clone())
            };
            assert_eq!(action, Some(expected));
            assert_eq!(
                view.read_with(cx, |panel, _| panel.listing.selected_count()),
                0
            );
        }
    }

    #[gpui_kit::gpui::test]
    fn row_context_open_uses_directory_or_read_only_file_action(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(panel_with_listing);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        for (index, file) in listing().files().iter().enumerate() {
            let bounds = cx
                .debug_bounds(leaked(format!("sftp-name-{index}")))
                .expect("name cell");
            cx.simulate_mouse_move(bounds.center(), None, Modifiers::default());
            cx.simulate_mouse_down(
                bounds.center(),
                gpui_kit::MouseButton::Right,
                Modifiers::default(),
            );
            cx.simulate_mouse_up(
                bounds.center(),
                gpui_kit::MouseButton::Right,
                Modifiers::default(),
            );
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
            // Popup-menu keyboard activation is the existing keyboard path.
            cx.simulate_keystrokes("down enter");
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let action = view.update(cx, |panel, _| panel.take_action());
            let expected = if file.is_dir {
                PanelAction::Navigate(file.full_path.clone())
            } else {
                PanelAction::View(file.full_path.clone())
            };
            assert_eq!(action, Some(expected));
        }
    }

    /// And ticking a row reports the row that was ticked, by index — which is the contract
    /// the shell's toggle depends on.
    #[gpui_kit::gpui::test]
    fn ticking_a_row_reports_which_row_it_was(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(panel_with_listing);
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        let bounds = cx
            .debug_bounds(leaked("sftp-tick-2".to_string()))
            .unwrap_or_else(|| panic!("row 2's tick box was not drawn"));
        cx.simulate_click(bounds.center(), Modifiers::default());

        let action = view.update(cx, |panel, _| panel.take_action());
        assert_eq!(action, Some(PanelAction::ToggleRow(2)));
    }

    /// The toolbar's two navigation buttons, which until now had never been pressed either.
    ///
    /// `up` reports the parent of the listing's own directory — computed by the model, not
    /// by the panel — and `refresh` reports a plain re-read.
    #[gpui_kit::gpui::test]
    fn the_toolbar_reports_a_parent_navigation_and_a_refresh(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(panel_with_listing);
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        let bounds = cx
            .debug_bounds("sftp-up")
            .unwrap_or_else(|| panic!("the up button was not drawn"));
        cx.simulate_click(bounds.center(), Modifiers::default());
        let up = view.update(cx, |panel, _| panel.take_action());
        assert_eq!(
            up,
            Some(PanelAction::Navigate("/".into())),
            "up from /var goes to the root"
        );

        let bounds = cx
            .debug_bounds("sftp-refresh")
            .unwrap_or_else(|| panic!("the refresh button was not drawn"));
        cx.simulate_click(bounds.center(), Modifiers::default());
        let refresh = view.update(cx, |panel, _| panel.take_action());
        assert_eq!(refresh, Some(PanelAction::Refresh));
    }

    /// A header reports a sort of *its* column — the third time this panel's rows and
    /// headers have needed a selector before a test could see them, which is now the most
    /// reliable predictor of a UI test that cannot find its element.
    #[gpui_kit::gpui::test]
    fn clicking_a_header_reports_a_sort_of_that_column(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(panel_with_listing);
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        let size_header = leaked(format!("sftp-col-{:?}", crate::core::SftpColumn::Size));
        let bounds = cx
            .debug_bounds(size_header)
            .unwrap_or_else(|| panic!("the size header was not drawn"));
        cx.simulate_click(bounds.center(), Modifiers::default());

        let action = view.update(cx, |panel, _| panel.take_action());
        assert_eq!(
            action,
            Some(PanelAction::Sort(crate::core::SftpColumn::Size))
        );
    }

    /// The batch buttons, which offer themselves only when there is something to act on —
    /// so this test covers two things: that a ticked row reaches the action, and that
    /// nothing ticked means no button at all rather than a button that downloads nothing.
    #[gpui_kit::gpui::test]
    fn the_batch_buttons_offer_themselves_only_with_a_row_ticked(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(panel_with_listing);
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        assert!(
            cx.debug_bounds("sftp-download-selected").is_none(),
            "nothing is ticked, so there is nothing to download"
        );

        // Tick the second row through the listing itself, which is what the shell does when
        // the panel reports a toggle.
        let expected = view.update(cx, |panel, cx| {
            let index = 1;
            panel.listing.toggle_selected(index);
            let path = panel.listing.files()[index].full_path.clone();
            cx.notify();
            path
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        let bounds = cx
            .debug_bounds("sftp-download-selected")
            .unwrap_or_else(|| panic!("the download button appeared with a row ticked"));
        cx.simulate_click(bounds.center(), Modifiers::default());

        let action = view.update(cx, |panel, _| panel.take_action());
        assert_eq!(
            action,
            Some(PanelAction::Download(vec![expected])),
            "the download carries the row that was ticked"
        );
    }

    /// `debug_bounds` wants a `'static` selector and these are built from indices.
    fn leaked(selector: String) -> &'static str {
        Box::leak(selector.into_boxed_str())
    }

    /// The drain's staleness comparison. The panel records which tab's
    /// generation its copy reflects; a different tab's generation is not an
    /// answer, because the comparison only ever runs against the tab showing.
    #[gpui_kit::gpui::test]
    fn synced_generation_answers_only_for_the_tab_it_synced(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let _ = window;
            SftpPanelView::new(cx)
        });
        view.update(cx, |panel, cx| {
            assert_eq!(
                panel.synced_generation("tab-1"),
                None,
                "a fresh panel has synced nothing"
            );
            panel.mark_synced("tab-1", 7, cx);
            assert_eq!(panel.synced_generation("tab-1"), Some(7));
            assert_eq!(
                panel.synced_generation("tab-2"),
                None,
                "another tab's generation is not an answer"
            );
        });
    }
    #[test]
    fn row_icons_distinguish_types_and_keep_the_selection_check() {
        let listing = listing();
        for original in listing.files() {
            let mut file = original.clone();
            assert_eq!(
                file_row_icon(&file),
                if file.is_dir {
                    IconName::Folder
                } else {
                    IconName::File
                }
            );
            file.selected = true;
            assert_eq!(file_row_icon(&file), IconName::CircleCheck);
            file.selected = false;
            assert_eq!(
                file_row_icon(&file),
                if file.is_dir {
                    IconName::Folder
                } else {
                    IconName::File
                }
            );
        }
    }

    #[test]
    fn non_directory_entries_never_infer_navigation_from_a_name_or_mode() {
        // SftpFile intentionally retains only is_dir plus permission bits,
        // not a symlink/unknown-type tag. Non-directory projections keep the
        // safe file fallback; the UI must not infer a type from rwx or a name.
        for (name, mode) in [("link-to-folder", 0o777), ("unknown", 0), ("file/", 0o644)] {
            let mut file = listing().files()[1].clone();
            file.name = name.into();
            file.full_path = format!("/var/{name}");
            file.mode = mode;
            assert!(!file.is_dir);
            assert_eq!(
                file_row_action(&file),
                PanelAction::View(file.full_path.clone())
            );
            assert_eq!(file_row_icon(&file), IconName::File);
        }
    }

    #[gpui_kit::gpui::test]
    fn double_click_does_not_repeat_the_primary_action(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(panel_with_listing);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        for (index, file) in listing().files().iter().enumerate() {
            let position = cx
                .debug_bounds(leaked(format!("sftp-name-{index}")))
                .unwrap()
                .center();
            cx.simulate_click(position, Modifiers::default());
            assert_eq!(
                view.update(cx, |panel, _| panel.take_action()),
                Some(file_row_action(file))
            );
            cx.simulate_event(gpui_kit::MouseDownEvent {
                position,
                button: gpui_kit::MouseButton::Left,
                modifiers: Modifiers::default(),
                click_count: 2,
                first_mouse: false,
            });
            cx.simulate_event(gpui_kit::MouseUpEvent {
                position,
                button: gpui_kit::MouseButton::Left,
                modifiers: Modifiers::default(),
                click_count: 2,
            });
            assert!(view.update(cx, |panel, _| panel.take_action()).is_none());
            assert_eq!(
                view.read_with(cx, |panel, _| panel.listing.selected_count()),
                0
            );
        }
    }

    #[gpui_kit::gpui::test]
    fn file_menu_retains_explicit_edit_external_and_copy_actions(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(panel_with_listing);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let path = listing().files()[1].full_path.clone();
        for (steps, expected) in [
            (2, PanelAction::OpenTemp(path.clone())),
            (3, PanelAction::OpenDefault(path.clone())),
            (4, PanelAction::View(path.clone())),
            (5, PanelAction::Edit(path.clone())),
            (6, PanelAction::CopyPath(path.clone())),
        ] {
            let position = cx.debug_bounds("sftp-name-1").unwrap().center();
            cx.simulate_mouse_move(position, None, Modifiers::default());
            cx.simulate_mouse_down(position, gpui_kit::MouseButton::Right, Modifiers::default());
            cx.simulate_mouse_up(position, gpui_kit::MouseButton::Right, Modifiers::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
            cx.simulate_keystrokes(&format!("{}enter", "down ".repeat(steps)));
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(
                view.update(cx, |panel, _| panel.take_action()),
                Some(expected)
            );
        }
        view.update(cx, |panel, cx| {
            panel.set_targets(vec![("another-tab".into(), "Another fixture".into())], cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let position = cx.debug_bounds("sftp-name-1").unwrap().center();
        cx.simulate_mouse_move(position, None, Modifiers::default());
        cx.simulate_mouse_down(position, gpui_kit::MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(position, gpui_kit::MouseButton::Right, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("down enter");
        assert_eq!(
            view.update(cx, |panel, _| panel.take_action()),
            Some(PanelAction::CopyTo {
                paths: vec![path],
                target: "another-tab".into()
            })
        );
    }
}
