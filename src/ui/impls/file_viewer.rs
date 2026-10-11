//! The built-in file viewer, and the editor it becomes when the file was opened to change.
//!
//! The original has both and they are one view: a file opened with `view` is read-only, one
//! opened with `edit` can be saved back. The difference is a flag, and the flag is on the
//! session command — `SftpCommand::ReadText { remote, edit }` — so this module's whole job is
//! the text, the flag, and reporting what to do with them.
//!
//! It is an overlay rather than a panel because it is about one file: a viewer that took a
//! column would push the terminal aside for something opened to look at once.
//!
//! # This view is wired, and here is what wiring it took
//!
//! It was wired on the sixth attempt, as a repair round rather than a feature round. The list
//! below is kept because the five failures in it are the useful part: each was a fact about
//! the toolkit that could not be known without trying, and each cost a reverted round.
//!
//! Four rounds have been spent trying to fit this into rounds that could not hold it, and each
//! one ended by reverting. The feature is about twenty edits and the integration ones are the
//! half that does not show up when the work is planned, so the list is here instead of in my
//! head. Nothing below is done; the module is not in `mod.rs`, so none of it compiles.
//!
//! **A fifth attempt got the whole thing in and failed on five compiler errors.** They are
//! recorded here because that attempt is the closest this has come, and every one of them is a
//! one-line repair once it is known:
//!
//! 1. pply_event in iew.rs takes &Arc<...> and is called with the Arc — one &.
//! 2. **The subscription does not work and should not be used.** cx.subscribe_in needs the
//!    view to be an EventEmitter, and this one reports through pending/	ake_action like
//!    every other panel here. Drain it instead: at the top of drain_opened_file, poll the
//!    existing open_file with 	ake_action, the way the tunnel panel's actions are drained.
//! 3. A match arm on &FileViewerAction binds **by reference**, so Save { path, content }
//!    gives &String and SftpCommand::WriteText { remote, content } wants String. Both
//!    need .clone(). The field names are right: WriteText { remote, content }.
//! 4. SessionEvent::SftpFileText must be handled in pply_event, not pump_messages — it is
//!    a free function, so the slot is one more parameter **and** one more argument.
//! 5. mt rewrites the file between edits, so an anchor copied from an earlier read may no
//!    longer match. Re-read before replacing, or use the edit tool.
//!//! The session side, which is the half that failed both times:
//!
//! 1. `session_state.rs`: add `pub(crate) opened_file: Arc<Mutex<Option<OpenedFile>>>` to
//!    `SessionState`, initialise it in `new`, and pass it in `sink_for`'s argument list.
//!    `OpenedFile { path, name, content, editable, error }` is the struct the shell reads.
//! 2. `view.rs`: add the same parameter to `TerminalView::new` **and** to `pump_messages` —
//!    both take `tunnels: TabTunnels` today, so the same anchor matches twice — pass it at the
//!    `pump_messages(...)` call site inside `new`, and add the `SessionEvent::SftpFileText`
//!    arm that writes the slot. That arm does not exist at all today: the GPUI shell drops the
//!    event on the floor, which is the whole reason this feature is missing.
//!
//! The shell side:
//!
//! 3. `Overlay::File(Entity<FileViewerView>)`, a field `open_file`, and
//!    `_file_subscription: Option<Subscription>` — the subscription must be **kept**, because
//!    dropping it leaves a Save button that draws, presses, and does nothing.
//! 4. The render arm beside the tunnel panel's, and a `drain_opened_file(window, cx)` called
//!    from the same block as the other drains; it takes the slot, builds the view, sets the
//!    overlay, and subscribes to it. `Save` sends `SftpCommand::WriteText { remote, content }`
//!    through the active tab's handle; `Close` clears the overlay.
//! 5. `sftp_panel.rs`: `PanelAction::View(String)` and `Edit(String)`, the two arms that send
//!    `SftpCommand::ReadText { remote, edit }`, and two row-menu entries — 查看 and 编辑 —
//!    beside the two that hand the file to the OS.
//!
//! And a test: set the text through the textarea's own state, press Save, and assert the action
//! carries what the box holds. The row-menu entries cannot be tested — a `PopupMenuItem` lives
//! in the toolkit's popup layer — so the two segments above them *are* the coverage.

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        input::{Textarea, TextareaState},
        v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    },
    div,
    prelude::*,
    Animation, AnimationExt as _, AnyElement, Entity, SharedString, Task, Window,
};

/// How long a save may stay unanswered before the button gives up being busy.
/// Same covenant as the SFTP panel's listing timeout: it exists for the reply
/// that is never coming, and is generous toward the one that is merely slow.
const SAVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// What the viewer asks the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileViewerAction {
    /// Write the text back to the remote path it came from.
    Save { path: String, content: String },
    /// Close the viewer without writing anything.
    Close,
}

/// One remote file, open in a text area.
pub(crate) struct FileViewerView {
    /// The remote path, which is what a save goes back to.
    path: String,
    /// The name as the heading shows it.
    name: String,
    /// Whether this file was opened to be changed. A read-only viewer still has a text area —
    /// it is the same widget, with read-only mode preserving selection and copy.
    editable: bool,
    /// The text itself. An entity because that is what the input widget is built from.
    text: Entity<TextareaState>,
    /// What went wrong last time, if anything, in the worker's own words.
    error: Option<String>,
    /// When the current save went out, which turns the Save button into a
    /// busy one. A timestamp like the SFTP panel's listing spinner: the write's
    /// completion comes back as a session status message this view never hears,
    /// so a write that dies silently would otherwise leave the button busy
    /// forever. The timeout bounds that.
    saving_since: Option<std::time::Instant>,
    pending: Option<FileViewerAction>,
    /// Kept alive so the input keeps its focus behaviour across frames.
    _task: Option<Task<()>>,
}

impl FileViewerView {
    /// The viewer for a file whose text has just arrived.
    ///
    /// `error` is the read failure the session reported, if it reported one: the original
    /// shows it in the same place the text would be, because "this file could not be read" is
    /// more useful than an empty editor that looks like an empty file.
    pub(crate) fn new(
        path: String,
        name: String,
        content: String,
        editable: bool,
        error: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(crate::i18n::t("空文件", "This file is empty"))
        });
        text.update(cx, |state, cx| {
            state.set_value(SharedString::from(content), window, cx);
        });
        Self {
            path,
            name,
            editable,
            text,
            error: (!error.is_empty()).then_some(error),
            saving_since: None,
            pending: None,
            _task: None,
        }
    }

    pub(crate) fn take_action(&mut self) -> Option<FileViewerAction> {
        self.pending.take()
    }

    /// Whether a save is still genuinely in flight. The write's answer arrives
    /// as a session status message somewhere else, so this reads the clock
    /// rather than an acknowledgment — long enough for a big file over a slow
    /// link, short enough that a lost write does not spin all day.
    fn is_saving(&self) -> bool {
        self.saving_since
            .map(|since| since.elapsed() < SAVE_TIMEOUT)
            .unwrap_or(false)
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    /// One row of the form-free header: the name, whether it can be saved, and the two
    /// buttons. No labels beyond the file's own name, because the title bar already says
    /// which file this is.
    fn header(&self, cx: &Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        // A read-only file gets no Save button rather than a dead one: the 只读 label beside
        // the name is what says why, and a button that does nothing when pressed is worse than
        // an absent one.
        let save = self.editable.then(|| {
            let saving = self.is_saving();
            let button = Button::new("file-viewer-save")
                .debug_selector(|| "file-viewer-save".to_string())
                .icon(Icon::new(if saving {
                    gpui_kit::assets::IconName::LoaderCircle
                } else {
                    gpui_kit::assets::IconName::Save
                }))
                .label(if saving {
                    crate::i18n::t("保存中…", "Saving…")
                } else {
                    crate::i18n::t("保存", "Save")
                })
                .small()
                .outline()
                .disabled(saving)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let content = this.text.read(cx).value().to_string();
                    this.pending = Some(FileViewerAction::Save {
                        path: this.path.clone(),
                        content,
                    });
                    // Clicked is busy: the write is someone else's to finish,
                    // but the wait is this button's to show.
                    this.saving_since = Some(std::time::Instant::now());
                    cx.notify();
                }));
            if saving {
                // A busy button breathes: the icon is the library's static
                // loader glyph, so the pulse is what says "working" rather
                // than "pressed". Disabled meanwhile, so a second click queues
                // nothing behind a write already going out.
                button
                    .with_animation(
                        "file-viewer-saving",
                        Animation::new(std::time::Duration::from_millis(900)).repeat(),
                        |button, delta| {
                            let pulse = 0.5 - 0.5 * (std::f32::consts::TAU * delta).cos();
                            button.opacity(0.55 + 0.45 * pulse)
                        },
                    )
                    .into_any_element()
            } else {
                button.into_any_element()
            }
        });

        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .child(SharedString::from(self.name.clone())),
            )
            .when(!self.editable, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(crate::i18n::t("只读", "Read only")),
                )
            })
            .when_some(save, |this, save| this.child(save))
            .child(
                Button::new("file-viewer-close")
                    .icon(Icon::new(gpui_kit::assets::IconName::X))
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pending = Some(FileViewerAction::Close);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

impl Render for FileViewerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.error {
            Some(error) => v_flex()
                .w_full()
                .p_3()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(SharedString::from(error.clone()))
                .into_any_element(),
            None => v_flex()
                .w_full()
                .flex_1()
                .min_h_0()
                .child(
                    // Multiline inputs default to auto height. The text area
                    // must use this body's height budget rather than one row.
                    Textarea::new(&self.text).h_full().readonly(!self.editable),
                )
                .into_any_element(),
        };

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(self.header(cx))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::{Modifiers, TestAppContext};

    /// A save carries the text the box holds, to the path the file was opened from.
    ///
    /// The viewer's whole contract. It is worth testing because the text lives in an input
    /// entity and the path lives on the view, so a save is the one moment the two meet — and
    /// a save that writes the file's *original* text, or to the wrong path, is silent.
    ///
    /// This test sets the textarea state directly to isolate the Save payload.
    /// The event-level tests below cover real typing and read-only interaction.
    #[gpui_kit::gpui::test]
    fn saving_reports_the_text_the_box_holds(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            FileViewerView::new(
                "/etc/motd".to_string(),
                "motd".to_string(),
                "original".to_string(),
                true,
                String::new(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        assert_eq!(
            view.read_with(cx, |viewer, _| viewer.path().to_string()),
            "/etc/motd",
            "the viewer remembers where the file came from"
        );

        cx.update(|window, cx| {
            view.update(cx, |viewer, cx| {
                viewer.text.clone().update(cx, |state, cx| {
                    state.set_value(gpui_kit::SharedString::from("edited"), window, cx);
                });
                cx.notify();
            });
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        let bounds = cx
            .debug_bounds("file-viewer-save")
            .unwrap_or_else(|| panic!("the save button was not drawn for an editable file"));
        cx.simulate_click(bounds.center(), Modifiers::default());

        assert_eq!(
            view.update(cx, |viewer, _| viewer.take_action()),
            Some(FileViewerAction::Save {
                path: "/etc/motd".to_string(),
                content: "edited".to_string(),
            })
        );
    }

    /// A read-only file is not offered a save at all, rather than offered a dead one.
    #[gpui_kit::gpui::test]
    fn a_read_only_file_has_no_save_button(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            FileViewerView::new(
                "/var/log/syslog".to_string(),
                "syslog".to_string(),
                "text".to_string(),
                false,
                String::new(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("file-viewer-save").is_none(),
            "a viewer that cannot write does not offer to"
        );
        let _ = view;
    }
    struct LayoutHarness {
        viewer: Entity<FileViewerView>,
        legacy_min_height: bool,
    }

    impl Render for LayoutHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .debug_selector(|| "viewer-height-wrapper".into())
                .w(gpui_kit::px(640.))
                .map(|element| {
                    if self.legacy_min_height {
                        element.min_h(gpui_kit::px(400.))
                    } else {
                        element.h(gpui_kit::px(400.)).min_h_0().overflow_hidden()
                    }
                })
                .child(self.viewer.clone())
        }
    }

    fn open_fixture(
        cx: &mut TestAppContext,
        editable: bool,
        legacy_min_height: bool,
        content: String,
    ) -> (
        Entity<FileViewerView>,
        &mut gpui_kit::gpui::VisualTestContext,
    ) {
        cx.update(gpui_kit::init);
        let (harness, cx) = cx.add_window_view(move |window, cx| {
            let viewer = cx.new(|cx| {
                FileViewerView::new(
                    "/synthetic/fixture.txt".into(),
                    "fixture.txt".into(),
                    content,
                    editable,
                    String::new(),
                    window,
                    cx,
                )
            });
            LayoutHarness {
                viewer,
                legacy_min_height,
            }
        });
        draw(cx);
        let viewer = harness.read_with(cx, |harness, _| harness.viewer.clone());
        (viewer, cx)
    }

    fn draw(cx: &mut gpui_kit::gpui::VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
    }

    fn focus_text(viewer: &Entity<FileViewerView>, cx: &mut gpui_kit::gpui::VisualTestContext) {
        use gpui_kit::gpui::Focusable as _;
        cx.update(|window, cx| {
            let text = viewer.read(cx).text.clone();
            let focus = text.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        });
    }

    fn shortcut(key: &str) -> String {
        format!(
            "{}-{key}",
            if cfg!(target_os = "macos") {
                "cmd"
            } else {
                "ctrl"
            }
        )
    }

    #[gpui_kit::gpui::test]
    fn textarea_uses_the_available_height_in_both_overlay_wrappers(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        let mut measurements = Vec::new();
        for legacy_min_height in [true, false] {
            let content = (0..80)
                .map(|n| format!("fixture line {n}\n"))
                .collect::<String>();
            let (viewer, cx) = open_fixture(cx, false, legacy_min_height, content);
            let wrapper = cx.debug_bounds("viewer-height-wrapper").unwrap();
            let input = cx.update(|window, cx| {
                window
                    .find(("input", viewer.read(cx).text.entity_id()))
                    .bounds()
            });
            measurements.push((
                legacy_min_height,
                f32::from(wrapper.size.height),
                f32::from(input.size.height),
            ));
        }
        eprintln!("legacy_min_height, wrapper, textarea: {measurements:?}");
        for (legacy, wrapper, input) in measurements {
            assert!(input >= wrapper * 0.65, "textarea should use the viewer's height, not a row-height input: legacy={legacy}, wrapper={wrapper}, textarea={input}");
        }
    }

    #[gpui_kit::gpui::test]
    fn readonly_view_rejects_edits_but_keeps_selection_and_copy(cx: &mut TestAppContext) {
        let original = "first line\nsecond 中文\nthird line".to_string();
        let (viewer, cx) = open_fixture(cx, false, false, original.clone());
        focus_text(&viewer, cx);
        cx.simulate_keystrokes(&shortcut("a"));
        assert_eq!(
            viewer.read_with(cx, |viewer, cx| viewer
                .text
                .read(cx)
                .selected_value()
                .to_string()),
            original
        );
        cx.simulate_keystrokes(&shortcut("c"));
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some(original.clone())
        );
        cx.simulate_input("attempted replacement");
        cx.simulate_keystrokes("backspace delete");
        cx.update(|_, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("paste attempt".into()))
        });
        cx.simulate_keystrokes(&shortcut("v"));
        assert_eq!(
            viewer.read_with(cx, |viewer, cx| viewer.text.read(cx).value().to_string()),
            original
        );
        assert!(viewer
            .update(cx, |viewer, _| viewer.take_action())
            .is_none());
    }

    #[gpui_kit::gpui::test]
    fn editable_view_accepts_real_input_and_close_never_saves_it(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        let (viewer, cx) = open_fixture(cx, true, false, "original".into());
        focus_text(&viewer, cx);
        cx.simulate_keystrokes(&shortcut("a"));
        cx.simulate_input("edited draft\nsecond line");
        assert_eq!(
            viewer.read_with(cx, |viewer, cx| viewer.text.read(cx).value().to_string()),
            "edited draft\nsecond line"
        );
        assert!(viewer
            .update(cx, |viewer, _| viewer.take_action())
            .is_none());
        let save = cx.debug_bounds("file-viewer-save").unwrap();
        cx.simulate_click(save.center(), Modifiers::default());
        assert_eq!(
            viewer.update(cx, |viewer, _| viewer.take_action()),
            Some(FileViewerAction::Save {
                path: "/synthetic/fixture.txt".into(),
                content: "edited draft\nsecond line".into()
            })
        );
        let close = cx.update(|window, _| window.find("file-viewer-close").bounds());
        cx.simulate_click(close.center(), Modifiers::default());
        assert_eq!(
            viewer.update(cx, |viewer, _| viewer.take_action()),
            Some(FileViewerAction::Close)
        );
        assert!(viewer
            .update(cx, |viewer, _| viewer.take_action())
            .is_none());

        let (viewer, cx) = open_fixture(cx, true, false, "unchanged remote".into());
        focus_text(&viewer, cx);
        cx.simulate_input("unsaved draft");
        let close = cx.update(|window, _| window.find("file-viewer-close").bounds());
        cx.simulate_click(close.center(), Modifiers::default());
        assert_eq!(
            viewer.update(cx, |viewer, _| viewer.take_action()),
            Some(FileViewerAction::Close)
        );
        assert!(viewer
            .update(cx, |viewer, _| viewer.take_action())
            .is_none());
    }
}
