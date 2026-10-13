//! The command palette: every window-level command, one searchable list.
//!
//! The quick-connect palette answers "which machine"; this one answers "what
//! can this window do" — pages, splits, the detached windows, the theme. It is
//! the keyboard surface the `actions!` migration was the prerequisite for: each
//! entry here runs the same shell method its menu item and its chord do, so a
//! command discovered here is the same command everywhere else.
//!
//! The list is a plain column of rows, not a virtualized one: the command set
//! is dozens, not thousands, and a palette row is one line. The quick-connect
//! palette's list widget earns its delegate; this would not.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::{
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
    ActiveTheme as _,
};
use gpui_kit::{
    div, prelude::*, px, AnyElement, Context, Entity, KeyDownEvent, SharedString, Subscription,
    WeakEntity,
};

/// One command: what it is called and what running it does. The palette holds
/// them by value; the shell is what executes, so a row reports a pick rather
/// than reaching into the window.
pub(crate) struct Command {
    pub(crate) label: SharedString,
    pub(crate) id: CommandId,
}

/// The commands the palette offers. Adding one is an entry here and an arm in
/// [`super::shell::Shell::run_command`] — the two live apart so the palette never needs the
/// window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandId {
    QuickConnect,
    NewSession,
    ImportConfig,
    ConnectionsPage,
    TerminalPage,
    SettingsPage,
    Tunnels,
    Processes,
    SystemInfo,
    ExecutionLog,
    SplitRight,
    SplitDown,
    CloseTab,
    NextTab,
    PrevTab,
    Reconnect,
    ToggleDock,
    ToggleSidebar,
    ThemeDark,
    ThemeLight,
    ThemeSystem,
}

/// The command palette, as an overlay over the window.
pub(crate) struct CommandPalette {
    /// The filter box, which owns its own key handling: typed characters never
    /// pass through the window's keymap.
    filter: Entity<InputState>,
    /// Every command, in the order they are offered.
    commands: Rc<Vec<Command>>,
    /// The row the user picked, drained by the shell at the top of the next
    /// frame — the same "a click cannot re-enter the render it arrived in"
    /// doctrine every panel here keeps.
    picked: Rc<RefCell<Option<CommandId>>>,
    /// The row the keyboard has moved to, an index into the *filtered* list.
    /// Reset to the top whenever the filter changes, because a selection that
    /// pointed past the shrunken list would be a selection of nothing.
    selected: usize,
    /// What keeps the rows re-filtered while the filter box types.
    _filter_subscription: Subscription,
}

impl CommandPalette {
    pub(crate) fn new(commands: Vec<Command>, window: &mut gpui_kit::Window, cx: &mut Context<Self>) -> Self {
        let commands = Rc::new(commands);
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::t("输入命令…", "Type a command…"))
        });
        let picked: Rc<RefCell<Option<CommandId>>> = Rc::new(RefCell::new(None));
        // Enter runs the selected match — the top row until the arrows say
        // otherwise: a palette typed down to one row is a command asked for, and
        // asking again by clicking it would be ceremony.
        let subscription = {
            let picked = picked.clone();
            let commands = commands.clone();
            cx.subscribe_in(&filter, window, move |this, _, event: &InputEvent, _, cx| {
                match event {
                    InputEvent::PressEnter { .. } => {
                        let query = this.filter.read(cx).value().to_string();
                        let visible = visible_commands(&commands, &query);
                        // The index was reset on every filter change, so it can
                        // only be past the end if the list emptied this frame.
                        let last = visible.len().saturating_sub(1);
                        if let Some(command) = visible.get(this.selected.min(last)) {
                            *picked.borrow_mut() = Some(command.id);
                            cx.notify();
                        }
                    }
                    // Typing reshapes the list; the keyboard selection goes back
                    // to the top, where the eye already is.
                    InputEvent::Change { .. } => this.selected = 0,
                    _ => {}
                }
            })
        };
        Self {
            filter,
            commands,
            picked,
            selected: 0,
            _filter_subscription: subscription,
        }
    }

    /// The keyboard selection, an index into the filtered list. For the
    /// tests that press the arrows; the card itself reads the field directly.
    #[cfg(test)]
    pub(crate) fn selected_index(&self) -> usize {
        self.selected
    }

    /// Take the pick, if the frame has one.
    pub(crate) fn take_pick(&mut self) -> Option<CommandId> {
        self.picked.borrow_mut().take()
    }

    /// Move the keyboard selection by `delta` rows, wrapping at the ends.
    ///
    /// The palette had hover-only rows, which made Enter permanently mean "the
    /// first match": the arrows moved nothing, and the only row the user could
    /// point at was the one the mouse happened to be over. This gives the
    /// keyboard the same authority the mouse has.
    fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).value().to_string();
        let count = visible_commands(&self.commands, &query).len();
        if count == 0 {
            return;
        }
        let current = self.selected.min(count - 1) as i32;
        self.selected = (current + delta).rem_euclid(count as i32) as usize;
        cx.notify();
    }

    /// The caret goes straight into the filter, the same way the quick-connect
    /// palette's does.
    pub(crate) fn focus(&mut self, window: &mut gpui_kit::Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |input, cx| input.focus(window, cx));
    }
}

fn visible_commands<'a>(commands: &'a [Command], query: &str) -> Vec<&'a Command> {
    let query = query.trim().to_lowercase();
    commands
        .iter()
        .filter(|command| {
            query.is_empty() || command.label.to_lowercase().contains(&query)
        })
        .collect()
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut gpui_kit::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let query = self.filter.read(cx).value().to_string();
        let visible = visible_commands(&self.commands, &query);
        // The selection cannot outlive the list it indexes; clamped here rather
        // than only on filter change so a stale index never paints a row.
        let selected = self.selected.min(visible.len().saturating_sub(1));

        let rows: Vec<AnyElement> = visible
            .into_iter()
            .enumerate()
            .map(|(index, command)| {
                let id = command.id;
                let picked = self.picked.clone();
                let is_selected = index == selected;
                h_flex()
                    .id(SharedString::from(format!("cmd-{}", id as u8)))
                    .debug_selector(move || format!("command-palette-row-{id:?}"))
                    .w_full()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(is_selected, |this| this.bg(theme.muted))
                    .hover(|this| this.bg(theme.muted))
                    .on_click(move |_, _, _cx| {
                        // The pick is drained by the shell at the top of the next
                        // frame; this click is only the message.
                        *picked.borrow_mut() = Some(id);
                    })
                    .child(
                        div()
                            .text_sm()
                            // The selected row is the one Enter will run, so it
                            // reads as more than a hover: semibold against the
                            // selection's background, the way the quick-connect
                            // palette marks its own pick.
                            .when(is_selected, |this| {
                                this.font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            })
                            .child(command.label.clone()),
                    )
                    .into_any_element()
            })
            .collect();

        v_flex()
            .size_full()
            .min_h_0()
            .on_key_down(cx.listener(
                |this, event: &KeyDownEvent, _, cx| {
                    // The arrows belong to the list, not to the input: a
                    // single-line filter has no vertical caret for them to
                    // move. Stopped once handled so the list below does not
                    // scroll under a selection that just moved.
                    match event.keystroke.key.as_str() {
                        "down" => this.move_selection(1, cx),
                        "up" => this.move_selection(-1, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                },
            ))
            .child(Input::new(&self.filter))
            .child(
                div()
                    .text_xs()
                    .px_3()
                    .py_1()
                    .text_color(muted)
                    // The same hint convention as the quick-connect palette:
                    // say what the keyboard does, once, quietly.
                    .child(crate::i18n::t(
                        "↑↓ 选择 · Enter 运行 · Esc 关闭",
                        "↑↓ select · Enter run · Esc close",
                    )),
            )
            .child(
                div()
                    .id("command-palette-list")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(rows.is_empty(), |this| {
                        this.child(
                            div()
                                .w_full()
                                .px_3()
                                .py_3()
                                .text_xs()
                                .text_color(muted)
                                .child(crate::i18n::t("没有匹配的命令", "No command matches")),
                        )
                    })
                    .children(rows),
            )
            .into_any_element()
    }
}
