//! The window's keyboard actions, and the one place the keymap is written down.
//!
//! Before this module the shortcuts were keystroke strings matched inside two
//! `on_key_down` closures — one on the window root, one on the terminal — which
//! meant no user rebinding, no way to name a shortcut in a menu, no command
//! palette, and no way to see the whole keymap without reading two render
//! methods. Each entry here is one action and one binding: the action is the
//! verb, `.on_action` handlers declare who can do it, and this file is the only
//! place that names a chord.
//!
//! The chord choices respect the fact that the terminal is a keyboard surface of
//! its own: a binding at the window scope would otherwise swallow a control
//! chord the shell behind the terminal is using. Ctrl+K is the one deliberate
//! exception — the empty state advertises it, so the palette wins it — and the
//! rest moved onto Shift-carrying chords (Ctrl+Shift+W/E/O) that no shell
//! reads as a control character.
//!
//! Contexts, from the focus outwards: `Terminal` on the grid (also its
//! accessibility role), `Input` on the toolkit's fields, `Dialog` on the modal
//! layer, and `Shell` on the window root. A binding's predicate is evaluated
//! against that whole stack, so `Shell && !Terminal` is how "the window's Enter,
//! but never the terminal's" is written.

use gpui_kit::{actions, KeyBinding, NoAction};

actions!(xenterm, [
    /// The quick-connect palette.
    QuickConnect,
    /// The command palette: every window-level command, one searchable list.
    CommandPalette,
    /// Bring back the active session after it ended.
    Reconnect,
    /// One tab to the right, wrapping.
    NextTab,
    /// One tab to the left, wrapping.
    PrevTab,
    /// Close the active tab.
    CloseTab,
    /// Put the next tab in a pane beside this one.
    SplitRight,
    /// Put the next tab in a pane below this one.
    SplitDown,
    /// Move the focus around the split panes, in layout order.
    CyclePane,
    /// Terminal: copy the selection.
    CopySelection,
    /// Terminal: paste from the clipboard.
    Paste,
    /// Terminal: open the find bar.
    Find,
    /// Terminal: one step larger.
    ZoomIn,
    /// Terminal: one step smaller.
    ZoomOut,
    /// Terminal: back to the settings' size.
    ZoomReset,
]);

/// The paste pair carries which of the two chords arrived, because the PTY
/// fallback has to send the bytes the chord would have — and Insert is a named
/// key while V is a control combination, so they encode differently.
#[derive(Clone, Debug, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = xenterm, no_json)]
pub struct PasteAlternate {
    /// `true` for Shift+Insert, `false` for Ctrl+Alt+V.
    pub insert: bool,
}

/// Bind every chord once, at startup. This is the whole keymap; nothing else in
/// the crate is allowed to call `bind_keys`.
pub(crate) fn init(cx: &mut gpui_kit::App) {
    cx.bind_keys([
        // The window's own shortcuts. `Shell` is the root context, so these hold
        // wherever the focus is — unless a deeper context claims the chord first.
        KeyBinding::new("ctrl-k", QuickConnect, Some("Shell")),
        // A field is where Ctrl+K must mean nothing: the palette has no business
        // opening while a password is being typed. Deeper context wins the chord.
        KeyBinding::new("ctrl-k", NoAction {}, Some("Input")),
        KeyBinding::new("ctrl-k", NoAction {}, Some("Dialog")),
        KeyBinding::new("ctrl-shift-p", CommandPalette, Some("Shell")),
        KeyBinding::new("ctrl-shift-p", NoAction {}, Some("Input")),
        KeyBinding::new("ctrl-shift-p", NoAction {}, Some("Dialog")),
        // Enter reconnects a dead session everywhere except inside the surfaces
        // that own the key: the terminal sends it to the shell, and a field or a
        // dialog has its own Enter.
        KeyBinding::new(
            "enter",
            Reconnect,
            Some("Shell && !Terminal && !Input && !Dialog"),
        ),
        KeyBinding::new("ctrl-tab", NextTab, Some("Shell")),
        KeyBinding::new("ctrl-shift-tab", PrevTab, Some("Shell")),
        // Shift-carrying chords, because Ctrl+W is a shell's delete-word and this
        // is a terminal client before it is anything else.
        KeyBinding::new("ctrl-shift-w", CloseTab, Some("Shell")),
        KeyBinding::new("ctrl-shift-e", SplitRight, Some("Shell")),
        KeyBinding::new("ctrl-shift-o", SplitDown, Some("Shell")),
        // F6 is the window-manager's own "move between regions" chord; the
        // Ctrl form leaves the bare key to whatever wants it.
        KeyBinding::new("ctrl-f6", CyclePane, Some("Shell")),
        // Deliberately absent: Ctrl+1..9 for per-index tab focus. Ctrl+2 through
        // Ctrl+8 are control characters (NUL through GS) that a shell expects to
        // receive, and a window-scope binding would take them from the terminal —
        // the same reason Ctrl+W became Ctrl+Shift+W. Tab switching is Ctrl+Tab.
        // The terminal's own set. Ctrl+C stays unbound — it is SIGINT, the one
        // chord a terminal must never take away. Three families stay keystroke
        // checks in the view rather than bindings, because a static keymap cannot
        // express their conditions: Escape/Backspace (the find bar's only while it
        // is open), and PageUp/PageDown/Home/End (scroll keys on the normal screen,
        // the program's keys on the alternate one — and which variant depends on
        // the snapshot, not the focus).
        KeyBinding::new("ctrl-shift-c", CopySelection, Some("Terminal")),
        KeyBinding::new("ctrl-v", Paste, Some("Terminal")),
        KeyBinding::new("ctrl-shift-v", Paste, Some("Terminal")),
        KeyBinding::new(
            "ctrl-alt-v",
            PasteAlternate { insert: false },
            Some("Terminal"),
        ),
        KeyBinding::new(
            "shift-insert",
            PasteAlternate { insert: true },
            Some("Terminal"),
        ),
        KeyBinding::new("ctrl-f", Find, Some("Terminal")),
        KeyBinding::new("ctrl-=", ZoomIn, Some("Terminal")),
        KeyBinding::new("ctrl-+", ZoomIn, Some("Terminal")),
        KeyBinding::new("ctrl-shift-=", ZoomIn, Some("Terminal")),
        KeyBinding::new("ctrl--", ZoomOut, Some("Terminal")),
        KeyBinding::new("ctrl-shift--", ZoomOut, Some("Terminal")),
        KeyBinding::new("ctrl-0", ZoomReset, Some("Terminal")),
    ]);
}
