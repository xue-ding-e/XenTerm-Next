use anyhow::Context as _;
use anyhow::Result;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::component::{
    h_flex, v_flex,
    ActiveTheme as _,
    Root,
    // `open_dialog` and `close_dialog` live on this trait, not on `Window`: without it
    // in scope a dialog is a method that does not exist.
    WindowExt as _,
};
use gpui_kit::gpui::KeyDownEvent;
use gpui_kit::{
    div, prelude::*, px, size, Animation, AnimationExt as _, Context, Entity, PromptLevel,
    SharedString, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowHandle,
    WindowOptions,
};

use crate::session::protocol::SessionCommand;

use super::pages::{
    PageId, Pages, SessionsAction, SessionsPage, SettingsPage, TerminalAction,
};
use super::{
    process_window_title, system_info_window_title, EditorOutcome, GroupManagerAction,
    GroupManagerView, PanelAction, ProcessWindowView, QuickManagerAction, QuickManagerView,
    RuleEditorAction, RuleEditorView, SessionEditor, SessionListView, SessionState,
    SettingsAction, SftpPanelView, SystemInfoWindowView, TransferAction, TunnelsView, WINDOW_ID,
};

/// Open the XenTerm window and run the platform event loop until it closes.
///
/// The loop ends by itself once the last window is gone: `QuitMode::Default`
/// quits when the window count reaches zero on every platform but macOS, so
/// nothing here has to ask it to stop, and the `Err` case is the only part of
/// *this* function's own contract that needs arranging.
///
/// What the loop ending does not end is the plugin processes started before it.
/// Those are arranged for after it returns, at the end of this function, and a
/// failure to open the window goes through the same path — the plugins were
/// started either way.
pub(crate) fn run() -> Result<()> {
    // Built here rather than inside the supervisor, because the runtime is the
    // application's and not the kernel's: the CLI owns one and this owns the UI's.
    // What the kernel takes is a handle, which
    // is also what every other long-lived task in this crate takes.
    let runtime = std::sync::Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("create the GPUI runtime")?,
    );
    // Started before the window rather than after it. A plugin can take seconds to
    // introduce itself — a bundled runtime on a cold disk cache — and those seconds
    // should be spent while the window is appearing, not after the user has started
    // waiting for a plugin they cannot see yet.
 

    // A channel rather than the closure's return value, because
    // `Application::run`'s callback returns nothing and the loop it runs is
    // blocking: this is the only way a failure to open the window reaches the
    // caller. `Sender` is moved in, so the callback's `'static` bound is met
    // without sharing anything.
    let (failed, failures) = std::sync::mpsc::channel();

    // Cloned into the window, because the shell hands the runtime to every session it
    // starts: `ConnectCtx` wants an `Arc<Runtime>` since the SFTP bootstrap task
    // outlives the call that begins it. The supervisor keeps a handle.
    let runtime_arc = runtime.clone();

    // Loaded before the window, not inside its closure, so a failure can propagate:
    // an unreadable config is fatal. Opening
    // a window onto an empty store would silently show no saved sessions while the
    // real ones sat on disk, which is worse than not opening — the user would have
    // reason to think their sessions were gone.
    let store = std::rc::Rc::new(std::cell::RefCell::new(
        crate::config::ConfigStore::load().context("load the saved sessions for the UI shell")?,
    ));

    gpui_kit::application()
        // `AllAssets` rather than the curated default `Assets`, because this shell draws
        // from the full Lucide catalog: the default bundle is a deliberately short list
        // (`zap`, `plug`, `server` and `puzzle` are all absent from it), and an asset
        // that cannot be loaded is a panic inside the paint, not a missing icon.
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            // The toolkit's own widgets translate from its own catalog — its settings search
            // box, its list placeholders — and that catalog defaults to English, which is how
            // one "Search..." appeared in an otherwise Chinese window. Following this app's
            // language is one line rather than a string per widget.
            // `rust_i18n` matches on the whole tag and falls back to its default locale, which
            // is English: passing this app's own `"zh"` left every toolkit string English.
            gpui_kit::component::set_locale(if crate::i18n::is_en() { "en" } else { "zh-CN" });
            gpui_kit::init(cx);
            super::dialogs::init(cx);
            // The keymap, in one place: every chord the window answers to.
            super::actions::init(cx);
            // The taskbar's "新建窗口" task, for *this* entry point: the GPUI window is
            // started with `gpui`, so that is what the task has to pass. Registered
            // before the first window shows, so the entry is there when the taskbar icon
            // appears; a failure is a missing menu item rather than a failed start.
            #[cfg(windows)]
            crate::app::jump_list::register_task("gpui", crate::i18n::t("新建窗口", "New window"));
            // The window's theme, from the setting: dark, light, or whichever the system is
            // set to. The terminal is dark whatever this says — light-on-dark is what a
            // terminal is for, and one that followed a light window would be white text on
            // white — so a light window is a black pane in a white frame, which is the
            // arrangement every terminal emulator that offers a light theme uses.
            match store.borrow().theme_pref() {
                "dark" => {
                    gpui_kit::component::Theme::change(
                        gpui_kit::component::ThemeMode::Dark,
                        None,
                        cx,
                    )
                }
                "light" => {
                    gpui_kit::component::Theme::change(
                        gpui_kit::component::ThemeMode::Light,
                        None,
                        cx,
                    )
                }
                _ => gpui_kit::component::Theme::sync_system_appearance(None, cx),
            }

            // The fonts, registered before any window opens. Meatshell Mono is
            // the terminal's default — a monospace face, which is what a cell
            // grid requires; a GPUI shell that did not register it would fall
            // back to whatever the platform calls monospace, which is a
            // different terminal. Its Regular and Bold faces are registered so
            // a session that asks for bold gets the real one rather than a
            // synthesised slant. MiSans and HarmonyOS Sans SC ship for the
            // text system at large; they are proportional and are deliberately
            // absent from the terminal's font picker.
            //
            // A failure is a missing family rather than a fatal error: the
            // theme's own font is still there to fall back on.
            let embedded_fonts: Vec<std::borrow::Cow<'static, [u8]>> = vec![
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/MeatshellMono-Regular.ttf"
                )),
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/MeatshellMono-Bold.ttf"
                )),
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/MiSans-Regular.ttf"
                )),
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/MiSans-Bold.ttf"
                )),
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/HarmonyOS_Sans_SC_Regular.ttf"
                )),
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/HarmonyOS_Sans_SC_Bold.ttf"
                )),
            ];
            if let Err(error) = cx.text_system().add_fonts(embedded_fonts) {
                tracing::warn!("could not register the bundled fonts: {error:#}");
            }
            // The fonts the user imported live as files in the data directory's
            // fonts/ folder. Registering them here puts every import on the
            // same footing as the embedded faces for this launch; a file the
            // user drops in between launches is picked up by this scan, and a
            // file they delete simply stops being registered.
            let mut imported = 0u32;
            if let Ok(entries) = std::fs::read_dir(crate::core::fonts::imported_fonts_dir()) {
                let files: Vec<std::path::PathBuf> = entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| {
                        matches!(
                            path.extension().and_then(|e| e.to_str()),
                            Some("ttf") | Some("otf") | Some("otc") | Some("ttc")
                        )
                    })
                    .collect();
                let owned: Vec<std::borrow::Cow<'static, [u8]>> = files
                    .iter()
                    .filter_map(|path| std::fs::read(path).ok())
                    .map(std::borrow::Cow::Owned)
                    .collect();
                imported = owned.len() as u32;
                if !owned.is_empty() {
                    if let Err(error) = cx.text_system().add_fonts(owned) {
                        tracing::warn!("could not register the imported fonts: {error:#}");
                    }
                }
            }
            if imported > 0 {
                tracing::info!("registered {imported} imported font file(s)");
            }

            // Sized before the window rather than after it, because
            // `WindowBounds::centered` wants `&App` to ask which display to
            // center on, and `open_window` takes `&mut App`. Two borrows of `cx`
            // in one expression would not compile, and the order here is the only
            // one that reads as "decide where, then open".
            let bounds = WindowBounds::centered(size(px(1280.), px(800.)), cx);
            let options = WindowOptions {
                window_bounds: Some(bounds),
                // A navigation rail and a terminal wide enough to be a terminal: below
                // this the pane stops having room for a 40-column shell, and a terminal
                // that loses columns reflows the user's output.
                window_min_size: Some(size(px(1100.), px(520.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("XenTerm".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };

            match cx.open_window(options, |window, cx| {
                let shell = cx.new(|cx| {
                    let gates: crate::terminal::RenderGates = std::sync::Arc::new(
                        std::sync::Mutex::new(std::collections::HashMap::new()),
                    );

                    // Everything a connect needs, built once for the window. The gates
                    // go in rather than being made twice: a producer's ticket and the
                    // view's flush have to be the same gate.
                    let state = SessionState::new(runtime_arc.clone(), gates, store.clone());

                    // What the terminal page needs to derive its pane width from: the
                    // window's width, written here each frame. A shared `Cell` rather
                    // than a field on the shell, because the page is built before the
                    // shell that measures.
                    let window_width: Rc<Cell<f32>> = Rc::new(Cell::new(0.0));

                    // The terminal workspace. It is a page like the rest — but it is
                    // the window's reason to exist, it holds the first tab, and it is
                    // built now rather than on first entry.
                    let terminal = cx.new(|cx| {
                        super::pages::TerminalPage::new(
                            state.clone(),
                            window_width.clone(),
                            window,
                            cx,
                        )
                    });

                    let mut shell = Shell {
                        pages: Pages::new(terminal),
                        window_width,
                        webdav_task: None,
                        approval_queue: Vec::new(),
                        approval_opened_at: std::collections::HashMap::new(),
                        approval_reported: std::collections::HashSet::new(),
                        approval_shown: std::collections::HashSet::new(),
                        audit_last_prune_day: None,
                        approval_poll: None,
                        process_window: Detached::new(process_window_title),
                        audit_window: crate::ui::AuditWindowHandle::new(),
                        system_info_window: Detached::new(system_info_window_title),
                        open_file: None,
                        state,
                        overlay: Overlay::None,
                        persistence_warning: PersistenceWarning::default(),
                        status: None,
                        status_epoch: 0,
                        status_expiry: None,
                        quick_connect_pick: None,
                        _quick_connect_subscription: None,
                        _root_subscription: None,
                    };
                    // The approval poll owns no window and needs none: it only
                    // fills the queue, and render shows the dialogs.
                    shell.start_approval_poll(cx);
                    shell
                });
                // `Root` has to be the window's first view. It is what the
                // component library's dialogs, sheets, notifications and tooltips
                // look up, and they panic rather than degrade when the window's
                // root is anything else — so adding it now costs one line, and
                // adding it at the first overlay would cost finding that panic.
                cx.new(|cx| Root::new(shell, window, cx))
            }) {
                Ok(_) => {}
                Err(error) => {
                    let _ = failed.send(error);
                    // Without this the process would sit in a platform message
                    // loop with no window on screen and nothing left that could
                    // ever close one, which reads as a hang rather than a failure.
                    cx.quit();
                }
            }
        });

    // A queued auth prompt is a connection parked on a one-shot channel waiting for a
    // dialog. The window that would have shown it is gone, so answering them all is
    // what turns "the window closed" into a connection that fails cleanly rather than
    // one that waits forever.
    crate::session::abort_window(WINDOW_ID);

    // After the loop rather than inside one of its callbacks, because this blocks
    // and the platform loop is the thing being blocked on. It also runs on the way
    // out of the failure path: a window that never opened still started whatever
    // plugins the user granted, and those are not the callback's to leave behind.
    //
 

    match failures.try_recv() {
        Ok(error) => Err(error),
        Err(_) => Ok(()),
    }
}

/// One note on the window's status line, with the bookkeeping its exit needs.
#[derive(Clone)]
pub(crate) struct StatusNote {
    text: String,
    /// When the note was said. Two notes compare equal only to themselves,
    /// which is how the expiry task recognises that the line it is retiring is
    /// still the one it was scheduled for.
    at: std::time::Instant,
    /// Sticky notes stay until replaced — "syncing…" describes an operation in
    /// flight, and expiring it mid-sync would be a lie.
    sticky: bool,
    /// Set a fade before retirement, so the note leaves by dimming rather than
    /// by vanishing between two frames.
    fading: bool,
}

/// How long a regular note stays readable before it starts to leave.
const STATUS_TTL: std::time::Duration = std::time::Duration::from_secs(6);
/// How long the fade out takes.
const STATUS_FADE: std::time::Duration = std::time::Duration::from_millis(400);

/// Save failures outlive the transient status line. The message comes only from
/// `ConfigStore::persistence_error`, which returns fixed, sanitized guidance,
/// never raw database errors, paths, or session details. The store owns when a
/// successful save clears it.
#[derive(Default)]
struct PersistenceWarning {
    message: Option<String>,
}

impl PersistenceWarning {
    /// Returns whether the window needs repainting. Repeated failed polls must
    /// neither dismiss the warning nor request a new frame every second.
    fn refresh(&mut self, error: Option<String>) -> bool {
        let changed = self.message != error;
        self.message = error;
        changed
    }

    fn render(&self, cx: &gpui_kit::App) -> Option<impl IntoElement> {
        self.message.as_ref().map(|message| {
            div()
                .id("configuration-save-warning")
                .debug_selector(|| "configuration-save-warning".to_string())
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(cx.theme().warning)
                .bg(cx.theme().warning)
                .text_color(cx.theme().warning_foreground)
                .text_sm()
                // Let the recovery advice wrap, rather than truncating the
                // part that tells the user to preserve their pending edits.
                .child(SharedString::from(message.clone()))
        })
    }
}

/// The window's contents: a navigation rail beside the active page.
///
/// The first real layout decision of the migration, and the one this pass is about:
/// the window is a *rail* and a *page*. A click on the rail switches the page —
/// there is no anchor to scroll to and no panel to fold — and each page keeps its
/// own state in an entity that outlives the visit, so switching away and back is
/// exactly leaving a room and coming back to it.
pub(crate) struct Shell {
    /// Every page, and which one is showing.
    pub(crate) pages: Pages,
    /// What the terminal page needs to derive its pane width from, written each
    /// frame before the tree is built. See [`TerminalPage::render_workspace`].
    window_width: Rc<Cell<f32>>,
    /// The WebDAV sync in flight, if any. Kept because dropping a Task cancels it.
    webdav_task: Option<gpui_kit::Task<()>>,
    /// Risky MCP commands waiting for a human at this window. Filled by the
    /// approval poll; each head-of-queue entry is shown once as a dialog and
    /// resolved by whichever button the human presses.
    approval_queue: Vec<crate::automation::approval::ApprovalRequest>,
    /// When the dialog for each shown request was opened — the UI-side
    /// decision deadline the deny button's countdown runs against.
    approval_opened_at: std::collections::HashMap<String, std::time::Instant>,
    /// Requests whose completion has already been reported on the status
    /// line, so the poll reports a decision once, not once per second.
    approval_reported: std::collections::HashSet<String>,
    /// The UTC day the audit retention sweep last ran, "YYYY-MM-DD". The
    /// sweep runs when the day turns, not on every poll tick.
    audit_last_prune_day: Option<String>,
    /// Requests a dialog has already been shown for. The drain runs every
    /// frame and the poll re-reads the same pending file every second; the
    /// set is what keeps those two loops from stacking one dialog per frame
    /// on top of the last. Entries leave when the request resolves (its file
    /// disappears, so it is never re-queued).
    approval_shown: std::collections::HashSet<String>,
    /// How long the approval poll has been running — a task started once at
    /// construction, kept so dropping it would stop the window ever seeing
    /// approval requests.
    approval_poll: Option<gpui_kit::Task<()>>,
    /// The process monitor, while it is open.
    ///
    /// Detached rather than a panel: a table you keep beside the terminal. See
    /// [`Detached`] for what that means for opening, focusing and following a tab.
    process_window: Detached<ProcessWindowView>,
    /// The approval-audit journal viewer, in its own window. Opened from the
    /// settings page's 查看审批记录 button; the journal is a record to browse,
    /// and a browse needs a surface that does not fight the form's scroll.
    audit_window: crate::ui::AuditWindowHandle,
    /// The system-information window, on the same terms.
    system_info_window: Detached<SystemInfoWindowView>,
    /// The remote file the session's SftpFileText opened, if any. Its actions are
    /// drained from here rather than subscribed to: this view reports through
    /// `take_action`, like every other panel in this shell.
    open_file: Option<Entity<super::file_viewer::FileViewerView>>,
    /// The window's shared session stores.
    ///
    /// Held here rather than by any one page, because a connect needs all of it —
    /// handles, buffers, gates, statuses, the SFTP channels and the store — and a
    /// page only draws one screen.
    state: SessionState,
    /// Which modal dialog is open over the window, if any.
    ///
    /// Everything that used to be a full-window overlay is a page now; what is
    /// left here is what is genuinely modal — a form being filled in, a file
    /// being read, a thing the user is mid-way through.
    pub(crate) overlay: Overlay,
    /// A persistent warning independent of transient connection/status notices.
    /// Refreshed after foreground actions and by the existing background poll.
    persistence_warning: PersistenceWarning,
    /// A status line for the window, under the content: connect and disconnect notices,
    /// and the answer to a command that had none of its own.
    status: Option<StatusNote>,
    /// Bumped by every `say`, so each note's fade animation carries an element id
    /// no earlier note used. A reused id would let a new note inherit the old
    /// note's animation clock — fade already spent, message invisible.
    status_epoch: u64,
    /// The expiry task for the current note. Replaced on every `say`; dropping it
    /// cancels the old schedule, which is what makes "new note" also mean "old
    /// note's fade-out is off".
    status_expiry: Option<gpui_kit::Task<()>>,
    /// The session the quick-connect palette confirmed, drained at the top of
    /// the next frame. The subscription's context is a bare `&App`, so the
    /// pick travels out through here rather than being acted on in place.
    quick_connect_pick: Option<String>,
    /// The quick-connect palette's Confirm subscription, alive while the dialog
    /// is open. A dropped subscription is a list that renders and does nothing.
    _quick_connect_subscription: Option<Subscription>,
    /// Keeps this window repainting when the modal queue changes.
    ///
    /// Taken on the first frame, when the window's root exists to be observed. See
    /// [`super::follow_root`] for why the shell cannot mount the dialog layer without it.
    _root_subscription: Option<Subscription>,
}

/// An overlay drawn over the whole window, as a dialog.
///
/// One at a time, because they are all modal-ish: a form half filled in with another
/// behind it would invite a click on the wrong one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Overlay {
    None,
    /// A remote file, open in the built-in viewer or editor.
    File(Entity<super::file_viewer::FileViewerView>),
    /// The session editor, open on a new or an existing session.
    ///
    /// Held as an entity rather than a flag, because the editor owns the draft being
    /// filled in and a flag would have nowhere to keep it.
    Editor(Entity<SessionEditor>),
    /// The session-group manager, on the same terms — and the place group rename and
    /// delete live, because a group's heading turned out not to be a clickable target.
    Groups(Entity<GroupManagerView>),
    /// The output-highlight rule editor, opened from the settings page.
    RuleEditor(Entity<RuleEditorView>),
    /// The quick-command manager, opened from the terminal page's dock: a form
    /// being filled in, which is dialog work and not page work.
    QuickManager(Entity<QuickManagerView>),
    /// The active session's port forwards, opened from the tab strip.
    Tunnels(Entity<TunnelsView>),
    /// The quick-connect palette, opened from the tab strip's `+` or Ctrl+K:
    /// a searchable list over every saved session and built-in shell, whose
    /// confirm connects in place.
    QuickConnect(Entity<SessionListView>),
    /// The command palette, opened from Ctrl+Shift+P: a searchable list over
    /// the window's own commands, whose confirm runs one.
    Commands(Entity<super::command_palette::CommandPalette>),
}

/// Join a directory and a name into the remote path the SFTP layer takes.
///
/// One slash between them, whatever the directory ends with: a doubled slash is a path
/// some servers accept and others read as a different one.
fn join_remote(dir: &str, name: &str) -> String {
    let dir = dir.trim_end_matches('/');
    let name = name.trim_start_matches('/');
    if dir.is_empty() {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

#[cfg(test)]
mod join_remote_tests {
    use super::join_remote;

    /// Every create and rename in the file panel goes through this, and a doubled slash is
    /// the kind of wrong path a server accepts for a while and then does not.
    #[test]
    fn a_directory_and_a_name_have_one_slash_between_them() {
        assert_eq!(
            join_remote("/home/user", "notes.txt"),
            "/home/user/notes.txt"
        );
        assert_eq!(
            join_remote("/home/user/", "notes.txt"),
            "/home/user/notes.txt",
            "a trailing slash on the directory must not double"
        );
        assert_eq!(
            join_remote("/home/user", "/notes.txt"),
            "/home/user/notes.txt",
            "a leading slash on the name must not double either"
        );
        assert_eq!(
            join_remote("/home/user///", "notes.txt"),
            "/home/user/notes.txt"
        );
    }

    /// The root is a directory, not an empty string: `"" + "/" + name` would be a path
    /// with no directory at all, which is the one form that reaches the session's working
    /// directory instead of the one the panel is showing.
    #[test]
    fn the_root_stays_the_root() {
        assert_eq!(join_remote("/", "notes.txt"), "/notes.txt");
        assert_eq!(join_remote("", "notes.txt"), "/notes.txt");
    }

    /// A name is not required to be one level deep: what the user typed is what is sent,
    /// because quietly rewriting it is worse than a server that refuses.
    #[test]
    fn a_name_that_looks_like_a_path_is_kept() {
        assert_eq!(join_remote("/home/user", "a/b"), "/home/user/a/b");
    }
}

#[cfg(test)]
mod persistence_warning_tests {
    use super::*;
    use gpui_kit::gpui::TestAppContext;

    const CONFLICT: &str = "Configuration changed elsewhere. Your changes were not saved. Keep pending edits, then reopen XenTerm to reload.";
    const STORAGE_FAILURE: &str = "Configuration could not be saved. Keep pending edits before closing XenTerm.";

    /// A minimal host exercises the shell's actual banner without starting
    /// terminals, resource samplers, approval scans, or database workers.
    struct WarningHost {
        warning: PersistenceWarning,
    }

    impl Render for WarningHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children(self.warning.render(cx))
        }
    }

    #[test]
    fn only_changed_save_feedback_requests_a_repaint() {
        let mut warning = PersistenceWarning::default();
        assert!(!warning.refresh(None));
        assert!(warning.refresh(Some(CONFLICT.into())));
        assert!(!warning.refresh(Some(CONFLICT.into())));
        assert_eq!(warning.message.as_deref(), Some(CONFLICT));
        assert!(warning.refresh(Some(STORAGE_FAILURE.into())));
        assert!(warning.refresh(None));
        assert!(!warning.refresh(None));
    }

    #[gpui_kit::gpui::test]
    fn save_warning_stays_visible_until_the_store_clears_it(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|_, _| WarningHost {
            warning: PersistenceWarning::default(),
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert!(cx.debug_bounds("configuration-save-warning").is_none());

        for message in [CONFLICT, CONFLICT, STORAGE_FAILURE] {
            view.update(cx, |host, cx| {
                if host.warning.refresh(Some(message.into())) {
                    cx.notify();
                }
            });
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
            assert!(cx.debug_bounds("configuration-save-warning").is_some());
            view.read_with(cx, |host, _| {
                assert_eq!(host.warning.message.as_deref(), Some(message));
            });
        }

        view.update(cx, |host, cx| {
            assert!(host.warning.refresh(None));
            cx.notify();
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert!(cx.debug_bounds("configuration-save-warning").is_none());
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Measured before anything else: the terminal page derives its pane width
        // from this, and a frame that forgot to write it would lay the panes out
        // against last frame's window.
        //
        // Not divided by the scale factor: `bounds()` already reports logical
        // pixels on every platform this runs on, and the division shrank the
        // width by exactly the scale factor on a 150% display — which is where
        // the old "the canvas reports 1224 but the arithmetic says 613" mystery
        // came from. The canvas and the events live in the same logical units
        // this writes; keep them that way.
        self.window_width.set(f32::from(window.bounds().size.width));

        // The active page's entity is built the first frame it is shown, and kept
        // for the rest of the window's life. Before the drains, because a drain
        // may read the page it just created.
        self.ensure_page(window, cx);

        // First thing in the frame, because this is the first point in it where a
        // `Context` exists to read the pages with — a click that arrived during the
        // previous frame's input pass is carried out here.
        self.drain_terminal(window, cx);
        self.drain_active_page(window, cx);
        self.drain_panels(window, cx);
        self.drain_transfers(cx);
        self.drain_quick_manager(window, cx);
        self.drain_quick_connect(window, cx);
        self.drain_commands(window, cx);
        self.drain_tunnels(cx);
        self.drain_opened_file(window, cx);
        self.drain_editor(window, cx);
        self.drain_group_manager(window, cx);
        self.drain_rule_editor(window, cx);
        self.drain_approval_queue(window, cx);
        // A synchronous save can fail during a drain. Display it in this frame
        // without closing or recreating the editor that still owns the draft.
        self.refresh_persistence_warning();

        let background = cx.theme().background;

        // The dialog layer needs the extension trait in scope for `open_dialog` /
        // `close_dialog`; the subscription is what makes this view repaint when the
        // queue behind the layer changes.
        if self._root_subscription.is_none() {
            self._root_subscription = super::follow_root(window, cx);
        }
        let sheet_layer = gpui_kit::component::Root::render_sheet_layer(window, cx);
        let dialog_layer = gpui_kit::component::Root::render_dialog_layer(window, cx);

        let body_element = self.render_body(cx);
        let status_bar = self.render_status_bar(cx);

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(background)
            .child(body_element)
            .children(self.persistence_warning.render(cx))
            .child(status_bar)
            // Last, so a dialog covers the window.
            .children(sheet_layer)
            .children(dialog_layer)
    }
}

impl Shell {
    fn refresh_persistence_warning(&mut self) -> bool {
        let error = self.state.store.borrow().persistence_error();
        self.persistence_warning.refresh(error)
    }

    /// Start the approval poll: once a second, sweep the approval queue.
    /// Three jobs in one pass — new pending requests join the dialog queue,
    /// finished requests (the human answered, or a timer did) are reported
    /// once on the status line and removed, and the audit journal's retention
    /// sweep runs when the day turns. The same tick observes background-save
    /// failures without a second timer or a busy repaint loop. Started once,
    /// at construction.
    fn start_approval_poll(&mut self, cx: &mut Context<Self>) {
        self.approval_poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(1))
                .await;
            // The sweep is filesystem work, so it belongs on the background
            // executor — this task runs outside any tokio runtime, which is
            // what the main window's process is.
            let requests = cx
                .background_executor()
                .spawn(async {
                    let dir = crate::automation::approval::queue_dir();
                    crate::automation::approval::scan_in(&dir)
                })
                .await;
            let alive = this.update(cx, |shell, cx| {
                if shell.refresh_persistence_warning() {
                    cx.notify();
                }
                let queue_root = crate::automation::approval::queue_dir();
                for request in requests {
                    match request.status.as_str() {
                        "pending" => {
                            if !shell.approval_shown.contains(&request.id)
                                && !shell
                                    .approval_queue
                                    .iter()
                                    .any(|q| q.id == request.id)
                            {
                                shell.approval_queue.push(request);
                            }
                        }
                        "approved" | "denied" => {
                            // One line on the status line per finished
                            // decision, then the file goes — the asking
                            // process has long since read its answer.
                            if shell.approval_reported.insert(request.id.clone()) {
                                let outcome = if request.status == "approved" {
                                    crate::i18n::t("已批准", "Approved")
                                } else {
                                    crate::i18n::t("已拒绝", "Denied")
                                };
                                shell.say(
                                    format!(
                                        "{}: {} — {}",
                                        outcome,
                                        request.command,
                                        crate::i18n::t(
                                            "已记入审批记录",
                                            "recorded in the audit log"
                                        )
                                    ),
                                    cx,
                                );
                            }
                            if let Some(path) = &request.path {
                                let _ = std::fs::remove_file(path);
                            }
                            //  keeps the id: the dialog may
                            // still be on screen for a frame or two, and a
                            // re-queue after the feedback pass would reopen it
                            // over the very answer it is showing.
                        }
                        _ => {}
                    }
                }
                // A dialog with a live countdown repaints once a second, so
                // the deny button's timer actually counts down.
                if !shell.approval_opened_at.is_empty() {
                    cx.notify();
                }
                // The audit sweep runs when the day turns — a daily job has no
                // business running on every tick.
                let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                if shell.audit_last_prune_day.as_deref() != Some(today.as_str()) {
                    let retention = shell.state.store.borrow().mcp_audit_retention_days();
                    crate::automation::approval::prune_audit_in(&queue_root, retention);
                    shell.audit_last_prune_day = Some(today);
                }
            });
            if alive.is_err() {
                break;
            }
        }));
    }

    /// Show the head of the approval queue as a dialog, if the frame is ready
    /// for one. Called from render, which has the window the dialog needs.
    ///
    /// Built to be read: the session and the command each get their own
    /// labelled row, the command in the mono family on its own block, the
    /// risk reasons as a list. The deny button carries the countdown — the
    /// number is the promise the timeout makes, visible where the action is.
    fn drain_approval_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // One dialog per request, ever: the first frame this request is at
        // the head of the queue marks it shown and opens the dialog, and the
        // frames after that skip it. Without the mark, a dialog opened from
        // render would be re-opened by the next frame — a stack of dialogs,
        // each new one blocking the clicks aimed at the last.
        let Some(request) = self
            .approval_queue
            .iter()
            .find(|request| !self.approval_shown.contains(&request.id))
            .cloned()
        else {
            return;
        };

        // The UI-side decision deadline: if the human has not answered within
        // the request's own wait window, the window denies it automatically —
        // the same fail-closed answer the MCP side would reach, just sooner
        // and with a message the user actually sees.
        let opened = self
            .approval_opened_at
            .entry(request.id.clone())
            .or_insert_with(std::time::Instant::now);
        if opened.elapsed().as_secs() >= request.wait_timeout_secs {
            self.approval_opened_at.remove(&request.id);
            self.approval_shown.insert(request.id.clone());
            self.approval_queue.retain(|q| q.id != request.id);
            crate::automation::approval::resolve_timeout_in(
                &crate::automation::approval::queue_dir(),
                &request.id,
            );
            // The status line is the receipt the user actually sees — the
            // audit journal keeps the durable copy.
            self.say(
                format!(
                    "{}: {}",
                    crate::i18n::t("已自动拒绝（超时）", "Auto-denied (timeout)"),
                    request.command
                ),
                cx,
            );
            cx.notify();
            return;
        }

        self.approval_shown.insert(request.id.clone());
        self.approval_queue.retain(|queued| queued.id != request.id);

        // The dialog body is rebuilt every frame — which is what makes the
        // deny button's countdown tick: the label is computed from the
        // request's own deadline each time the builder runs.
        let request_for_body = request.clone();
        let reasons_for_body = request.reasons.clone();
        let request_for_countdown = request.clone();
        let id_for_ok = request.id.clone();
        let id_for_cancel = request.id.clone();

        crate::ui::dialogs::approval_dialog(
            window,
            cx,
            request_for_body,
            reasons_for_body,
            move || {
                let left = crate::automation::approval::seconds_left(&request_for_countdown);
                if left > 0 {
                    SharedString::from(format!(
                        "{} ({}s)",
                        crate::i18n::t("拒绝", "Deny"),
                        left
                    ))
                } else {
                    SharedString::from(crate::i18n::t("拒绝", "Deny"))
                }
            },
            move |_, _| {
                crate::automation::approval::resolve_manual(&id_for_ok, true);
                true
            },
            move |_, _| {
                crate::automation::approval::resolve_manual(&id_for_cancel, false);
                true
            },
        );
    }

    /// Say something on the status line: readable for a few seconds, then it
    /// fades out and the line goes quiet again.
    ///
    /// Every writer goes through here rather than assigning the field, because a
    /// note that outlives its moment reads as current — the exact failure the
    /// field's own doc comment warns about, which the plain assignment could
    /// never fix.
    fn say(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.say_inner(text.into(), false, cx);
    }

    /// Say something that stays until the next note replaces it — an operation
    /// in flight, whose completion has not landed yet.
    fn say_sticky(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.say_inner(text.into(), true, cx);
    }

    fn say_inner(&mut self, text: String, sticky: bool, cx: &mut Context<Self>) {
        self.status_epoch += 1;
        let at = std::time::Instant::now();
        self.status = Some(StatusNote {
            text,
            at,
            sticky,
            fading: false,
        });
        if sticky {
            // No schedule of its own: the next `say` replaces it, and replacing
            // this field's task drops whatever schedule the old note had.
            self.status_expiry = None;
            cx.notify();
            return;
        }
        self.status_expiry = Some(cx.spawn(async move |this, cx| {
            // First leg: run the fade. Guarded by `at` so a note that was
            // replaced mid-flight is not faded by its predecessor's timer.
            cx.background_executor()
                .timer(STATUS_TTL.saturating_sub(STATUS_FADE))
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(note) = &mut this.status {
                    if note.at == at {
                        note.fading = true;
                        cx.notify();
                    }
                }
            });
            // Second leg: the fade has run; retire the line.
            cx.background_executor().timer(STATUS_FADE).await;
            let _ = this.update(cx, |this, cx| {
                if this.status.as_ref().is_some_and(|note| note.at == at) {
                    this.status = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    /// The rail, the page, and the two keystrokes the window keeps for itself.
    fn render_body(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.pages.render_active();
        let nav = super::nav::render_nav_rail(self.pages.active, cx);

        h_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            // The root names the window's key context, so the keymap's window-scope
            // bindings (`Shell && …`) resolve here, and the actions land on handlers
            // instead of on keystroke strings: see `impls/actions.rs`, which is the
            // one place a chord is written down.
            .key_context("Shell")
            .on_action(cx.listener(|this, _: &crate::ui::CommandPalette, window, cx| {
                if !matches!(this.overlay, Overlay::None) {
                    return;
                }
                this.open_command_palette(window, cx);
            }))
            .on_action(cx.listener(
                |this, _: &crate::ui::QuickConnect, window, cx| {
                    // The dialog context's NoAction already keeps Ctrl+K out of the
                    // modal layer; this guard covers the overlays that render without
                    // that context (the session editor, the file viewer).
                    if !matches!(this.overlay, Overlay::None) {
                        return;
                    }
                    this.open_quick_connect(window, cx);
                },
            ))
            .on_action(cx.listener(
                |this, _: &crate::ui::Reconnect, _window, cx| {
                    this.reconnect_ended_session(cx);
                },
            ))
            .on_action(cx.listener(|this, _: &crate::ui::NextTab, _, cx| {
                this.pages
                    .terminal
                    .update(cx, |page, cx| page.cycle_tab(false, cx));
            }))
            .on_action(cx.listener(|this, _: &crate::ui::PrevTab, _, cx| {
                this.pages
                    .terminal
                    .update(cx, |page, cx| page.cycle_tab(true, cx));
            }))
            .on_action(cx.listener(|this, _: &crate::ui::CloseTab, _, cx| {
                this.pages.terminal.update(cx, |page, cx| {
                    if let Some(id) = page.active_tab_id() {
                        page.close_tab(&id, cx);
                    }
                });
            }))
            .on_action(cx.listener(|this, _: &crate::ui::SplitRight, _, cx| {
                this.pages
                    .terminal
                    .update(cx, |page, cx| page.split_active_tab(false, cx));
            }))
            .on_action(cx.listener(|this, _: &crate::ui::SplitDown, _, cx| {
                this.pages
                    .terminal
                    .update(cx, |page, cx| page.split_active_tab(true, cx));
            }))
            .on_action(cx.listener(|this, _: &crate::ui::CyclePane, _, cx| {
                this.pages
                    .terminal
                    .update(cx, |page, cx| page.cycle_pane(cx));
            }))
            .child(nav)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(page),
            )
    }

    /// The status line: what the window has to say, or nothing.
    ///
    /// Deliberately *not* "Ready" or the last thing that happened. A status line that
    /// keeps announcing a close from three actions ago is worse than an empty one,
    /// because it is read as current. What belongs here is a fact about now — and the
    /// session's own state has a better home on its tab, where the dot is.
    fn render_status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;
        // An expired note is skipped here as well as retired by its task: render
        // has no `&mut`, so a frame that lands between the two legs of the timer
        // would otherwise draw a note whose fade has already finished.
        let note = self
            .status
            .as_ref()
            .filter(|note| note.sticky || note.at.elapsed() < STATUS_TTL);
        // No note, no bar: an empty strip above nothing read as a rendering
        // bug. The row only exists while there is a fact to put in it, which
        // also keeps the workspace one line taller the rest of the time.
        let Some(note) = note else {
            return div();
        };
        let mut bar = h_flex()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(border)
            .text_xs()
            .text_color(muted_fg);
        {
            let text = SharedString::from(note.text.clone());
            if note.fading {
                bar = bar.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .with_animation(
                            SharedString::from(format!("status-fade-{}", self.status_epoch)),
                            Animation::new(STATUS_FADE),
                            |line, delta| line.opacity(1.0 - delta),
                        )
                        .child(text),
                );
            } else {
                bar = bar.child(div().min_w_0().truncate().child(text));
            }
        }
        bar
    }

    // ------------------------------------------------------------------
    // Pages.
    // ------------------------------------------------------------------

    /// Bring the active page's entity into existence, the first frame it is
    /// shown. Each page also gets its one-time sync here — the active tab it
    /// should describe on arrival — because the page was not around when the
    /// tab changed.
    fn ensure_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.pages.active {
            PageId::Terminal => {}
            PageId::Sessions => {
                if self.pages.sessions.is_none() {
                    let store = self.state.store.clone();
                    self.pages.sessions = Some(cx.new(|cx| SessionsPage::new(store, window, cx)));
                    // The list highlights the session showing; it may already be
                    // one, because the terminal page opened a tab before this page
                    // was ever visited.
                    let tab = self.pages.terminal.read(cx).active_tab_id();
                    if let Some(page) = self.pages.sessions.clone() {
                        page.update(cx, |page, cx| page.set_active(tab, cx));
                    }
                }
            }
            PageId::Settings => {
                if self.pages.settings.is_none() {
                    let store = self.state.store.clone();
                    self.pages.settings = Some(cx.new(|cx| SettingsPage::new(store, cx)));
                }
            }
        }
    }

    /// Switch pages. The page just left keeps its entity — nothing to tear down,
    /// which is the whole point; the page being entered is built if this is its
    /// first visit.
    pub(crate) fn open_page(&mut self, id: PageId, window: &mut Window, cx: &mut Context<Self>) {
        self.pages.active = id;
        self.ensure_page(window, cx);
        cx.notify();
    }

    /// Open `session_id` under `tab_id`, and connect it. The one connect path in
    /// the window: a list row, a duplicate, a reconnect all arrive here. Takes
    /// `&mut App` because the palette's Confirm subscription holds an `App`, and
    /// every other caller reaches it through a `Context` deref.
    fn connect(&mut self, tab_id: &str, session_id: &str, cx: &mut gpui_kit::App) {
        self.pages.terminal.update(cx, |page, cx| {
            // Same id on both sides is "show me this session": focusing an open
            // tab beats stacking a second connection to the same host.
            if tab_id == session_id {
                page.activate_or_open(session_id, cx)
            } else {
                page.open_session_tab(tab_id, session_id, cx)
            }
        });
    }

    /// Point every follower that describes "the session showing" — the detached
    /// windows and the session list's highlight — at the tab the terminal page
    /// just made active.
    ///
    /// One method rather than calls at each site, because a site that remembered
    /// one follower and forgot another would leave a view describing the session
    /// the user just left.
    fn on_tab_change(&mut self, tab: Option<String>, cx: &mut Context<Self>) {
        self.process_window.follow(tab.clone(), cx);
        self.system_info_window.follow(tab.clone(), cx);

        if let Some(page) = self.pages.sessions.clone() {
            page.update(cx, |page, cx| page.set_active(tab, cx));
        }
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Drains.
    // ------------------------------------------------------------------

    /// Perform whatever the terminal page asked for since the last frame.
    fn drain_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        loop {
            let Some(action) = self.pages.terminal.read(cx).take_action() else {
                return;
            };
            match action {
                TerminalAction::Connect { tab_id, session_id } => {
                    self.connect(&tab_id, &session_id, cx)
                }
                TerminalAction::ActiveTabChanged(tab) => self.on_tab_change(tab, cx),
                TerminalAction::OpenQuickManager => {
                    // A form being filled in: a fresh dialog each time, so a
                    // cancelled one leaves no half-filled draft behind.
                    let store = self.state.store.clone();
                    let manager = cx.new(|cx| QuickManagerView::new(store, window, cx));
                    self.overlay = Overlay::QuickManager(manager.clone());
                    Self::overlay_dialog(
                        manager,
                        crate::i18n::t("快速命令", "Quick commands").into(),
                        760.,
                        520.,
                        window,
                        cx,
                    );
                    cx.notify();
                }
                TerminalAction::OpenTunnels => {
                    // Built on demand against the active tab: the panel names the
                    // session it describes with the tab's own title.
                    let tab = self.pages.terminal.read(cx).active_tab_id();
                    let session = self.pages.terminal.read(cx).tab_title(tab.as_deref());
                    let state = self.state.clone();
                    let panel = cx.new(|cx| {
                        TunnelsView::new(
                            state.store.clone(),
                            tab,
                            session,
                            state.tunnels.clone(),
                            window,
                            cx,
                        )
                    });
                    self.overlay = Overlay::Tunnels(panel.clone());
                    Self::overlay_dialog(
                        panel,
                        crate::i18n::t("端口转发", "Port forwarding").into(),
                        720.,
                        480.,
                        window,
                        cx,
                    );
                    cx.notify();
                }
                TerminalAction::ShowProcesses => self.open_process_window(cx),
                TerminalAction::ShowSystemInfo => self.open_system_info_window(cx),
                TerminalAction::OpenQuickConnect => self.open_quick_connect(window, cx),
                TerminalAction::NewConnection => self.open_editor(None, window, cx),
                TerminalAction::ImportConfig => self.import_connections(cx),
            }
        }
    }

    /// Open the quick-connect palette: a searchable list over every saved
    /// session and built-in shell, as a light dialog over whatever is on
    /// screen. Confirming connects in place and brings the terminal page
    /// forward — the hot path for opening another session, without a detour
    /// through the connections page.
    /// The Reconnect action's handler: bring back the active session if it ended.
    ///
    /// Read from the status map rather than from the view: state 2 is what a session
    /// that has ended leaves behind, and it is the signal this window has already been
    /// seen to act on. The overlay guard repeats the keymap's `!Dialog` intent, because
    /// not every overlay is rendered with the dialog context.
    pub(crate) fn reconnect_ended_session(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        let Some(id) = self.pages.terminal.read(cx).active_tab_id() else {
            return;
        };
        let ended = self
            .state
            .statuses
            .lock()
            .map(|map| map.get(&id).map(|status| status.state == 2).unwrap_or(false))
            .unwrap_or(false);
        if ended {
            self.connect(&id, &id, cx);
        }
    }

    pub(crate) fn open_quick_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.state.store.clone();
        let active = self.pages.terminal.read(cx).active_tab_id();
        // Compact: rows only. The header chrome this view normally draws is
        // the connections page's furniture; the palette is a question the
        // shell asks, and the hint bar below the list is the answer to
        // "what do I do now".
        let list = cx.new(|cx| SessionListView::new_compact(store, active, window, cx));
        // A confirm — a click on the row, or Enter on the selection — records
        // the pick; the drain at the top of the next frame, which has a window
        // and a mutable context, connects and takes the dialog down. The
        // event lives on the list widget's own state, not on the view, so the
        // subscription follows the state — the same split the connections page
        // makes. The closure cannot borrow the list it serves, so a clone
        // resolves the index.
        let list_for_events = list.clone();
        let list_state = list.read(cx).list().clone();
        self._quick_connect_subscription = Some(cx.subscribe_in(
            &list_state,
            window,
            move |this: &mut Shell,
                  _state,
                  event: &gpui_kit::component::list::ListEvent,
                  _window,
                  cx| {
                let gpui_kit::component::list::ListEvent::Confirm(ix) = event else {
                    return;
                };
                let Some(session_id) = list_for_events.read(cx).session_at(*ix, cx) else {
                    // A group heading, not a session. Nothing to open.
                    return;
                };
                this.quick_connect_pick = Some(session_id);
            },
        ));
        self.overlay = Overlay::QuickConnect(list.clone());
        let list_for_focus = list.clone();
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        // Centred like every other dialog: the toolkit parks dialogs at a fixed
        // distance from the top, and a palette this tall reads as detached, so
        // the margin is computed from the nominal height instead.
        let margin_top =
            ((f32::from(window.bounds().size.height) - (440. + 104.)) / 2.0).max(24.0);
        window.open_dialog(cx, move |dialog, _window, _cx| {
            dialog
                .title(crate::i18n::t("快速连接", "Quick connect"))
                .width(px(420.))
                .margin_top(px(margin_top))
                .close_button(true)
                // Click-away dismisses: a palette is a question asked of the
                // window, not a form being filled in, and Escape or a click
                // outside should both be "never mind".
                .overlay_closable(true)
                .child(
                    v_flex()
                        .w_full()
                        .h(px(440.))
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .overflow_hidden()
                                .child(list.clone().into_any_element()),
                        )
                        // One hint line instead of chrome: what the stripped
                        // header would have said, in the space the rows use.
                        .child(
                            h_flex()
                                .w_full()
                                .flex_shrink_0()
                                .justify_center()
                                .gap_2()
                                .px_3()
                                .py_1p5()
                                .border_t_1()
                                .border_color(gpui_kit::rgb(0x00000014))
                                .text_xs()
                                .text_color(gpui_kit::rgb(0x8a8a8a))
                                .child(SharedString::from(crate::i18n::t(
                                    "↑↓ 选择 · Enter 连接 · Esc 关闭",
                                    "↑↓ select · Enter connect · Esc dismiss",
                                ))),
                        ),
                )
        });
        // The caret goes straight into the search box: Ctrl+K, a few letters,
        // Enter — hands never leave the keyboard. Focused *after* the dialog is
        // queued, because the layer's own open takes the focus with it.
        list_for_focus.update(cx, |view, cx| view.focus_search(window, cx));
        cx.notify();
    }

    /// The commands the palette offers, in the order they are shown. Each name
    /// is the verb a menu would carry; the dispatch lives in `run_command`.

    /// Carry the palette's confirm out — connect in place, terminal page
    /// forward, dialog down — or take the palette down if it went away without
    /// one (a click outside, or Escape), dropping the subscription with it.
    fn drain_quick_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Overlay::QuickConnect(_)) {
            return;
        }
        if let Some(session_id) = self.quick_connect_pick.take() {
            self.overlay = Overlay::None;
            self._quick_connect_subscription = None;
            window.close_dialog(cx);
            self.pages.active = PageId::Terminal;
            self.connect(&session_id, &session_id, cx);
            cx.notify();
            return;
        }
        if self.dialog_dismissed(window, cx) {
            self.overlay = Overlay::None;
            self._quick_connect_subscription = None;
        }
    }

    /// Perform whatever the active page asked for since the last frame. A hidden
    /// page's queue is not drained — it is not rendered, and an action it cannot
    /// have received this frame should not leap the moment it is shown.
    fn drain_active_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.pages.active {
            PageId::Terminal => {}
            PageId::Sessions => self.drain_sessions(window, cx),
            PageId::Settings => self.drain_settings(window, cx),
        }
    }

    /// Perform whatever the session list asked for since the last frame.
    fn drain_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.pages.sessions.clone() else {
            return;
        };
        while let Some(action) = page.update(cx, |page, cx| page.take_action(cx)) {
            match action {
                SessionsAction::Open(id) => {
                    // A row click is a page switch as much as a connect: the session
                    // opens in the terminal page, and that page is where the answer
                    // to "what happened" shows.
                    self.open_page(PageId::Terminal, window, cx);
                    self.connect(&id, &id, cx);
                }
                SessionsAction::NewSession => self.open_editor(None, window, cx),
                SessionsAction::Edit(id) => {
                    let session = self.state.store.borrow().get(&id).cloned();
                    match session {
                        Some(session) => self.open_editor(Some(session), window, cx),
                        None => tracing::warn!(
                            "the list offered session {id} to edit, which is not saved"
                        ),
                    }
                }
                SessionsAction::Duplicate(id) => self.duplicate_session(&id, window, cx),
                SessionsAction::Delete(id) => self.delete_session(&id, window, cx),
                SessionsAction::Move { id, group } => self.move_session(&id, &group, cx),
                SessionsAction::ImportConfig => self.import_connections(cx),
                SessionsAction::ExportConfig => self.export_sessions(cx),
                SessionsAction::ManageGroups => {
                    // The dialog rather than a name prompt: it can create, rename and
                    // delete, and a group is easier to manage where all of it is visible
                    // at once. It is also the only surface for them, because a group's
                    // own heading turned out not to be a clickable target in this list.
                    let store = self.state.store.clone();
                    let manager = cx.new(|cx| GroupManagerView::new(store, window, cx));
                    self.overlay = Overlay::Groups(manager.clone());
                    Self::overlay_dialog(
                        manager,
                        crate::i18n::t("会话分组", "Session groups").into(),
                        620.,
                        520.,
                        window,
                        cx,
                    );
                    cx.notify();
                }
            }
        }
    }

    /// Carry out whatever the tunnel dialog asked for.
    ///
    /// The dialog owns no session, so starting and stopping tunnels is reported
    /// rather than done: the shell holds the handles, and a tunnel belongs to the
    /// session that opened it — closing the tab takes it down with the connection.
    fn drain_tunnels(&mut self, cx: &mut Context<Self>) {
        let Overlay::Tunnels(panel) = self.overlay.clone() else {
            return;
        };
        let Some(tab_id) = self.pages.terminal.read(cx).active_tab_id() else {
            return;
        };
        while let Some(action) = panel.update(cx, |view, _| view.take_action()) {
            let command = match action {
                crate::ui::TunnelAction::Start(forward) => {
                    SessionCommand::AddTunnel {
                        // The runtime id is the SSH layer's key for stopping it again; a
                        // timestamp keeps two rules with the same ports apart, which the
                        // ports themselves cannot.
                        id: format!(
                            "tunnel-{}",
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|elapsed| elapsed.as_millis())
                                .unwrap_or_default()
                        ),
                        forward,
                    }
                }
                crate::ui::TunnelAction::Stop(id) => SessionCommand::StopTunnel(id),
            };
            if let Some(handle) = self.state.handles.borrow().get(&tab_id) {
                let _ = handle.commands.send(command);
            }
        }
    }

    /// Perform whatever the quick-command manager asked for since the last frame.
    ///
    /// The manager owns the store and writes it itself; the only thing to carry out
    /// is the consequence: the dock's rows have to be rebuilt from what was saved.
    fn drain_quick_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::QuickManager(manager) = self.overlay.clone() else {
            return;
        };
        // The toolkit's × closes the card, and a dismissal is not an action the view
        // reports — Saved is the only one it has. The layer is what says it is gone.
        if self.dialog_dismissed(window, cx) {
            self.overlay = Overlay::None;
            return;
        }
        while let Some(action) = manager.update(cx, |view, _| view.take_action()) {
            match action {
                QuickManagerAction::Saved => {
                    self.pages
                        .terminal
                        .update(cx, |page, cx| page.refresh_quick(cx));
                }
            }
            cx.notify();
        }
    }


    /// Perform whatever the settings page asked for since the last frame.
    ///
    /// The page records intent rather than opening the editor itself, because the
    /// editor is a dialog and this is what owns the dialog slot.
    fn drain_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.pages.settings.clone() else {
            return;
        };
        while let Some(action) = page.update(cx, |page, cx| page.take_action(cx)) {
            match action {
                // The window theme: what the user chose, or what the system is set to. The
                // terminal is unaffected either way — its own palette is dark and is set
                // where the view is built — so this is only ever about the frame.
                SettingsAction::ThemeChanged(pref) => {
                    match pref.as_str() {
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
                SettingsAction::LanguageChanged => {
                    // Rebuilt, not merely repainted: the toolkit bakes its own placeholders in
                    // when a widget is constructed, so this page's search box would keep the
                    // language it was built in until the page was made again. A new entity is
                    // the whole fix.
                    let store = self.state.store.clone();
                    page.update(cx, |page, cx| page.rebuild(store, cx));
                    window.refresh();
                    cx.notify();
                }
                SettingsAction::NewHighlightRule => {
                    let store = self.state.store.clone();
                    let editor = cx.new(|cx| RuleEditorView::new(store, window, cx));
                    self.overlay = Overlay::RuleEditor(editor.clone());
                    Self::overlay_dialog(
                        editor,
                        crate::i18n::t("输出高亮规则", "Highlight rule").into(),
                        620.,
                        480.,
                        window,
                        cx,
                    );
                    cx.notify();
                }
                // The connections page's two actions: the same doors the session list's
                // menu used to hold, now reached from Settings.
                SettingsAction::ImportSshConfig => self.import_ssh_config(cx),
                SettingsAction::ExportSessions => self.export_sessions(cx),
                // The third way in: a list someone pasted. The shell does the adding for
                // the same reason it does the other two — it owns the store and the status
                // line the count goes on.
                SettingsAction::ImportPasted(text) => {
                    let count = crate::core::batch::parse(&text).len();
                    let outcome = if count == 0 {
                        crate::i18n::t(
                            "没有可导入的连接：每行至少要有主机名。",
                            "Nothing to import: each line needs at least a host.",
                        )
                        .to_string()
                    } else {
                        {
                            let mut store = self.state.store.borrow_mut();
                            for session in crate::core::batch::parse(&text) {
                                store.upsert(session);
                            }
                            if let Err(error) = store.save() {
                                tracing::warn!("could not save the imported sessions: {error:#}");
                            }
                        }
                        format!("{} {count}", crate::i18n::t("已导入连接", "imported"))
                    };
                    self.say(outcome, cx);
                }
                SettingsAction::WebdavUpload => self.webdav_sync(true, cx),
                SettingsAction::WebdavDownload => self.webdav_sync(false, cx),
                SettingsAction::ImportFont => self.import_fonts(cx),
                SettingsAction::OpenFontFolder => self.open_font_folder(cx),
                SettingsAction::OpenAuditFolder => self.open_audit_viewer(cx),
            }
        }
    }

    /// Pick font files, copy them into the import directory, and register them
    /// with the text system for the rest of this launch.
    ///
    /// The dialog runs off the UI thread like every other rfd dialog here; the
    /// copies and the registration land back on this thread, where the data
    /// directory and the text system live. The settings page's picker rebuilds
    /// its options every frame it renders, so notifying it is all a new import
    /// needs to appear in the list.
    fn import_fonts(&mut self, cx: &mut Context<Self>) {
        let task = rfd::AsyncFileDialog::new()
            .add_filter(
                "字体 / Fonts",
                &["ttf", "otf", "ttc", "otc"],
            )
            .pick_files();
        // Detached: dropping the Task cancels it, and a dropped handle at the
        // end of this function would cancel the dialog before it was shown.
        cx.spawn(async move |this, cx| {
            let Some(picked) = task.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                let mut families: Vec<String> = Vec::new();
                let mut failed = 0u32;
                for handle in picked {
                    match crate::core::fonts::import_font(handle.path()) {
                        Ok(family) => {
                            if !families.contains(&family) {
                                families.push(family);
                            }
                            // Registered now, so the import is usable without a
                            // restart; the startup scan covers later launches.
                            match std::fs::read(
                                crate::core::fonts::imported_fonts_dir().join(
                                    handle.file_name(),
                                ),
                            ) {
                                Ok(bytes) => {
                                    if let Err(error) = cx
                                        .text_system()
                                        .add_fonts(vec![std::borrow::Cow::Owned(bytes)])
                                    {
                                        tracing::warn!(
                                            "could not register imported font: {error:#}"
                                        );
                                    }
                                }
                                Err(error) => {
                                    tracing::warn!("could not read the imported font: {error:#}");
                                }
                            }
                        }
                        Err(error) => {
                            tracing::warn!("could not import font: {error:#}");
                            failed += 1;
                        }
                    }
                }
                if families.is_empty() {
                    shell.say(
                        crate::i18n::t("没有可导入的字体文件", "No font files could be imported"),
                        cx,
                    );
                    return;
                }
                if failed > 0 {
                    shell.say(
                        format!(
                            "{} ({} {})",
                            crate::i18n::t("已导入", "imported"),
                            failed,
                            crate::i18n::t("个文件失败", "file(s) failed")
                        ),
                        cx,
                    );
                } else {
                    shell.say(
                        format!(
                            "{}: {}",
                            crate::i18n::t("已导入字体", "imported fonts"),
                            families.join(", ")
                        ),
                        cx,
                    );
                }
                if let Some(page) = shell.pages.settings.clone() {
                    page.update(cx, |_, cx| cx.notify());
                }
            });
        })
        .detach();
    }

    /// Open the approval-audit viewer: a dedicated, read-only window over
    /// the journal. The raw day files stay reachable from its toolbar.
    pub(crate) fn open_audit_viewer(&mut self, cx: &mut Context<Self>) {
        self.audit_window.show(cx);
    }

    /// Open the font import folder in the OS file manager, best effort — a
    /// machine with no file manager is not the app's problem and not worth a
    /// dialog.
    fn open_font_folder(&mut self, _cx: &mut Context<Self>) {
        let dir = crate::core::fonts::imported_fonts_dir();
        let _ = std::fs::create_dir_all(&dir);
        let opener = if cfg!(target_os = "windows") {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener)
            .arg(&dir)
            .spawn();
    }

    /// Perform whatever the terminal page's file dock asked for since the last
    /// frame.
    fn drain_panels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dock = self.pages.terminal.read(cx).dock_panel().clone();
        // Session-side SFTP events write the shared store, which the panel
        // holds a *copy* of — nothing in the event path can reach the panel
        // directly. The store's listing generation is what bridges the gap:
        // when it moves past what the panel recorded at its last sync, the
        // panel is re-pointed here, on this frame, and the new entries (or the
        // spinner-ending error) land the same beat they arrived.
        if let Some(tab) = self.pages.terminal.read(cx).active_tab_id() {
            let generation = self.state.listing_generation(&tab);
            if dock.read(cx).synced_generation(&tab) != Some(generation) {
                self.pages.terminal.update(cx, |page, cx| page.refresh_dock(cx));
            }
        }
        self.drain_panel(window, &dock, cx);
    }

    /// Perform whatever one SFTP panel asked for since the last frame.
    ///
    /// Same shape as the other drains, and the reason is the same: the panel does
    /// not own a session handle, so it reports the request and the shell — which
    /// holds the handles — carries it out, against the active tab.
    fn drain_panel(
        &mut self,
        window: &mut Window,
        panel: &Entity<SftpPanelView>,
        cx: &mut Context<Self>,
    ) {
        use crate::core::SftpListing;

        let mut touched_listing = false;

        // The panel shows the active tab's listing and its buttons act on that tab's
        // session, which is the whole reason the panel belongs to a session rather
        // than to the window.
        let Some(tab_id) = self.pages.terminal.read(cx).active_tab_id() else {
            return;
        };

        while let Some(action) = panel.update(cx, |panel, _| panel.take_action()) {
            // The session, if there is one. A panel with no session still shows the
            // stale listing, and its buttons do nothing rather than panicking.
            let handle = self
                .state
                .sftp_handles
                .lock()
                .ok()
                .and_then(|map| map.get(&tab_id).map(|h| h.commands.clone()));

            match action {
                PanelAction::Collapse => {
                    self.pages.terminal.update(cx, |page, cx| {
                        page.set_sftp_collapsed(true, cx);
                    });
                }
                PanelAction::Navigate(path) => {
                    if let Some(commands) = &handle {
                        let _ = commands.send(crate::sftp::SftpCommand::ListDir(path.clone()));
                    }
                    panel.update(cx, |panel, cx| panel.set_path(path, cx));
                }
                // The desktop's own editor, on a copy in a temporary directory: the
                // session downloads it, opens it, watches it, and uploads it back when it
                // is saved. The panel's whole part is naming the file.
                PanelAction::OpenTemp(remote) => {
                    if let Some(commands) = &handle {
                        let _ = commands
                            .send(crate::sftp::SftpCommand::OpenTemp { remote, edit: true });
                    }
                }
                // The same command with the watching turned off: the file is copied out and
                // handed to the desktop, and saving it there does not send it back.
                PanelAction::View(remote) => {
                    if let Some(commands) = &handle {
                        let _ = commands.send(crate::sftp::SftpCommand::ReadText {
                            remote,
                            edit: false,
                        });
                    }
                }
                PanelAction::Edit(remote) => {
                    if let Some(commands) = &handle {
                        let _ = commands
                            .send(crate::sftp::SftpCommand::ReadText { remote, edit: true });
                    }
                }
                PanelAction::OpenDefault(remote) => {
                    if let Some(commands) = &handle {
                        let _ = commands.send(crate::sftp::SftpCommand::OpenTemp {
                            remote,
                            edit: false,
                        });
                    }
                }
                // Cross-session copy: this session reads, the target session writes.
                // The destination directory is whatever the target is showing, which is
                // the only answer the user can predict.
                PanelAction::CopyTo { paths, target } => {
                    let target_dir = self.state.listing(&target).path().to_string();
                    // Both handles, because the copy is a method on the source session
                    // and takes the target command channel as its destination.
                    if let Ok(handles) = self.state.sftp_handles.lock() {
                        if let (Some(source), Some(destination)) =
                            (handles.get(&tab_id), handles.get(&target))
                        {
                            source.copy_to(paths, destination.commands.clone(), target_dir);
                        }
                    }
                }
                PanelAction::CopyPath(path) => {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path.clone()));
                    self.say(
                        format!("{} {path}", crate::i18n::t("已复制路径", "copied the path of")),
                        cx,
                    );
                }
                // The drag on the panel's toolbar: the same panel, the other edge. Written to
                // the config, because where the panel lives is a preference rather than a
                // property of this frame — the layout reads it back every frame.
                PanelAction::Redock { right } => {
                    {
                        let mut store = self.state.store.borrow_mut();
                        store.set_sftp_panel_dock(
                            if right { "right" } else { "bottom" }.to_string(),
                        );
                        if let Err(error) = store.save() {
                            tracing::warn!("could not save the file panel edge: {error:#}");
                        }
                    }
                    cx.notify();
                }
                // The tree is the session's: it owns which directories are expanded, and
                // every toggle answers with the whole flattened list rather than a delta.
                PanelAction::TreeToggle(path) => {
                    if let Some(commands) = &handle {
                        let _ = commands.send(crate::sftp::SftpCommand::ToggleTreeNode(path));
                    }
                }
                PanelAction::Refresh => {
                    let path = panel.read(cx).path().to_string();
                    if let Some(commands) = &handle {
                        let _ = commands.send(crate::sftp::SftpCommand::RefreshDir(path));
                        // The reply is the same SftpEntries a navigate answers, but
                        // nothing here looks different until it lands — so the panel
                        // says the refresh is in flight, the way a navigate already
                        // does through `set_path`.
                        panel.update(cx, |panel, cx| panel.set_loading(cx));
                    }
                }
                // The four directory actions are one shape: ask for the thing a menu
                // cannot carry — a name, or a mode — and send the session the command it
                // already has. A handler that refuses empty input returns `false`, which
                // keeps the dialog open rather than closing it on nothing.
                PanelAction::MkDir => {
                    let dir = panel.read(cx).path().to_string();
                    let Some(commands) = handle.clone() else {
                        return;
                    };
                    super::dialogs::prompt(
                        window,
                        cx,
                        crate::i18n::t("新建文件夹", "New folder").into(),
                        crate::i18n::t("文件夹名称", "Folder name").into(),
                        crate::i18n::t("创建", "Create").into(),
                        move |name, _, _| {
                            let name = name.trim().to_string();
                            if name.is_empty() {
                                return false;
                            }
                            let _ = commands
                                .send(crate::sftp::SftpCommand::MkDir(join_remote(&dir, &name)));
                            true
                        },
                    );
                }
                PanelAction::TouchFile => {
                    let dir = panel.read(cx).path().to_string();
                    let Some(commands) = handle.clone() else {
                        return;
                    };
                    super::dialogs::prompt(
                        window,
                        cx,
                        crate::i18n::t("新建文件", "New file").into(),
                        crate::i18n::t("文件名称", "File name").into(),
                        crate::i18n::t("创建", "Create").into(),
                        move |name, _, _| {
                            let name = name.trim().to_string();
                            if name.is_empty() {
                                return false;
                            }
                            let _ = commands.send(crate::sftp::SftpCommand::TouchFile(
                                join_remote(&dir, &name),
                            ));
                            true
                        },
                    );
                }
                PanelAction::Rename(paths) => {
                    let Some(from) = paths.first().cloned() else {
                        return;
                    };
                    let Some(commands) = handle.clone() else {
                        return;
                    };
                    super::dialogs::prompt(
                        window,
                        cx,
                        crate::i18n::t("重命名", "Rename").into(),
                        crate::i18n::t("新名称", "New name").into(),
                        crate::i18n::t("重命名", "Rename").into(),
                        move |name, _, _| {
                            // A name with a slash in it is a move, which this is not: it
                            // would either fail at the server or land somewhere the user
                            // did not name.
                            let name = name.trim().to_string();
                            if name.is_empty() || name.contains('/') {
                                return false;
                            }
                            let parent = crate::core::parent_path(&from);
                            let to = join_remote(&parent, &name);
                            let _ = commands.send(crate::sftp::SftpCommand::Rename {
                                from: from.clone(),
                                to,
                            });
                            true
                        },
                    );
                }
                PanelAction::Chmod(paths) => {
                    let Some(commands) = handle.clone() else {
                        return;
                    };
                    super::dialogs::prompt(
                        window,
                        cx,
                        crate::i18n::t("修改权限", "Change permissions").into(),
                        crate::i18n::t("八进制权限，例如 644", "Octal mode, for example 644")
                            .into(),
                        crate::i18n::t("应用", "Apply").into(),
                        move |text, _, _| {
                            // Octal, and only octal: a mode typed as decimal would be
                            // applied as a different mode without saying so.
                            let Ok(mode) = u32::from_str_radix(text.trim(), 8) else {
                                return false;
                            };
                            for path in &paths {
                                let _ = commands.send(crate::sftp::SftpCommand::Chmod {
                                    path: path.clone(),
                                    mode,
                                });
                            }
                            true
                        },
                    );
                }
                PanelAction::ToggleRow(index) => {
                    if let Ok(mut map) = self.state.sftp_listings.lock() {
                        map.entry(tab_id.clone())
                            .or_insert_with(SftpListing::default)
                            .toggle_selected(index);
                    }
                    touched_listing = true;
                }
                PanelAction::Sort(column) => {
                    if let Ok(mut map) = self.state.sftp_listings.lock() {
                        map.entry(tab_id.clone())
                            .or_insert_with(SftpListing::default)
                            .advance_sort(column);
                    }
                    touched_listing = true;
                }
                PanelAction::Upload => {
                    // The picker runs off the UI thread (see `import_connections`
                    // for why a blocking one would take the window down), so the
                    // send happens in the continuation. The session picked is the
                    // one the user clicked on: its channel and directory are
                    // captured before the dialog opens.
                    let Some(commands) = handle.clone() else {
                        return;
                    };
                    let remote_dir = panel.read(cx).path().to_string();
                    cx.spawn(async move |this, cx| {
                        let Some(picked) = rfd::AsyncFileDialog::new().pick_file().await
                        else {
                            return;
                        };
                        let local = picked.path().to_path_buf();
                        this.update(cx, |_shell, _cx| {
                            let _ = commands.send(crate::sftp::SftpCommand::Upload {
                                local,
                                remote_dir,
                                cleanup_after: None,
                            });
                        })
                        .ok();
                    })
                    .detach();
                }
                PanelAction::Download(paths) => {
                    // The folder picker is async like the upload's (a blocking
                    // native dialog on the UI thread re-enters the frame and
                    // takes the window down); the rest of the flow runs in the
                    // continuation, against the session the user clicked on.
                    let Some(commands) = handle.clone() else {
                        return;
                    };
                    let remote_dir = panel.read(cx).path().to_string();
                    let (always_ask, preset) = {
                        let store = self.state.store.borrow();
                        (
                            store.download_always_ask(),
                            store.download_dir().to_string(),
                        )
                    };
                    cx.spawn(async move |this, cx| {
                        // The directory comes from the setting unless the user asked
                        // to be asked, which is the original's policy and the one the
                        // setting's name promises.
                        let dir = if !always_ask && !preset.is_empty() {
                            preset.clone()
                        } else {
                            let Some(picked) = rfd::AsyncFileDialog::new().pick_folder().await
                            else {
                                return;
                            };
                            let dir = picked.path().to_string_lossy().to_string();
                            // A directory the user just chose becomes the one the
                            // transfer panel's "open folder" goes to, so the setting
                            // is not a second place to look for something a picker
                            // already answered.
                            this.update(cx, |shell, _| {
                                let mut store = shell.state.store.borrow_mut();
                                store.set_download_dir(dir.clone());
                                if let Err(error) = store.save() {
                                    tracing::warn!(
                                        "could not save the download directory: {error:#}"
                                    );
                                }
                            })
                            .ok();
                            dir
                        };
                        // Kept inside `this.update` rather than run on the async path:
                        // the per-file conflict question is a blocking native dialog,
                        // and asking it from the background executor would take the
                        // frame down.
                        this.update(cx, |_, _| {
                            crate::sftp::start_download(paths, dir, remote_dir, commands)
                        })
                        .ok();
                    })
                    .detach();
                }
                PanelAction::Delete(paths) => {
                    if let Some(commands) = &handle {
                        for remote in paths {
                            let _ = commands.send(crate::sftp::SftpCommand::Delete(remote));
                        }
                    }
                }
            }
        }

        if touched_listing {
            // The ticked rows live in the shared listing store, and the dock has
            // to draw the new state.
            self.pages.terminal.update(cx, |page, cx| page.refresh_dock(cx));
            cx.notify();
        }
    }

    /// Perform whatever the transfer records popover asked for since the last
    /// frame.
    fn drain_transfers(&mut self, cx: &mut Context<Self>) {
        let strip = self.pages.terminal.read(cx).transfers().clone();
        while let Some(action) = strip.update(cx, |list, _| list.take_action()) {
            match action {
                TransferAction::Cancel(id) => {
                    // Broadcast to every session, because only the one that started this
                    // transfer has it registered — and the list does not know which that
                    // is. Every other handle ignores an id it does not know.
                    if let Ok(handles) = self.state.sftp_handles.lock() {
                        for handle in handles.values() {
                            handle.cancel_transfer(id.clone());
                        }
                    }
                }
                TransferAction::Clear => {
                    if let Ok(mut store) = self.state.transfers.lock() {
                        store.clear();
                    }
                    strip.update(cx, |list, cx| list.refresh(cx));
                }
                TransferAction::OpenFolder => {
                    let dir = self.state.store.borrow().download_dir().to_string();
                    if dir.is_empty() {
                        return;
                    }
                    // Best effort: a machine with no file manager is not the app's problem
                    // and not worth a dialog.
                    let opener = if cfg!(target_os = "windows") {
                        "explorer"
                    } else if cfg!(target_os = "macos") {
                        "open"
                    } else {
                        "xdg-open"
                    };
                    if let Err(error) = std::process::Command::new(opener).arg(&dir).spawn() {
                        tracing::warn!("could not open {dir}: {error:#}");
                    }
                }
            }
        }
    }

    /// Open the viewer for a file whose text the session has reported, and carry out what
    /// the open one asks for.
    ///
    /// The event says what to show; the shell is what can show it, because building a view
    /// needs a window and a session's handler has only the view it draws.
    fn drain_opened_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(viewer) = self.open_file.clone() {
            let action = viewer.update(cx, |viewer, cx| {
                let action = viewer.take_action();
                if action.is_some() {
                    cx.notify();
                }
                action
            });
            match action {
                Some(super::file_viewer::FileViewerAction::Close) => {
                    self.open_file = None;
                    self.overlay = Overlay::None;
                    window.close_dialog(cx);
                    cx.notify();
                }
                // Written the way every other file write is: the session owns the channel,
                // and the shell knows which tab the file belongs to.
                Some(super::file_viewer::FileViewerAction::Save { path, content }) => {
                    let sender = self
                        .pages
                        .terminal
                        .read(cx)
                        .active_tab_id()
                        .and_then(|id| {
                            self.state
                                .sftp_handles
                                .lock()
                                .ok()?
                                .get(&id)
                                .map(|handle| handle.commands.clone())
                        });
                    if let Some(sender) = sender {
                        let _ = sender.send(crate::sftp::SftpCommand::WriteText {
                            remote: path,
                            content,
                        });
                    }
                    cx.notify();
                }
                None => {}
            }
        }
        let Some(opened) = self
            .state
            .opened_file
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
        else {
            return;
        };
        let viewer = cx.new(|cx| {
            super::file_viewer::FileViewerView::new(
                opened.path,
                opened.name,
                opened.content,
                opened.editable,
                opened.error,
                window,
                cx,
            )
        });
        self.open_file = Some(viewer.clone());
        self.overlay = Overlay::File(viewer.clone());
        Self::overlay_dialog(
            viewer,
            crate::i18n::t("文件", "File").into(),
            760.,
            560.,
            window,
            cx,
        );
        cx.notify();
    }

    /// Close the editor if it has finished, and refresh the tree when it saved.
    ///
    /// Polled at the top of the frame rather than pushed, because the editor cannot
    /// reach the shell that owns it — the same reason the list reports through an
    /// action queue.
    fn drain_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Editor(editor) = &self.overlay else {
            return;
        };
        let outcome = editor.update(cx, |editor, _| editor.take_outcome());
        let Some(outcome) = outcome else {
            return;
        };
        self.overlay = Overlay::None;
        // Both, and in this order: the state is what the shell drains, the dialog is what
        // the user sees. Clearing one without the other leaves either an empty card or a
        // form with no way out of it.
        window.close_dialog(cx);
        if outcome == EditorOutcome::Saved {
            // The list is rebuilt from the store, because a session that was added or
            // renamed is a row that did not exist before or no longer says what it did.
            if let Some(page) = self.pages.sessions.clone() {
                page.update(cx, |page, cx| page.reload(cx));
            }
        }
        cx.notify();
    }

    /// Perform whatever the group manager asked for since the last frame.
    ///
    /// It owns the store and writes it itself, so the only thing to carry out is the
    /// consequence: the list showing those groups has to be rebuilt from what was saved.
    fn drain_group_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Groups(manager) = self.overlay.clone() else {
            return;
        };
        // The toolkit's × closes the card, and a dismissal is not an action the view
        // reports — Saved is the only one it has. The layer is what says it is gone.
        if self.dialog_dismissed(window, cx) {
            self.overlay = Overlay::None;
            return;
        }
        while let Some(action) = manager.update(cx, |view, _| view.take_action()) {
            match action {
                GroupManagerAction::Saved => {
                    if let Some(page) = self.pages.sessions.clone() {
                        page.update(cx, |page, cx| page.reload(cx));
                    }
                    cx.notify();
                }
            }
        }
    }

    /// Perform whatever the rule editor asked for since the last frame.
    ///
    /// Nothing has to be pushed to the terminals: the rules travel with the appearance
    /// the terminal page compares every frame, so a saved rule reaches every open tab
    /// on the next one.
    fn drain_rule_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::RuleEditor(editor) = self.overlay.clone() else {
            return;
        };
        // The toolkit's × closes the card, and a dismissal is not an action the view
        // reports — Saved is the only one it has. The layer is what says it is gone.
        if self.dialog_dismissed(window, cx) {
            self.overlay = Overlay::None;
            return;
        }
        while let Some(action) = editor.update(cx, |view, _| view.take_action()) {
            match action {
                RuleEditorAction::Saved => {
                    self.overlay = Overlay::None;
                    self.say(crate::i18n::t("已添加高亮规则", "Highlight rule added"), cx);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Sessions: the store, and the dialogs that write it.
    // ------------------------------------------------------------------

    /// Put a session in a group, or back among the ungrouped ones.
    ///
    /// The rule about which targets exist is the store's, in `session_models`: `system`
    /// belongs to the built-in local shells and is refused. What stays here is the
    /// redraw, and only when the store says it wrote something — `false` means "no move
    /// happened", not "the save failed", and reloading on it would repaint a list the
    /// store never changed.
    fn move_session(&mut self, id: &str, group: &str, cx: &mut Context<Self>) {
        let moved = {
            let mut store = self.state.store.borrow_mut();
            crate::app::session_models::move_session(&mut store, id, group)
        };
        if moved {
            if let Some(page) = self.pages.sessions.clone() {
                page.update(cx, |page, cx| page.reload(cx));
            }
            cx.notify();
        }
    }

    /// Copy a saved session under a new id, and open it for a name.
    ///
    /// The copy's shape (fresh id, suffixed name) is `session_models`'s; the editor is
    /// what is opened, and what saves it, so a duplicate is never written half-made.
    fn duplicate_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(original) = self.state.store.borrow().get(id).cloned() else {
            tracing::warn!("the list offered session {id} to duplicate, which is not saved");
            return;
        };
        let copy = crate::app::session_models::duplicate_of(&original);
        // Opened rather than written straight in, so the name can be changed before it
        // exists. The editor saves it, which is also what makes the id official.
        self.open_editor(Some(copy), window, cx);
    }

    /// Delete a saved session, after asking.
    ///
    /// A saved session is the one thing in this window that cannot be reconstructed: a
    /// closed tab was never saved, a renamed one kept its config, but a deleted session
    /// is gone and the password it held is gone with it. So this asks first, and the
    /// prompt names the session so a mis-click is visible before it is acted on.
    fn delete_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.state.store.borrow().get(id).cloned() else {
            tracing::warn!("the list offered session {id} to delete, which is not saved");
            return;
        };
        let name = session.name.clone();
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("{} {name}?", crate::i18n::t("删除会话", "Delete session")),
            Some(crate::i18n::t(
                "该会话的配置与已保存的密码都会被删除，此操作无法撤销。",
                "Its configuration and any saved password are removed, and this cannot be undone.",
            )),
            &[
                crate::i18n::t("删除", "Delete"),
                crate::i18n::t("取消", "Cancel"),
            ],
            cx,
        );
        let id = id.to_string();
        cx.spawn_in(window, async move |this, cx| {
            // Index 0 is the destructive button and the default is the last, so anything
            // that is not an explicit pick of 0 cancels — including the dialog being
            // dismissed, which resolves the channel with no value at all.
            if answer.await != Ok(0) {
                return;
            }
            this.update_in(cx, |shell, _, cx| shell.remove_session(&id, cx))
                .ok();
        })
        .detach();
    }

    /// Carry out a confirmed deletion.
    ///
    /// The record itself is the store's to remove and save (`session_models`); what is
    /// left here is the window the deletion is visible in — the tab, if one is open, and
    /// the session list.
    fn remove_session(&mut self, id: &str, cx: &mut Context<Self>) {
        {
            let mut store = self.state.store.borrow_mut();
            crate::app::session_models::delete_session(&mut store, id);
        }
        // The tab, if one is open, ends with its session — through the page, which
        // owns the strip and the pane tree.
        self.pages.terminal.update(cx, |page, cx| {
            if page.has_tab(id) {
                page.close_tab(id, cx);
            }
        });
        if let Some(page) = self.pages.sessions.clone() {
            page.update(cx, |page, cx| page.reload(cx));
        }
        cx.notify();
    }

    /// Open the session editor, on a new session or an existing one.
    ///
    /// `session` is `None` for a new one. The editor owns the draft, so it is a new
    /// entity each time: reusing one would leave a half-filled form from a cancelled
    /// edit behind the next open.
    pub(crate) fn open_editor(
        &mut self,
        session: Option<crate::config::Session>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.state.store.clone();
        // A new session joins whichever group the user is looking at, which is the one
        // they have expanded rather than an arbitrary default.
        let group = session
            .as_ref()
            .map(|s| s.group.clone())
            .unwrap_or_default();
        let editor = cx.new(|_| match session {
            Some(session) => SessionEditor::edit(store, session),
            None => SessionEditor::new_session(store, group),
        });
        // Kept as state as well as shown, because the outcome is polled from here and a
        // dialog's own closure is not a place anything can be read back from.
        self.overlay = Overlay::Editor(editor.clone());
        Self::editor_dialog(editor, window, cx);
        cx.notify();
    }

    /// The session editor, as a dialog rather than across the window.
    ///
    /// Creating or editing a session is something done *to* the list behind it, and a form
    /// that covers the window hides the thing being edited. The dialog keeps the list
    /// visible and insets the form into a card — which only works because the form carries
    /// its own sidebar (会话 / 端口转发 / 自动应答) rather than being one long column.
    pub(crate) fn editor_dialog(editor: Entity<SessionEditor>, window: &mut Window, cx: &mut gpui_kit::App) {
        let title = SharedString::from(editor.read(cx).heading());
        let weak_editor = editor.downgrade();
        Self::overlay_dialog_with_close(
            editor, title, 940., 560., window, cx,
            move |window, cx| {
                if let Some(editor) = weak_editor.upgrade() {
                    editor.update(cx, |editor, cx| editor.clear_sensitive_inputs(window, cx));
                }
            },
        );
    }

    /// Any create-or-edit view, as a dialog rather than across the window.
    ///
    /// One helper rather than five copies, because the surfaces differ only in size. Each is
    /// opened from something the user still wants to see — the session list, the settings
    /// page, the tab a file belongs to — and a form that covers the window hides exactly that.
    ///
    /// `overlay_closable(false)` throughout: clicking away would throw out a half-filled form
    /// silently, and every one of these has a 取消 button that says so instead.
    fn overlay_dialog<T: Render + 'static>(
        view: Entity<T>,
        title: SharedString,
        width: f32,
        height: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::overlay_dialog_with_close(view, title, width, height, window, cx, |_, _| {});
    }

    /// Every toolkit dismissal path (Escape, platform cancel, or the close button)
    /// runs this callback before a hidden editor can retain sensitive input entities.
    fn overlay_dialog_with_close<T: Render + 'static>(
        view: Entity<T>,
        title: SharedString,
        width: f32,
        height: f32,
        window: &mut Window,
        cx: &mut gpui_kit::App,
        on_close: impl Fn(&mut Window, &mut gpui_kit::App) + 'static,
    ) {
        let on_close = Rc::new(on_close);
        // One card at a time. `open_dialog` pushes onto the layer's queue rather than
        // replacing what is there, so opening a second one leaves the first underneath it —
        // a dialog with something unexpected beneath it, which is the overlap this avoids.
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        // Sized to the window rather than to the number the caller asked for. A card wider
        // than the window is cut off at one edge; a taller one puts its own footer past the
        // bottom, where it cannot be reached. Both used to be possible, because every caller
        // passes a constant and none of them knew how big the window was.
        //
        // What the card takes around the content it is handed: a title row and the padding
        // and gaps above and below it. Measured against a solid block of known height — a
        // dialog asked for 426 gave its content 322 and stood 355 tall altogether — so the
        // number a caller passes is spent *inside* the card, and about a hundred pixels more
        // is needed for the content box to come out that tall.
        const CHROME: f32 = 104.0;
        const MARGIN: f32 = 48.0;
        // `bounds()` is logical already — see `render` for the division that used
        // to shrink these by the scale factor and clamp cards to a window two
        // thirds of the size of the one on screen.
        let window_width = f32::from(window.bounds().size.width);
        let window_height = f32::from(window.bounds().size.height);
        let width = width.min(window_width - MARGIN * 2.0).max(240.0);
        let margin_top = ((window_height - (height + CHROME)) / 2.0).max(24.0);
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let on_close = on_close.clone();
            dialog
                .on_close(move |_, window, cx| on_close(window, cx))
                .title(title.clone())
                .width(px(width))
                .margin_top(px(margin_top))
                .close_button(true)
                .overlay_closable(false)
                .child(
                    div()
                        .w_full()
                        // A floor, not a ceiling: the card itself is
                        // content-height (the toolkit sets none), so the dialog
                        // is exactly as tall as its view makes it — and grows
                        // with the content. The floor keeps a two-row list from
                        // collapsing to two rows.
                        .min_h(px(height))
                        .overflow_hidden()
                        .child(view.clone().into_any_element()),
                )
        });
    }

    /// Whether the overlay's dialog has gone away without the view saying so.
    ///
    /// The managers report `Saved` and nothing else — a dismissal is not an action any of them
    /// knows about — so the only thing that can tell the shell its card is gone is the layer
    /// itself. Without this the state would keep pointing at a view nobody can see.
    fn dialog_dismissed(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // Safe in the frame the dialog is opened: `open_dialog` pushes onto `Root's queue
        // synchronously — it panics without a mounted Root, which is how that was
        // established — so the layer is already active by the time the next drain looks.
        !window.has_active_dialog(cx)
    }

    // ------------------------------------------------------------------
    // Detached windows.
    // ------------------------------------------------------------------

    /// Open the process monitor, or bring the one that is already open forward.
    ///
    /// A separate window rather than a panel: the table is something you keep beside
    /// the terminal, and a panel would take width from the terminal every time you
    /// looked at what was using the CPU.
    pub(crate) fn open_process_window(&mut self, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let tab = self.pages.terminal.read(cx).active_tab_id();
        let host = self.active_host(cx);
        self.process_window.show(
            cx,
            state,
            tab,
            host,
            process_window_title,
            (640., 520.),
            // The table's five columns stop fitting below this, and the original's own
            // minimum is not much smaller.
            (420., 280.),
            ProcessWindowView::new,
        );
    }

    /// Open the system-information window, or bring the open one forward.
    pub(crate) fn open_system_info_window(&mut self, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let tab = self.pages.terminal.read(cx).active_tab_id();
        let host = self.active_host(cx);
        self.system_info_window.show(
            cx,
            state,
            tab,
            host,
            system_info_window_title,
            // The original's own preferred size: seven cards, each wide enough for five
            // columns.
            (1160., 760.),
            (760., 520.),
            SystemInfoWindowView::new,
        );
    }

    /// The connection label of the active tab, for a detached window's title.
    fn active_host(&self, cx: &Context<Self>) -> String {
        let Some(tab) = self.pages.terminal.read(cx).active_tab_id() else {
            return String::new();
        };
        self.state
            .statuses
            .lock()
            .ok()
            .and_then(|statuses| statuses.get(&tab).map(|status| status.host.clone()))
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------
    // Settings' store-level actions.
    // ------------------------------------------------------------------

    /// Write every saved session to a file the user names.
    ///
    /// The count and the failures go on the status line, which is what the import does
    /// too: an export that wrote nothing is an ordinary answer, not a dialog's worth of
    /// news, and the file manager is where the user will look for the result anyway.
    fn export_sessions(&mut self, cx: &mut Context<Self>) {
        // The save dialog is async: a blocking native dialog on the UI thread
        // re-enters the frame that opened it, and gpui's app cell refuses the
        // re-entrant borrow — the crash the import path already hit.
        cx.spawn(async move |this, cx| {
            let Some(picked) = rfd::AsyncFileDialog::new()
                .set_file_name("xenterm-connections.json")
                .add_filter("JSON", &["json"])
                .save_file()
                .await
            else {
                return;
            };
            let path = picked.path().to_path_buf();
            this.update(cx, |shell, cx| shell.export_sessions_to(path, cx))
                .ok();
        })
        .detach();
    }

    /// Write the export, and put the count on the status line.
    fn export_sessions_to(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let line = match self.state.store.borrow().export_to(&path) {
            Ok(count) => format!("{} {count}", crate::i18n::t("已导出连接", "exported")),
            Err(error) => format!("{}: {error}", crate::i18n::t("导出失败", "export failed")),
        };
        self.say(line, cx);
    }

    /// Import connections from a file the user picks: a JSON export of this
    /// app's own, or an OpenSSH config — the file is sniffed by content, not
    /// by extension, because ssh configs usually have none. The count and the
    /// failures go on the status line, as with every other import.
    pub(crate) fn import_connections(&mut self, cx: &mut Context<Self>) {
        // The picker is async: a blocking native dialog on the UI thread
        // re-enters the frame that opened it, and gpui's app cell refuses the
        // re-entrant borrow — the crash this replaced.
        cx.spawn(async move |this, cx| {
            let Some(picked) = rfd::AsyncFileDialog::new()
                .set_title(crate::i18n::t("导入连接", "Import connections"))
                .pick_file()
                .await
            else {
                return;
            };
            let path = picked.path().to_path_buf();
            this.update(cx, |shell, cx| shell.finish_import(path, cx))
                .ok();
        })
        .detach();
    }

    /// Carry out an import of the file at `path` — the app's own export first,
    /// then an OpenSSH config, and if neither parses, a line for the status bar
    /// saying so.
    ///
    /// The parsing and the store write are `session_models`'; all this decides is
    /// that the caller is a window, and so the answer has to reach the status bar
    /// and maybe the session list.
    fn finish_import(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let outcome = {
            let mut store = self.state.store.borrow_mut();
            crate::app::session_models::import_picked_file(&mut store, &path)
        };
        self.report_import(outcome, cx);
    }

    /// Add the hosts `~/.ssh/config` names, and say how many arrived.
    fn import_ssh_config(&mut self, cx: &mut Context<Self>) {
        let outcome = {
            let mut store = self.state.store.borrow_mut();
            crate::app::session_models::import_ssh_config(&mut store)
        };
        self.report_import(outcome, cx);
    }

    /// Show what an import did, and rebuild the session list if it changed.
    ///
    /// One place for both imports, because the two used to carry the same six lines
    /// of status/reload/notify each — and the one rule that matters is the `reload`
    /// flag: a rejected file leaves the store exactly as it was, so redrawing the
    /// list for it would be a repaint of the same rows.
    fn report_import(
        &mut self,
        outcome: crate::app::session_models::ImportOutcome,
        cx: &mut Context<Self>,
    ) {
        self.say(outcome.status, cx);
        if outcome.reload {
            if let Some(page) = self.pages.sessions.clone() {
                page.update(cx, |page, cx| page.reload(cx));
            }
        }
        cx.notify();
    }

    /// Send the configuration to the WebDAV server, or bring it back.
    ///
    /// The request runs on the background executor, not in the click handler: `ureq`
    /// blocks, and a twenty-second timeout would be twenty seconds of a frozen window.
    /// What crosses to the task is a `WebdavSync` of plain strings, and what comes back
    /// is a `WebdavRun` — the store itself never leaves this thread.
    ///
    /// The task is kept on the shell rather than dropped: dropping a `Task` cancels it,
    /// and a sync killed at the first frame would be a button that does nothing. The
    /// assignment is also what lets a second click cancel the first sync instead of
    /// running two.
    fn webdav_sync(&mut self, upload: bool, cx: &mut Context<Self>) {
        let prepared = crate::app::webdav::WebdavSync::prepare(&self.state.store.borrow(), upload);
        let sync = match prepared {
            Ok(sync) => sync,
            Err(status) => {
                self.say(status, cx);
                return;
            }
        };

        // Sticky on purpose: a sync can run for the request timeout, and a
        // progress note that expires mid-flight reads as finished.
        self.say_sticky(crate::i18n::t("正在同步…", "syncing…"), cx);

        self.webdav_task = Some(cx.spawn(async move |this, cx| {
            // The blocking call goes to the background executor; the `WebdavRun` that
            // comes back is data, and it is applied — status line and, for a download,
            // the import itself — back on this thread where the store lives.
            let run = cx.background_executor().spawn(async move { sync.run() }).await;
            let _ = this.update(cx, |shell, cx| {
                let line = run.status_line(&mut shell.state.store.borrow_mut());
                shell.say(line, cx);
            });
        }));
    }
}

/// A detached window that follows whichever session the main window is showing.
///
/// Two of them exist — the process monitor and the system-information window — and
/// mechanically they are the same thing: a second OS window, opened from the monitor
/// page, that has to come forward rather than stack when its button is pressed twice,
/// and that has to keep describing the session the user is looking at.
///
/// The view is held weakly. It is owned by its window, and a strong handle here would
/// keep it — and its poll — alive after the window was closed. A closed window leaves
/// the handle stale, which `WindowHandle::update` reports and this clears.
struct Detached<T: TabFollower> {
    window: Option<WindowHandle<Root>>,
    view: Option<WeakEntity<T>>,
    /// How this window titles itself, as a function of the host it describes, so the
    /// title can follow the same tab the contents do.
    title: fn(&str) -> SharedString,
}

/// What a detached window has to be able to do for [`Detached`] to drive it.
pub(crate) trait TabFollower: Render + Sized + 'static {
    /// Show `tab`'s data.
    fn set_tab(&mut self, tab: Option<String>, cx: &mut Context<Self>);
    /// The connection label the window's title should carry.
    fn host(&self) -> SharedString;
}

impl<T: TabFollower> Detached<T> {
    fn new(title: fn(&str) -> SharedString) -> Self {
        Self {
            window: None,
            view: None,
            title,
        }
    }

    /// Bring the window forward, or open it onto `state` and `tab`.
    ///
    /// `build` runs inside the new window's own context, which is why the entity cannot
    /// simply be returned alongside the handle: it is stashed on the way past.
    #[allow(clippy::too_many_arguments)]
    fn show(
        &mut self,
        cx: &mut Context<Shell>,
        state: SessionState,
        tab: Option<String>,
        host: String,
        title: fn(&str) -> SharedString,
        extent: (f32, f32),
        min_extent: (f32, f32),
        build: impl FnOnce(SessionState, Option<String>, &mut Window, &mut Context<T>) -> T + 'static,
    ) {
        if let Some(handle) = self.window.clone() {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            // The window was closed; the handle is a name for something that is gone.
            self.window = None;
            self.view = None;
        }

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(extent.0), px(extent.1)), cx)),
            window_min_size: Some(size(px(min_extent.0), px(min_extent.1))),
            titlebar: Some(TitlebarOptions {
                title: Some(title(&host).to_string().into()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let opened: Rc<RefCell<Option<Entity<T>>>> = Rc::new(RefCell::new(None));
        let stash = opened.clone();
        match cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| build(state, tab, window, cx));
            *stash.borrow_mut() = Some(view.clone());
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            Ok(handle) => {
                self.title = title;
                self.view = opened.borrow().as_ref().map(|view| view.downgrade());
                self.window = Some(handle);
            }
            Err(error) => {
                // Worth a line rather than a dialog: the click did nothing visible, and
                // the log is where "why did nothing happen" is answered.
                tracing::warn!("could not open a detached window: {error:#}");
            }
        }
    }

    /// Point the window at `tab`, and retitle it for that tab's host.
    fn follow(&mut self, tab: Option<String>, cx: &mut Context<Shell>) {
        let (Some(handle), Some(view)) = (self.window.clone(), self.view.clone()) else {
            return;
        };
        if view.update(cx, |view, cx| view.set_tab(tab, cx)).is_err() {
            // The window closed between the two calls; the next click reopens it.
            self.window = None;
            self.view = None;
            return;
        }
        let host = view
            .read_with(cx, |view, _| view.host())
            .unwrap_or_default();
        let title = self.title;
        let _ = handle.update(cx, |_, window, _| window.set_window_title(&title(&host)));
    }
}
