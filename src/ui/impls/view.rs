//! One terminal, from a local PTY to the grid on screen.
//!
//! This is the view the renderer in the sibling module exists for: it owns the
//! `TermBuffer` the pump feeds, the measured cell metrics, and the keyboard path
//! back to the PTY. What it deliberately does not own is layout — tabs, the session
//! sidebar and the SFTP panel are #18's, and this view is written to sit inside
//! whatever arrangement that produces rather than to be one.
//!
//! Why a local shell and not an SSH session: a renderer has to be verifiable, and
//! the cheapest thing that proves it draws is a PTY that always exists. An SSH
//! session would make every check of this view depend on a server being reachable,
//! which is the same reasoning that put `plugin-selftest` in the box. The SSH path
//! produces the same `SessionEvent`s into the same buffer, so what is exercised
//! here is the rendering, not a special case of it.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui_kit::{
    component::{dialog::DialogButtonProps, h_flex, v_flex, ActiveTheme, Root, Sizable as _, Size,
        spinner::Spinner},
    div,
    prelude::*,
    px, App, Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle, Focusable,
    Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render,
    Role, ScrollDelta, ScrollWheelEvent, SharedString, Task, UTF16Selection, Window,
};

use crate::session::protocol::{
    CredentialResponder, HostKeyResponder, MfaResponder, SessionCommand, SessionEvent,
    SessionHandle,
};
use crate::terminal::{self, TermBuffer};

use super::event_sink::UiMessage;
use super::terminal::{rgba_to_hsla, CursorStyle, GridStyle};
use super::{terminal_grid, CellMetrics, GridSnapshot};

/// The find bar's resting height, in pixels: one text line and its padding.
/// The slide's ceiling and the closed state's zero are the only two values the
/// animation needs; the bar clips inside its wrapper, so this bounds rather
/// than measures.
const FIND_BAR_HEIGHT: f32 = 32.0;
/// How long the find bar's open/close slide takes.
const FIND_BAR_SLIDE: std::time::Duration = std::time::Duration::from_millis(160);
/// The slide's tick, in milliseconds — one layout per tick, the pace the
/// sidebar's own slide runs at.
const FIND_BAR_TICK_MS: u64 = 16;

/// The size the settings stepper owns, and what Ctrl+0 returns a session to.
///
/// A constant here rather than read from the store because the view is built before the
/// shell has a store to hand it, and the settings page's font control does not reach
/// the terminal yet. Wiring that through is its own change; this keeps the zoom range
/// and the reset target coming from the one place that already defines them.
/// Everything the settings say about how a terminal looks.
///
/// One value rather than six fields on the view, because the shell compares it against
/// what the tabs are already using — the settings are stored in the config, which any
/// window or the CLI can change, so "push the change" is really "notice the change", and
/// that needs one thing to compare.
///
/// The highlighting rules are here for the same reason and are the reason they are
/// *compiled*: a rule is compared against the one already applied, and the source text of
/// the rule is what makes two sets equal — a compiled rule has forgotten its pattern.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TerminalSettings {
    pub(crate) family: SharedString,
    pub(crate) font_size: u32,
    pub(crate) bold: bool,
    /// Inset the grid from the pane's edge, so output does not sit flush
    /// against the frame.
    pub(crate) padding: bool,
    pub(crate) line_spacing: f32,
    pub(crate) cursor_style: CursorStyle,
    /// The cursor's colour, already parsed, or the theme's when unset.
    pub(crate) cursor_color: Option<Hsla>,
    /// Which preset marks the output up, if any.
    pub(crate) highlight: crate::terminal::OutputHighlightPreset,
    /// The user's own rules, as the settings store them.
    ///
    /// The source rather than the compiled form, because this value is compared: two sets
    /// of rules are the same when the rules are the same, and a compiled rule has
    /// forgotten the pattern it came from. Compiling happens where the buffer is handed
    /// them.
    pub(crate) rules: Vec<crate::config::OutputHighlightRule>,
    /// Whether a multi-line paste is shown for review before it reaches the session.
    ///
    /// Behaviour rather than appearance, and here anyway: it is a setting about the
    /// terminal, and one comparable value is what lets the shell notice a change from any
    /// writer. A second struct would be a second comparison per frame for no gain.
    pub(crate) review_multiline_paste: bool,
    /// Whether the paste shortcuts beyond Ctrl+V are live.
    ///
    /// The extras are Ctrl+Alt+V, Shift+Insert and middle-click: habits from other
    /// terminals, each of which can also be a surprise in a program that wants the mouse.
    pub(crate) paste_shortcuts: bool,
}

impl TerminalSettings {
    /// Read the settings.
    ///
    /// An empty family means the built-in font, which is what the original means by it
    /// and what the shell registers at startup. It is not the theme's monospace family:
    /// that is the platform's idea of monospace, and the two are visibly different
    /// terminals.
    pub(crate) fn from_store(store: &crate::config::ConfigStore) -> Self {
        let configured = store.font_family();
        let family = if configured.is_empty() {
            SharedString::from(crate::core::fonts::BUILT_IN_MONO)
        } else {
            SharedString::from(configured.to_string())
        };
        Self {
            family,
            font_size: store.font_size(),
            bold: store.terminal_bold(),
            padding: store.terminal_padding(),
            line_spacing: store.terminal_line_spacing(),
            cursor_style: CursorStyle::from_setting(store.terminal_cursor_style()),
            cursor_color: crate::config::hex_to_rgb(store.terminal_cursor_color())
                .map(|(r, g, b)| rgba_to_hsla(crate::terminal::Rgba { r, g, b, a: 255 })),
            // The preset and the on-switch are one setting in the buffer's terms: a
            // disabled highlighter is the `Off` preset, which is what makes "enabled" and
            // "which preset" two controls over one behaviour rather than two behaviours.
            highlight: crate::terminal::OutputHighlightPreset::from_settings(
                store.output_highlight_enabled(),
                store.output_highlight_preset(),
            ),
            rules: store.output_highlight_rules().to_vec(),
            review_multiline_paste: store.paste_confirm_enabled(),
            paste_shortcuts: store.extra_paste_shortcuts_enabled(),
        }
    }
}

/// A paste waiting for the user to look at it before it reaches the session.
struct PendingPaste {
    /// What to show, which is the clipboard as it arrived.
    text: String,
    /// What to send if the answer is yes, already encoded.
    bytes: Vec<u8>,
}

/// The terminal pane.
pub(crate) struct TerminalView {
    /// Which tab's session this view draws. Every shared map — handles, buffers,
    /// gates, SFTP listings — is keyed by it.
    tab_id: String,
    /// The parsed screen. Behind a mutex because the pump writes it from a tokio
    /// worker while this view reads it to paint.
    buffer: Arc<Mutex<TermBuffer>>,
    /// The last snapshot taken, which is what actually gets painted. A snapshot
    /// rather than a lock held during paint: `TermBuffer::render` walks the whole
    /// visible screen, and doing that inside `Render` would put that work on every
    /// repaint the window decides to do, including ones this view did not cause.
    snapshot: GridSnapshot,
    /// Cell size, measured once per viewport rather than per frame.
    metrics: Option<CellMetrics>,
    /// Where keystrokes come from.
    focus: FocusHandle,
    /// The window's live sessions, shared with the shell that started them.
    ///
    /// The view no longer owns a session: input and resize go out through whatever
    /// session is registered under this view's tab id, which is the same map the shell
    /// connects into. `None` under the tab id means no session is open, and input is
    /// dropped rather than queued somewhere nobody will read.
    handles: Rc<std::cell::RefCell<HashMap<String, SessionHandle>>>,
    /// The page this tab belongs to, for the one notification the strip cannot
    /// get anywhere else: a session-state transition (dialling, live, ended) is
    /// written into a plain shared map no dependency tracker can see, and the
    /// tab strip that draws the dot is cached between page notifications.
    page: Option<gpui_kit::WeakEntity<super::pages::terminal_page::TerminalPage>>,
    /// Keeps the message pump alive for as long as the view is.
    _pump: Task<()>,
    /// The last size sent to the session, so a resize goes out only when it changes.
    last_grid: Option<(u16, u16)>,
    /// The latest status text for this tab, from `SessionEvent::Status` and the
    /// connect/disconnect events. Drawn as one line above the grid.
    status: Option<String>,
    /// This tab's connection phase — 0 connecting, 1 connected, 2 disconnected —
    /// mirroring the shared status map. The status line reads it to decide whether
    /// the text it is showing deserves a spinner beside it, and a status text
    /// arriving over a closed session is the reconnect attempt talking, which is
    /// what moves the phase back to 0.
    conn_state: u8,
    /// An auth prompt waiting for a window to be shown in.
    ///
    /// Parked by `apply_event`, which runs inside a view update and has no `Window`, and
    /// taken by `render`, which has both a window and an `App`. One slot rather than a
    /// queue: the prompt policy in `crate::session::prompt_queue` already queues per
    /// window, so a second prompt cannot reach here until the first is answered.
    pending_prompt: Option<PendingPrompt>,
    /// Whether Enter was pressed on a tab whose session has ended.
    ///
    /// Set by `send_key`, cleared by the shell: the view can tell that a session is gone —
    /// it holds the session handle — but starting one is the shell's work, so all it can do
    /// is ask.
    reconnect_requested: bool,
    /// Whether a multi-line paste is reviewed before it is sent.
    paste_confirm: bool,
    /// Whether the paste shortcuts beyond Ctrl+V are live.
    paste_shortcuts: bool,
    /// The IME's in-progress composition, which is the only text this view holds.
    ///
    /// Held rather than forwarded, because a composition is the user still choosing a
    /// character: sending each intermediate state to the PTY would type `n`, `ni`, `nin`
    /// into the shell before the user had picked anything, and a PTY has no way to
    /// un-type them. It is drawn at the cursor instead, and sent when it commits.
    composition: Option<String>,
    /// Whether the find bar is showing.
    find_open: bool,
    /// The find bar's animated height, while it slides open or closed. Held
    /// rather than derived from `find_open` because the bar leaves by the same
    /// slide it arrived by: closed-but-still-visible is a real state here, and
    /// the bar renders whenever this is above zero.
    find_height: f32,
    /// The slide in flight. Replaced on every toggle — dropping it cancels the
    /// old slide, so opening mid-close retargets from wherever the height is.
    find_anim: Option<Task<()>>,
    /// What is in the find bar.
    ///
    /// The view's copy is the authoritative one — it is what the box shows — and it is
    /// pushed into the buffer, which is what searches. Two values rather than one
    /// because the box has to draw a caret's worth of state that the search does not
    /// care about, and because routing typing here keeps the bar from needing its own
    /// text-input entity and therefore its own `Window` at construction.
    find_query: String,
    /// This session's font size, in the core's terms.
    ///
    /// Held as the core type rather than as a bare `Pixels` so the clamping range and
    /// the reset semantics stay in one place: `FontZoom` is where "how small can this
    /// get" is answered, rather than a range re-derived in the view. One entry per view,
    /// because one view is one session and zooming a session must not resize the rest.
    zoom: crate::core::FontZoom,
    /// Where the grid was drawn last frame, needed to turn a pointer position into a
    /// cell.
    ///
    /// Recorded during paint rather than read from the hitbox: the paint closure is
    /// the only place the grid's bounds exist, and mouse events carry window
    /// coordinates. Without this a click has no way back to the character under it.
    ///
    /// A shared cell rather than a plain field because of *when* it is written:
    /// `render` builds the element and returns, and the paint closure runs after
    /// that, with an `App` and no way to update the view. The view keeps one handle
    /// and the closure gets a clone, so the value written during paint is the one a
    /// later click reads.
    grid_bounds: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    /// The anchor of the selection being dragged out, in grid cells.
    ///
    /// Kept here as well as in the buffer because the mouse callbacks need it to
    /// decide a double click from a drag, and reading it back out of the buffer
    /// would mean locking the grid on every move event for one comparison.
    drag_anchor: Option<(i32, i32)>,
    family: SharedString,
    font_size: Pixels,
    /// Whether the session's bold runs are drawn bold, and how far apart the rows sit.
    ///
    /// Held rather than read from the config at paint time: the shell reads the settings
    /// when it opens a tab and pushes changes to every tab, so a terminal never has to
    /// know where a preference is stored — and a view drawn by a test has no store.
    bold: bool,
    /// Inset the grid from the pane's edge (the padding setting).
    padding: bool,
    line_spacing: f32,
    /// The insertion cursor's shape and colour.
    cursor_style: CursorStyle,
    cursor_color: Option<Hsla>,
}

impl TerminalView {
    /// The screen, so the connect path can register it in the shared buffer map.
    pub(crate) fn buffer(&self) -> &Arc<Mutex<TermBuffer>> {
        &self.buffer
    }

    /// Open a local shell on `runtime` and return the view, ready to render.
    ///
    /// The grid starts at 24x80 because the PTY has to be told a size before a window
    /// exists to measure, and a shell that drew its first prompt at 1x1 would reflow the
    /// moment the window opened.
    pub(crate) fn new(
        tab_id: String,
        appearance: TerminalSettings,
        handles: Rc<std::cell::RefCell<HashMap<String, SessionHandle>>>,
        sftp_listings: crate::core::SftpListings,
        sftp_trees: super::session_state::TabTrees,
        statuses: crate::resource::TabStatuses,
        transfers: crate::core::TransferRecords,
        tunnels: super::session_state::TabTunnels,
        opened_file: Arc<Mutex<Option<super::session_state::OpenedFile>>>,
        messages: tokio::sync::mpsc::UnboundedReceiver<UiMessage>,
        page: Option<gpui_kit::WeakEntity<super::pages::terminal_page::TerminalPage>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (cols, rows) = (80u16, 24u16);
        // The screen exists before any session does, because a session needs somewhere
        // to be parsed into and the shell registers this in the shared buffer map when
        // it connects. Created here rather than by the connect path so the view owns
        // the one thing it draws from.
        let buffer = Arc::new(Mutex::new(TermBuffer::new(rows, cols)));
        // Dark, always: see where `render` takes the pane's background. It is the
        // buffer's flag because the palette maps a default foreground and background from
        // it, and a light palette would remap the colours a program asked for.
        if let Ok(mut buffer) = buffer.lock() {
            buffer.is_dark = true;
        }
        // The highlighter is the buffer's, not the painter's: `TermBuffer::render` applies
        // the preset and the user's rules to every run it produces, so a view that never
        // sets them draws plain output — which is what this shell did until now.
        apply_highlighting(&buffer, &appearance);

        let pumped = buffer.clone();
        let listings = sftp_listings.clone();
        let trees = sftp_trees.clone();
        let tracked = statuses.clone();
        let live_tunnels = tunnels.clone();
        let pump = cx.spawn(async move |this, cx| {
            pump_messages(
                this,
                cx,
                messages,
                pumped,
                listings,
                trees,
                tracked,
                transfers,
                live_tunnels,
                opened_file,
            )
            .await;
        });

        let family = appearance.family.clone();
        let font_size = px(appearance.font_size as f32);
        let padding = appearance.padding;
        Self {
            page,
            tab_id,
            padding,
            buffer,
            snapshot: GridSnapshot::default(),
            metrics: None,
            focus: cx.focus_handle(),
            handles,
            _pump: pump,
            last_grid: None,
            status: None,
            // A view is born while its session is still dialling: the connect path
            // starts before the first frame, so the opening phase is "connecting"
            // until an event says otherwise.
            conn_state: 0,
            pending_prompt: None,
            reconnect_requested: false,
            paste_confirm: appearance.review_multiline_paste,
            paste_shortcuts: appearance.paste_shortcuts,
            composition: None,
            find_open: false,
            // The bar starts fully closed; the first Ctrl+F slides it in.
            find_height: 0.0,
            find_anim: None,
            find_query: String::new(),
            // The zoom base is the settings' size: Ctrl+zoom moves a tab away from it,
            // and a change to the setting moves the base every tab measures from.
            zoom: crate::core::FontZoom::new(appearance.font_size as i32),
            grid_bounds: Rc::new(std::cell::Cell::new(None)),
            drag_anchor: None,
            family,
            font_size,
            bold: appearance.bold,
            line_spacing: appearance.line_spacing,
            cursor_style: appearance.cursor_style,
            cursor_color: appearance.cursor_color,
        }
    }

    /// Take new appearance settings, re-measuring the grid.
    ///
    /// The metrics are dropped rather than adjusted: they are a measurement of a font at
    /// a size, and every one of these settings changes what that measurement would be.
    /// The next frame measures again, and the session is told the new grid size by the
    /// same path a window resize takes.
    pub(crate) fn set_appearance(&mut self, appearance: TerminalSettings, cx: &mut Context<Self>) {
        self.zoom.set_base(appearance.font_size as i32);
        // A tab's own zoom override is kept: it is the user's adjustment to *this*
        // session, and a change to the base size should move the base it sits on rather
        // than discard it.
        let size = self
            .zoom
            .override_of(&self.tab_id)
            .map(i32::from)
            .unwrap_or(appearance.font_size as i32);
        // The rules live in the buffer, so a change has to reach it and the screen has to
        // be rebuilt from it — the spans already drawn came from the old rules.
        apply_highlighting(&self.buffer, &appearance);
        self.family = appearance.family;
        self.font_size = px(size as f32);
        self.bold = appearance.bold;
        self.line_spacing = appearance.line_spacing;
        self.cursor_style = appearance.cursor_style;
        self.cursor_color = appearance.cursor_color;
        self.paste_confirm = appearance.review_multiline_paste;
        self.paste_shortcuts = appearance.paste_shortcuts;
        self.metrics = None;
        self.refresh();
        cx.notify();
    }

    /// Re-read the buffer into the snapshot.
    pub(crate) fn refresh(&mut self) {
        let Ok(mut buffer) = self.buffer.lock() else {
            return;
        };
        let built = buffer.render();
        // Both overlays are read under the same lock as the render, and after it:
        // `render` refreshes `displayed_text`, which is what the column math for a
        // highlight is measured against.
        let cols = buffer.parser.screen().size().1;
        let selection = buffer.selection_rects_visible(cols);
        // Only when a search is active, so a buffer with no query does no work per frame.
        let find = if buffer.has_find_query() {
            buffer.find_matches()
        } else {
            Vec::new()
        };
        self.snapshot = GridSnapshot::of(&built);
        self.snapshot.selection = selection;
        self.snapshot.find_matches = find;
        // Carried across rather than snapshotted, because it is the one thing on screen
        // that does not come from the session: a composition lives in this view until it
        // commits, so a refresh from PTY output must not drop it.
        self.snapshot.composition = self.composition.clone();
    }

    /// Scroll the scrollback by `delta` lines, which may be fractional.
    ///
    /// A thin wrapper: the policy — banking the fraction so a momentum tail converges,
    /// clamping a stray delta, stopping at the ends — is `TermBuffer::scroll_by_lines`.
    /// The only thing this adds is taking the snapshot again, because the caller has to
    /// repaint and the screen it would repaint is the one the offset change produced.
    fn scroll(&mut self, delta: f32, cx: &mut Context<Self>) {
        let moved = {
            let Ok(mut buffer) = self.buffer.lock() else {
                return;
            };
            buffer.scroll_by_lines(delta)
        };
        if moved {
            self.refresh();
            cx.notify();
        }
    }

    /// Turn a window-space pointer position into a grid cell.
    ///
    /// `None` until the grid has been painted once, because only paint knows where
    /// it went. Negative rows and columns are deliberately not clamped here: the
    /// auto-scroll logic needs to know the pointer left the grid, and clamping
    /// would report "row 0" for a pointer a screen above it.
    fn cell_at(&self, position: gpui_kit::Point<Pixels>) -> Option<(i32, i32)> {
        let bounds = self.grid_bounds.get()?;
        let metrics = self.metrics?;
        let x = f32::from(position.x) - f32::from(bounds.origin.x);
        let y = f32::from(position.y) - f32::from(bounds.origin.y);
        let col = (x / f32::from(metrics.width)).floor() as i32;
        let row = (y / f32::from(metrics.height)).floor() as i32;
        Some((row, col))
    }

    /// Clamp a cell into the grid, for the selection API's `u16` coordinates.
    fn clamped_cell(&self, cell: (i32, i32)) -> (u16, u16) {
        let (rows, cols) = self
            .grid_bounds
            .get()
            .and_then(|bounds| self.metrics.map(|metrics| metrics.grid_size(bounds.size)))
            .unwrap_or((80, 24));
        (
            cell.0.clamp(0, rows.saturating_sub(1) as i32) as u16,
            cell.1.clamp(0, cols.saturating_sub(1) as i32) as u16,
        )
    }

    /// Begin a selection at a pointer position.
    ///
    /// The policy — what ctrl and shift mean, that the anchor is absolute — is
    /// `TermBuffer::begin_selection`'s. What this adds is the pointer-to-cell conversion
    /// and remembering the anchor in grid cells, which is what tells a later double click
    /// apart from a drag.
    fn begin_selection_at(
        &mut self,
        position: gpui_kit::Point<Pixels>,
        ctrl: bool,
        shift: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(cell) = self.cell_at(position) else {
            return;
        };
        let (row, col) = self.clamped_cell(cell);
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.begin_selection(row, col, ctrl, shift);
        }
        self.drag_anchor = Some((row as i32, col as i32));
        self.refresh();
        cx.notify();
    }

    /// Extend the in-progress selection to a pointer position.
    fn extend_selection_at(&mut self, position: gpui_kit::Point<Pixels>, cx: &mut Context<Self>) {
        if self.drag_anchor.is_none() {
            return;
        }
        let Some(cell) = self.cell_at(position) else {
            return;
        };
        let (row, col) = self.clamped_cell(cell);
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.extend_selection(row, col);
        }
        self.refresh();
        cx.notify();
    }

    /// Finish the in-progress selection and copy what it covers.
    ///
    /// Select-to-copy on release, which is what the design baseline (macOS Terminal
    /// and PuTTY alike) does: a drag that ends with text selected has already put it
    /// on the clipboard, so `Ctrl+C` is only needed when the selection was made some
    /// other way. `finish_selection` owns the click-versus-drag judgement and clears
    /// the selection when there is nothing to copy.
    fn finish_selection(&mut self, cx: &mut Context<Self>) {
        self.drag_anchor = None;
        let text = {
            let Ok(mut buffer) = self.buffer.lock() else {
                return;
            };
            buffer.finish_selection()
        };
        if let Some(text) = text {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        }
        self.refresh();
        cx.notify();
    }

    /// Select the word under the pointer (double click) and copy it.
    fn select_word_at(&mut self, position: gpui_kit::Point<Pixels>, cx: &mut Context<Self>) {
        let text = self.cell_at(position).and_then(|cell| {
            let (row, col) = self.clamped_cell(cell);
            self.buffer
                .lock()
                .ok()
                .and_then(|mut buffer| buffer.select_word_at(row, col))
        });
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        }
        self.refresh();
        cx.notify();
    }

    /// Select the whole row under the pointer (triple click) and copy it.
    fn select_line_at(&mut self, position: gpui_kit::Point<Pixels>, cx: &mut Context<Self>) {
        let text = self.cell_at(position).and_then(|cell| {
            let (row, _) = self.clamped_cell(cell);
            self.buffer
                .lock()
                .ok()
                .and_then(|mut buffer| buffer.select_line_at(row))
        });
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        }
        self.refresh();
        cx.notify();
    }

    /// Tell the session how big the grid now is.
    fn sync_size(&mut self, cols: u16, rows: u16) {
        if self.last_grid == Some((cols, rows)) {
            return;
        }
        self.last_grid = Some((cols, rows));
        if let Some(handle) = self.handles.borrow().get(&self.tab_id) {
            let _ = handle
                .commands
                .send(SessionCommand::Resize(cols as u32, rows as u32));
        }
        // The buffer reflows rather than truncates, so scrollback survives a resize
        // the way a real terminal's does (#169).
        if let Ok(mut buffer) = self.buffer.lock() {
            if (rows, cols) != buffer.parser.screen().size() {
                buffer.reflow(rows, cols);
            }
        }
    }

    /// The cell the composition is drawn at, in grid coordinates.
    ///
    /// The terminal's cursor, clamped into the grid: a program can leave the cursor one
    /// past the last column while it waits for a wrap, and a composition placed there
    /// would be drawn outside the terminal. Falls back to the origin when there is no
    /// cursor to speak of, so a caller always gets a usable cell.
    fn composition_cell(&self) -> (i32, i32) {
        let row = self.snapshot.cursor_row.max(0);
        let col = self.snapshot.cursor_col.max(0);
        (row, col)
    }

    /// Forward a mouse event to the program, if it asked for them.
    ///
    /// Returns whether the event was reported, so the caller can skip its own handling:
    /// a program that turned mouse reporting on — btop, htop, mc — wants the click on
    /// its widgets, and doing a local selection as well would both fight the program and
    /// put its screen into the clipboard.
    ///
    /// `shift` is the escape hatch every terminal offers: with it held the event is
    /// never reported, so the user can still select text out of a full-screen program.
    /// Without it, a mouse-tracking session would be one whose output could not be
    /// copied at all.
    ///
    /// `kind` follows the shared encoder's convention: 0 press, 1 release, 2 drag.
    fn report_mouse(
        &mut self,
        kind: i32,
        button: u8,
        position: gpui_kit::Point<Pixels>,
        shift: bool,
    ) -> bool {
        if !self.snapshot.mouse_tracked || shift {
            return false;
        }
        let Some(cell) = self.cell_at(position) else {
            return false;
        };
        let (row, col) = self.clamped_cell(cell);
        let (cols, rows) = self
            .grid_bounds
            .get()
            .and_then(|bounds| self.metrics.map(|metrics| metrics.grid_size(bounds.size)))
            .unwrap_or((80, 24));
        let bytes = {
            let Ok(buffer) = self.buffer.lock() else {
                return false;
            };
            let screen = buffer.parser.screen();
            let encoding = screen.mouse_protocol_encoding();
            // xterm's codes, which the shared encoder expects: motion with a button
            // held is 35, and a release is the press code flagged rather than a
            // separate number.
            let btn = if kind == 2 { 35 } else { button };
            terminal::encode_mouse_event(
                btn,
                kind == 1,
                col as i32,
                row as i32,
                cols,
                rows,
                encoding,
            )
        };
        self.send_bytes(&bytes);
        true
    }

    /// Push the bar's text into the buffer and repaint.
    ///
    /// One place, because the query reaches the search through the buffer and the
    /// overlay is a snapshot of it: a caller that updated one and not the other would
    /// leave a bar showing a query whose highlights belong to the previous one.
    fn apply_find_query(&mut self, cx: &mut Context<Self>) {
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.set_find_query(&self.find_query);
        }
        self.refresh();
        cx.notify();
    }

    /// Send raw bytes to the session, if one is open.
    ///
    /// The one place input is written, so the key path and the text path cannot drift
    /// about what "no session is open" looks like. Looking the handle up on every call
    /// rather than caching it is deliberate: a session can end (the tab stays, showing
    /// the reconnect hint) and be replaced by a reconnect, and a cached sender would
    /// keep writing into the dead one.
    fn send_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if let Some(handle) = self.handles.borrow().get(&self.tab_id) {
            let _ = handle
                .commands
                .send(SessionCommand::RawInput(bytes.to_vec()));
        }
    }

    /// Send a keystroke to the PTY.
    /// Ask to be reconnected if this tab session has ended, and say whether it had.
    ///
    /// Shared by this view own key handling and by the shell window-level one, because the
    /// frontend that came before caught Enter at the window: the key has to work whichever
    /// widget holds focus, and this view only sees it when it is the focused one.
    pub(crate) fn request_reconnect_if_ended(&mut self, cx: &mut Context<Self>) -> bool {
        let ended = match self.handles.borrow().get(&self.tab_id) {
            Some(handle) => handle.commands.is_closed() || handle.join.is_finished(),
            // No handle at all is the same statement: the shell removes it when the session
            // ends, and a missing handle is read as "not open" elsewhere in this file.
            None => true,
        };
        if ended {
            self.reconnect_requested = true;
            cx.notify();
        }
        ended
    }

    /// Whether this tab asked to be reconnected, cleared by the read.
    pub(crate) fn take_reconnect_request(&mut self) -> bool {
        std::mem::take(&mut self.reconnect_requested)
    }

    fn send_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let ctrl = keystroke.modifiers.control;
        let alt = keystroke.modifiers.alt;
        let shift = keystroke.modifiers.shift;
        // Enter, when this view happens to be the focused one. The shell also catches
        // it at the window, which is where the original catches it and the only place
        // that works whichever widget has focus — so this branch stops the event here:
        // one ended session must not be reconnected twice by two layers that both
        // saw the same key.
        if keystroke.key == "enter" && !ctrl && !alt && self.request_reconnect_if_ended(cx) {
            cx.stop_propagation();
            return;
        }
        // Copy, paste, find and zoom moved to actions bound in the `Terminal`
        // context (`impls/actions.rs`); the keymap consumes those chords before
        // this handler runs, so they are no longer matched here. Ctrl+C stays
        // unmatched on purpose — it is SIGINT — and Escape/Backspace above are
        // state-gated, which a static binding cannot be.
        // Escape closes the bar and clears the query, so the highlights go with it. A
        // search that outlived its bar would leave rectangles on screen with nothing to
        // explain them.
        if keystroke.key == "escape" && self.find_open {
            self.find_open = false;
            self.find_query.clear();
            if let Ok(mut buffer) = self.buffer.lock() {
                buffer.set_find_query("");
            }
            self.refresh();
            self.animate_find_to(0.0, cx);
            return;
        }
        // While the bar is open the editing keys belong to it: Backspace on a query is
        // the user correcting a typo, and sending it to the shell would delete into the
        // session's line while the box kept its text.
        if self.find_open {
            match keystroke.key.as_str() {
                "backspace" => {
                    if self.find_query.pop().is_some() {
                        self.apply_find_query(cx);
                    }
                    return;
                }
                // Enter and Escape are the only keys that leave the bar; everything else
                // is either text (through the input handler) or something the box has no
                // use for. Named keys are dropped rather than forwarded so an arrow key
                // does not move the session's cursor while the user is searching.
                _ => return,
            }
        }
        // Paging keys scroll the scrollback, but only on the normal screen. A
        // full-screen program — `less`, `vim`, `tmux` — puts the terminal on the
        // alternate screen and expects these keys itself, so there they fall through to
        // the encoder below and reach the program. That distinction is the whole reason
        // this is guarded rather than unconditional: swallowing PageDown inside `less`
        // would make the program unscrollable.
        if !self.snapshot.is_alt {
            let boundary = match keystroke.key.as_str() {
                "pageup" | "home" => Some(false),
                "pagedown" | "end" => Some(true),
                _ => None,
            };
            if let Some(bottom) = boundary {
                let moved = {
                    let Ok(mut buffer) = self.buffer.lock() else {
                        return;
                    };
                    buffer.scroll_to_boundary(bottom)
                };
                if moved {
                    self.refresh();
                }
                return;
            }
        }

        let bytes = match gpui_key_name(&keystroke.key) {
            // A named key, translated into the encoding the shared encoder speaks.
            Some(named) => terminal::key_to_pty_bytes(named, ctrl, alt, false),
            // Printable text, which is what `key_char` carries: it is the character
            // the layout would have typed, so on a layout where the key is labelled
            // differently it is still what the user meant. Ctrl+C and the rest go
            // through the same call, which is where the control-byte mapping lives.
            //
            // Text also arrives through `EntityInputHandler`, and the two must not
            // both send it: an ordinary letter here would be typed twice. Named keys
            // are excluded from that path — the platform sends no character for
            // Enter or an arrow — and `key_char` is `None` for them, so this arm
            // fires only for control combinations, which the handler never sees.
            None if ctrl => terminal::key_to_pty_bytes(&keystroke.key, ctrl, alt, false),
            None => Vec::new(),
        };
        self.send_bytes(&bytes);
    }

    /// Step this session's font size.
    ///
    /// `step` is `1`, `-1` or `0` for reset. The clamping and the reset semantics are
    /// `crate::core::FontZoom`'s: it owns the settings size and the per-tab rules, and
    /// re-deriving the range here would be a second answer to "how small can this get".
    ///
    /// The cell metrics are dropped rather than adjusted, because the grid is measured
    /// from the text system: a size change has to be re-measured, and a stale metric
    /// would lay the grid out at the old pitch.
    fn zoom_font(&mut self, step: i32, cx: &mut Context<Self>) {
        let size = if step == 0 {
            // The base is the settings' size, which is exactly what a reset means: back
            // to what the settings page says, not to a constant that could drift from it.
            self.zoom
                .reset(self.tab_id.as_str(), i32::from(self.zoom.base()))
        } else {
            self.zoom.zoom(self.tab_id.as_str(), step)
        };
        self.font_size = px(f32::from(size));
        self.metrics = None;
        // The session is told the new grid size on the next paint, because the number
        // of columns changed and the program needs to know.
        self.last_grid = None;
        cx.notify();
    }

    /// Copy the current selection, if there is one.
    ///
    /// The explicit copy the original binds to Ctrl+Shift+C. It is not the only way a
    /// selection reaches the clipboard — releasing a drag already copies, which is what
    /// a macOS-style terminal does — but a selection made some other way (a word, a
    /// line, a keyboard-driven range) needs a shortcut to ask for it again.
    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let text = self
            .buffer
            .lock()
            .ok()
            .map(|buffer| buffer.extract_selection_text())
            .filter(|text| !text.is_empty());
        if let Some(text) = text {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        }
    }

    /// The line above the grid: what this session is doing, or that there is none.
    ///
    /// The fallback is the original's own: a terminal with no session that says nothing
    /// at all is a blank rectangle, and the first tab of a window is exactly that — a
    /// terminal waiting for a session, which this view labels "Not connected". A
    /// tab whose session *is* open says nothing until the session has something to say,
    /// because a permanent "connected" would be a line that never changes and therefore
    /// never informs.
    fn status_line(&self) -> Option<SharedString> {
        // An ended session names the way back: the red line in the scrollback is
        // output, and output scrolls; the status line is furniture, and it is
        // where an affordance belongs.
        if self.conn_state == 2 {
            let base = self
                .status
                .clone()
                .unwrap_or_else(|| crate::i18n::t("已断开", "Disconnected").to_string());
            return Some(SharedString::from(format!(
                "{} · {}",
                base,
                crate::i18n::t("Enter 重连", "Enter to reconnect")
            )));
        }
        if let Some(status) = &self.status {
            return Some(SharedString::from(status.clone()));
        }
        let open = self.handles.borrow().contains_key(&self.tab_id);
        (!open).then(|| SharedString::from(crate::i18n::t("未连接", "Not connected")))
    }

    /// Slide the find bar towards `target` — full height, or closed.
    ///
    /// Driven by a task rather than `with_animation` so a toggle mid-slide
    /// retargets from wherever the height is: replacing `find_anim` drops the
    /// previous task, and the bar reverses smoothly instead of snapping. Each
    /// tick re-lays the workspace out, which is one terminal grid resize per
    /// frame — the cost a window resize already pays, spread over 160ms instead
    /// of landing as one jump that shoves the whole session up or down.
    fn animate_find_to(&mut self, target: f32, cx: &mut Context<Self>) {
        let start = self.find_height;
        if (target - start).abs() < 0.5 {
            self.find_height = target;
            self.find_anim = None;
            cx.notify();
            return;
        }
        let begun = std::time::Instant::now();
        self.find_anim = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(FIND_BAR_TICK_MS))
                    .await;
                let t = (begun.elapsed().as_secs_f32() / FIND_BAR_SLIDE.as_secs_f32()).min(1.0);
                // Ease-out cubic, the same curve the sidebar's width slide uses:
                // fast where the eye is following, settling where it lands.
                let eased = 1.0 - (1.0 - t).powi(3);
                let height = start + (target - start) * eased;
                let alive = this.update(cx, |view, cx| {
                    view.find_height = height;
                    cx.notify();
                });
                if alive.is_err() || t >= 1.0 {
                    break;
                }
            }
        }));
    }

    /// Show a parked multi-line paste and ask whether it should be sent.
    ///
    /// The preview is the clipboard as it arrived, in the monospace family, scrollable:
    /// the question "is this what you meant to paste" is answered by reading it, and a
    /// preview that wrapped or reflowed would be a different text from the one that is
    /// about to be sent. A paste too long to read in full is cut with a note — the point
    /// is to recognise it, not to read every line.
    fn review_paste(&mut self, paste: PendingPaste, window: &mut Window, cx: &mut Context<Self>) {
        const PREVIEW_LIMIT: usize = 4000;
        let view = cx.entity();
        let large = terminal::paste_requires_large_review(&paste.text);
        let shown = if paste.text.len() > PREVIEW_LIMIT {
            // Cut on a char boundary: slicing at a fixed byte offset panics on
            // multibyte text, and a long CJK paste lands mid-character almost
            // every time (the remote-output → select → paste chain).
            let mut end = PREVIEW_LIMIT;
            while end > 0 && !paste.text.is_char_boundary(end) {
                end -= 1;
            }
            let mut cut = paste.text[..end].to_string();
            cut.push_str(&crate::i18n::t(
                "\n…(内容过长,已截断显示)",
                "\n…(truncated for review)",
            ));
            cut
        } else {
            paste.text.clone()
        };
        let bytes = paste.bytes;
        let lines = paste.text.lines().count();

        Root::update(window, cx, move |root, window, cx| {
            root.open_dialog(
                move |dialog, _window, cx| {
                    let send = bytes.clone();
                    let view = view.clone();
                    dialog
                        .title(crate::i18n::t("确认多行粘贴", "Confirm multi-line paste"))
                        // Only the callbacks: a `Dialog` renders these, not its buttons,
                        // so the visible ones are in the footer. See `super::dialogs`.
                        .button_props(
                            DialogButtonProps::default()
                                .on_ok(move |_, _window, cx| {
                                    let _ = view.update(cx, |view, cx| {
                                        view.send_bytes(&send);
                                        cx.notify();
                                    });
                                    true
                                })
                                // Cancelling sends nothing at all, which is the whole
                                // point: a backdrop click must not accept a paste.
                                .on_cancel(|_, _, _| true),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .max_w(px(720.))
                                .child(div().text_sm().child(SharedString::from(format!(
                                    "{}{}",
                                    crate::i18n::t(
                                        "粘贴的内容包含换行,可能一次执行多条命令。",
                                        "The pasted text contains line breaks and may run \
                                             several commands."
                                    ),
                                    if lines > 0 {
                                        format!(
                                            " ({})",
                                            match crate::i18n::t("行", "lines") {
                                                "行" => format!("{lines} 行"),
                                                other => format!("{lines} {other}"),
                                            }
                                        )
                                    } else {
                                        String::new()
                                    },
                                ))))
                                .child(
                                    div()
                                        .id("paste-preview")
                                        .when(large, |this| this.h(px(360.)))
                                        .when(!large, |this| this.max_h(px(200.)))
                                        .w_full()
                                        .overflow_y_scroll()
                                        .p_2()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().muted)
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_xs()
                                        .child(SharedString::from(shown.clone())),
                                ),
                        )
                        // The action row: a plain `Dialog` draws no buttons of its own, so
                        // without this the only way to answer is the keyboard.
                        .footer(super::answer_footer(
                            SharedString::from(crate::i18n::t("粘贴", "Paste")),
                            false,
                            SharedString::from(crate::i18n::t("取消", "Cancel")),
                        ))
                },
                window,
                cx,
            );
        });
    }

    /// Read the clipboard and paste it into the session.
    ///
    /// Separate from `send_key` because the platform delivers a paste as a clipboard
    /// item through `EntityInputHandler::paste` on some paths and as a keystroke on
    /// others, and both have to end in the same encoder. Ctrl+V and Ctrl+Shift+V are
    /// bound here; the trait method covers the rest.
    /// The opt-in paste pair: Ctrl+Alt+V and Shift+Insert. When the setting is
    /// off, the chord means what it always meant to the program on the other end
    /// of the PTY, so the bytes it would have sent are sent instead — the same
    /// fall-through the pre-action handler had.
    fn paste_alternate(
        &mut self,
        action: &crate::ui::PasteAlternate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.paste_shortcuts {
            self.paste_from_clipboard(window, cx);
            return;
        }
        let bytes = if action.insert {
            terminal::key_to_pty_bytes("insert", false, false, false)
        } else {
            terminal::key_to_pty_bytes("v", true, true, false)
        };
        self.send_bytes(&bytes);
        cx.notify();
    }

    /// Open the find bar. What is typed goes into the bar's own input rather than
    /// being read from here: a terminal has no text to search by keystroke.
    fn open_find(&mut self, cx: &mut Context<Self>) {
        self.find_open = true;
        self.animate_find_to(FIND_BAR_HEIGHT, cx);
    }

    fn paste_from_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = item.text() else {
            return;
        };
        let bracketed = self
            .buffer
            .lock()
            .map(|buffer| buffer.parser.screen().bracketed_paste())
            .unwrap_or(false);
        // A multi-line paste is several commands and a shell runs all of them, so it is
        // reviewed first when the setting says so.
        if terminal::paste_needs_review(&text, self.paste_confirm) {
            // Opened here rather than parked for `render`, which is what the auth prompts
            // do and what this first did: those arrive from a session's pump, which has no
            // window at all, while a paste arrives from a key or a click — and a dialog
            // opened while the shell is rendering lands in `Root`'s queue after `Root` has
            // already drawn its layer, which is a dialog nobody ever sees.
            let bytes = terminal::encode_pasted_text(&text, bracketed);
            self.review_paste(PendingPaste { text, bytes }, window, cx);
            return;
        }
        let bytes = terminal::encode_pasted_text(&text, bracketed);
        self.send_bytes(&bytes);
    }
}

/// Translate the shell's name for a special key into the text the shared encoder reads.
///
/// The encoder reads a special key as a Unicode Private Use Area character —
/// `\u{F700}` for Up, and so on — while the shell reports the same keys as words:
/// `"up"`, `"enter"`, `"escape"`. Handed `"enter"`, the encoder does exactly what it
/// is documented to do with an unrecognised single word and sends it as literal
/// text, which is how the first run of this view ended with the characters
/// `enterenter` sitting at a PowerShell prompt.
///
/// The mapping is the whole of the difference between the two key vocabularies, and
/// it lives here rather than in the encoder because the PUA set is a fact about
/// `key_to_pty_bytes`: one adapter is cheaper than a second copy of the control-byte
/// and modifier logic, and if the encoder ever learns the words directly this
/// function is the one thing to delete.
///
/// `enter`, `tab`, `escape` and `space` map to the characters they actually are
/// rather than to a PUA code, because the encoder already handles them correctly by
/// value — which is also why they need a mapping at all: without one they would be
/// sent as their own names like `"enter"` was.
fn gpui_key_name(key: &str) -> Option<&'static str> {
    Some(match key {
        "up" => "\u{F700}",
        "down" => "\u{F701}",
        "left" => "\u{F702}",
        "right" => "\u{F703}",
        "home" => "\u{F729}",
        "end" => "\u{F72B}",
        "pageup" => "\u{F72C}",
        "pagedown" => "\u{F72D}",
        "delete" => "\u{F728}",
        "f1" => "\u{F704}",
        "f2" => "\u{F705}",
        "f3" => "\u{F706}",
        "f4" => "\u{F707}",
        "f5" => "\u{F708}",
        "f6" => "\u{F709}",
        "f7" => "\u{F70A}",
        "f8" => "\u{F70B}",
        "f9" => "\u{F70C}",
        "f10" => "\u{F70D}",
        "f11" => "\u{F70E}",
        "f12" => "\u{F70F}",
        "backspace" => "\u{0008}",
        "enter" => "\n",
        "tab" => "\t",
        "escape" => "\u{001b}",
        "space" => " ",
        _ => return None,
    })
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// How typed text and IME composition reach the terminal.
///
/// This is not optional, and it is the subtlest part of the whole view. GPUI's
/// Windows backend delivers a typed character through `WM_CHAR` into
/// `InputHandler::replace_text_in_range`, *not* through `KeyDownEvent`: the latter
/// carries only the key and the modifiers, and `key_char` is populated for
/// characters the layout can name. Handling only `KeyDownEvent`, as the first
/// version of this view did, gives a terminal where Enter, Backspace and the arrow
/// keys work perfectly and no letter ever arrives — and where Chinese is not merely
/// broken but impossible, because a composition has no single key event to hang on.
///
/// The terminal has no editable document: keystrokes go to the PTY and the screen
/// comes back as output. The one exception is an IME composition, which is text the
/// user has not committed yet — it is held in `composition`, drawn at the cursor, and
/// sent only when the platform commits it through `replace_text_in_range`. Everything
/// else the read-side methods report is that composition and nothing more, because
/// inventing text here would let a composition rewrite the session's own output.
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        _adjusted: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        // Only the composition is a document. Everything else on the grid is the
        // session's output, which the IME has no business rewriting — reporting it as
        // editable text would let a composition splice itself into scrollback.
        let text = self.composition.as_deref()?;
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let start = range.start.min(utf16.len());
        let end = range.end.min(utf16.len()).max(start);
        String::from_utf16(&utf16[start..end]).ok()
    }

    /// Where the caret is, which the platform needs before it will place its
    /// composition or candidate window.
    ///
    /// Returning `None` here — as this view did first — is not a small omission. The
    /// Windows backend asks this question first in `retrieve_caret_position`, and a
    /// `None` short-circuits the whole of `WM_IME_STARTCOMPOSITION` handling: the
    /// candidate window stays wherever the platform last put it and never tracks the
    /// terminal's cursor.
    ///
    /// The document is the composition, so the caret is at its end. With no composition
    /// the document is empty and the caret is at 0 — which is still an answer, and still
    /// enough for the platform to place a window at the cursor.
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let caret = self
            .composition
            .as_deref()
            .map_or(0, |text| text.encode_utf16().count());
        Some(UTF16Selection {
            range: caret..caret,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let len = self
            .composition
            .as_deref()
            .map(|text| text.encode_utf16().count())?;
        Some(0..len)
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // The composition ended without committing a character — Escape, or the IME
        // cancelling. Nothing was sent to the PTY, so dropping it is the whole undo.
        let (_, held) = terminal::CompositionEvent::Cancel.apply();
        if self.composition.take().is_some() {
            self.snapshot.composition = None;
            cx.notify();
        }
        self.composition = held;
    }

    /// A character, a paste, or the committed result of a composition. The one method
    /// that types.
    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // With the find bar open the keyboard belongs to it, not to the session. The
        // bar is the focused thing in the sense that matters — the user opened it to
        // type a query — and forwarding those characters to the PTY would type the
        // query into the shell instead of into the box.
        if self.find_open {
            self.find_query.push_str(text);
            self.apply_find_query(cx);
            return;
        }
        // The range is ignored rather than honoured: it indexes a text buffer this
        // view does not have. A terminal's input is a stream, so there is nothing
        // to replace — the PTY's own line editor owns everything that has been
        // typed.
        //
        // This is also where a composition commits: the Windows backend reports the
        // IME's `GCS_RESULTSTR` through this call. The policy that decides what that
        // means — and that a preedit must not be sent — is `CompositionEvent`.
        let (bytes, held) = terminal::CompositionEvent::Commit(text).apply();
        // Repainted only when a composition was actually on screen: this method is also
        // the ordinary typing path, and an unconditional `notify` there would repaint the
        // window on every keystroke for nothing.
        if self.composition.take().is_some() {
            self.snapshot.composition = None;
            cx.notify();
        }
        self.composition = held;
        self.send_bytes(&bytes);
    }

    /// IME composition: the text the user is still choosing between.
    ///
    /// Nothing is sent from here. The first version of this method forwarded the text
    /// whenever the platform omitted the selection range, on the theory that a missing
    /// range meant the composition had committed — but the platform omits it for an
    /// ordinary preedit update too, so every intermediate pinyin state reached the shell
    /// before the user had chosen a character. The contract is in `gpui-base`'s own
    /// implementation and its tests: preedit arrives here, the commit arrives through
    /// `replace_text_in_range`.
    ///
    /// Held and drawn at the cursor instead, which is where a native terminal shows it
    /// and what makes the candidate window's context visible.
    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (_, held) = terminal::CompositionEvent::Prelude(new_text).apply();
        self.composition = held;
        // Straight into the snapshot, because a composition produces no PTY output and so
        // no `refresh` would ever arrive to pick it up.
        self.snapshot.composition = self.composition.clone();
        cx.notify();
    }

    /// Where the platform should draw its composition and candidate windows.
    ///
    /// Answered from the terminal's own cursor rather than from a text buffer, because
    /// that is where the user is looking: the grid's cursor is the insertion point in
    /// every sense that matters to the person typing. `_range` is ignored for the same
    /// reason the ranges above are — the document is the composition, not the screen.
    ///
    /// This is what `retrieve_caret_position` needs: without it the candidate window
    /// appears at a stale position, or not at all.
    fn bounds_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let metrics = self.metrics?;
        let cell = |col: i32, row: i32| {
            gpui_kit::point(
                element_bounds.origin.x + metrics.width * col as f32,
                element_bounds.origin.y + metrics.height * row as f32,
            )
        };
        let (row, col) = self.composition_cell();
        let origin = cell(col, row);
        Some(Bounds::new(
            origin,
            gpui_kit::size(metrics.width, metrics.height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui_kit::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        // The composition is at the cursor and nowhere else, so every point maps to its
        // end. A real answer matters to the platform's click-to-position handling; for a
        // one-line composition at a fixed cell there is only one position to report.
        Some(
            self.composition
                .as_deref()
                .map_or(0, |text| text.encode_utf16().count()),
        )
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        true
    }

    /// A paste, encoded by the shared rules rather than the trait's default.
    ///
    /// The default implementation calls `replace_text_in_range` with the raw clipboard
    /// text, which for a terminal is wrong twice over: it sends LF where a shell's line
    /// editor expects CR, and it skips bracketed paste — the `\x1b[200~…\x1b[201~`
    /// wrapper that tells a program the bytes arrived all at once, so an editor inserts
    /// them instead of treating each newline as a command. Both rules already exist in
    /// `crate::terminal`, so this is a call and not a second implementation.
    fn paste(
        &mut self,
        item: gpui_kit::ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = item.text() else {
            return;
        };
        // Whether the remote asked for bracketed paste is the parser's answer, not this
        // view's guess: it is set by a CSI sequence the program sent.
        let bracketed = self
            .buffer
            .lock()
            .map(|buffer| buffer.parser.screen().bracketed_paste())
            .unwrap_or(false);
        // The platform's paste and the keyboard's take the same route once the text is in
        // hand, review included: a paste that skipped the question on one path would be a
        // setting that depends on how the text arrived.
        if terminal::paste_needs_review(&text, self.paste_confirm) {
            let bytes = terminal::encode_pasted_text(&text, bracketed);
            self.review_paste(PendingPaste { text, bytes }, window, cx);
            return;
        }
        let bytes = terminal::encode_pasted_text(&text, bracketed);
        self.send_bytes(&bytes);
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Measured here rather than at construction: the text system is only
        // reachable with an `App`, so the first render is the earliest point one
        // exists. Cached afterwards, because `measure` resolves a font id and reads
        // its metrics.
        let metrics = *self.metrics.get_or_insert_with(|| {
            CellMetrics::measure(cx, &self.family, self.font_size, self.line_spacing)
        });

        // The grid is sized to the area this view actually has, not to the window.
        //
        // A pane is narrower than the window, and it is the whole window minus the other panes
        // when there is a split. Sized from the window, a terminal claims columns it cannot show
        // — the session wraps its output past the edge of its own pane — and a second pane would
        // claim the window as well and be drawn off the screen. `grid_bounds` is what paint
        // measured last frame; the first frame has nothing measured yet and falls back to the
        // window, which the next frame corrects.
        let area = self
            .grid_bounds
            .get()
            .map(|bounds| bounds.size)
            .unwrap_or_else(|| window.viewport_size());
        let (cols, rows) = metrics.grid_size(area);
        self.sync_size(cols, rows);

        let theme = cx.theme();
        // The terminal is dark whatever the window's theme is: a terminal is read for a
        // long time and in long columns, and light-on-dark is the convention every
        // terminal shares. The colour comes from the shared palette rather than from a
        // literal here, so the pane and the grid cannot disagree about it — a seam at the
        // edge of the grid is what a second answer looks like.
        let background = rgba_to_hsla(terminal::terminal_background(true));
        // What the settings say this grid looks like, gathered in one value so the
        // painter takes a description rather than six arguments.
        let style = GridStyle {
            family: self.family.clone(),
            font_size: self.font_size,
            bold: self.bold,
            line_spacing: self.line_spacing,
            cursor: self.cursor_style,
            cursor_color: self.cursor_color,
        };
        // The colour a cursor falls back to: the theme's foreground, which is what the
        // terminal's text is drawn in.
        // The cursor's default is plain white, in both window themes: the
        // grid it is drawn on is terminal-dark either way, and a block cursor
        // reads best at full contrast. An explicit colour in settings still
        // overrides it.
        let cursor = gpui_kit::white();

        // A prompt parked by `apply_event` gets its window here, which is the only
        // place one exists. Answered or not, the slot is cleared: the policy in
        // `crate::session::prompt_queue` holds any further prompt until this one is
        // resolved, so nothing is lost by taking it.
        if let Some(prompt) = self.pending_prompt.take() {
            prompt.open(window, cx);
        }

        // Captured before the grid is built, because the paint closure receives an
        // `App` rather than a `Context<Self>` and cannot make one. The handle is
        // what `handle_input` needs to route text back into this view, and the focus
        // handle is what it checks before accepting.
        let this = cx.entity();
        let focus = self.focus.clone();
        // The grid's painted bounds, passed out of the paint closure. A `Cell`
        // because paint runs with an `App`, where no entity update is possible —
        // and read back below, in the same frame, once paint has set it.
        let grid_bounds_cell = self.grid_bounds.clone();

        div()
            .id("terminal")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(background)
            .track_focus(&self.focus)
            // The role, and the reason this line exists: `track_focus` alone registers a
            // focus handle, and an accessibility client that asks what has focus is told
            // "something in this window" rather than which thing. GPUI logs it as
            // "it has an id but no role" and falls back to announcing the whole window.
            // `Terminal` is the role accesskit defines for exactly this widget — it maps
            // to UIA's `Document` control type on Windows, which is what a screen reader
            // expects a terminal's text to be.
            .role(Role::Terminal)
            .aria_label("Terminal")
            .key_context("Terminal")
            // The bound chords arrive here as actions — `impls/actions.rs` is where
            // their keystrokes live — and everything the keymap cannot express
            // (state-gated Escape, alt-screen paging) still reaches `send_key`.
            .on_action(cx.listener(|this, _: &crate::ui::CopySelection, _, cx| {
                this.copy_selection(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::ui::Paste, window, cx| {
                this.paste_from_clipboard(window, cx);
            }))
            .on_action(cx.listener(
                |this, action: &crate::ui::PasteAlternate, window, cx| {
                    this.paste_alternate(action, window, cx);
                },
            ))
            .on_action(cx.listener(|this, _: &crate::ui::Find, _, cx| {
                this.open_find(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::ui::ZoomIn, _, cx| {
                this.zoom_font(1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::ui::ZoomOut, _, cx| {
                this.zoom_font(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::ui::ZoomReset, _, cx| {
                this.zoom_font(0, cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.send_key(event, window, cx);
                // Notified rather than waiting for the PTY to echo: a keystroke that
                // produced no output would otherwise leave the cursor where it was
                // until something else redrew.
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                // A program that asked for the mouse wants the wheel too: inside btop
                // or htop the wheel scrolls the program's own panel, and scrolling our
                // scrollback instead would do nothing visible on the alternate screen.
                // Shift still hands it back to us, as it does for the buttons.
                if this.snapshot.mouse_tracked && !event.modifiers.shift {
                    let cell_height = this
                        .metrics
                        .map_or(16.0, |metrics| f32::from(metrics.height));
                    let lines = match event.delta {
                        ScrollDelta::Lines(point) => point.y,
                        ScrollDelta::Pixels(point) => f32::from(point.y) / cell_height,
                    };
                    // Wheel up is xterm's 64, wheel down is 65, and one report per
                    // notch: a program's wheel handling is discrete, so a fractional
                    // delta is rounded to the nearest notch rather than accumulated.
                    if lines != 0.0 {
                        let button = if lines > 0.0 { 64 } else { 65 };
                        let notches = lines.abs().round().max(1.0) as usize;
                        for _ in 0..notches {
                            this.report_mouse(0, button, event.position, false);
                        }
                    }
                    return;
                }
                // One conversion, in one place. GPUI reports a wheel-*up* as a positive
                // y — measured, not assumed: ten notches arrived as `Lines(y: 30.0)` —
                // and scrolling up in a terminal means moving back into the scrollback,
                // which is an increasing `view_offset`. So the sign passes through
                // unchanged, and a caller reading this does not have to work out which
                // way round it goes.
                let cell_height = this
                    .metrics
                    .map_or(16.0, |metrics| f32::from(metrics.height));
                let lines = match event.delta {
                    // Already in lines, and fractional for a trackpad.
                    ScrollDelta::Lines(point) => point.y,
                    // Pixels, which the cell height turns into fractional lines — the
                    // same unit the momentum policy banks.
                    ScrollDelta::Pixels(point) => f32::from(point.y) / cell_height,
                };
                this.scroll(lines, cx);
            }))
            .when_some(self.status_line(), |this, status| {
                // One line above the grid, not an overlay: a status that covered
                // the first row would hide the very output it is describing. While
                // the tab is actually dialling — a status text exists and the
                // phase is still connecting — a spinner rides in front of it,
                // because "connecting" as still words reads exactly like "hung".
                // The phase alone is not enough: a tab that was never connected
                // is also in phase 0, and "Not connected" must not spin.
                this.child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .when(self.conn_state == 0 && self.status.is_some(), |this| {
                            this.child(
                                Spinner::new()
                                    .with_size(Size::Small)
                                    .color(cx.theme().muted_foreground),
                            )
                        })
                        .child(status),
                )
            })
            .when(self.find_height > 0.5, |this| {
                // One line above the grid, like the status line and for the same reason:
                // a bar drawn over the grid would cover the first row of the very output
                // it is searching. It shows the query and the hit count, because a find
                // box that only shows what was typed cannot tell the user whether the
                // search found anything. Drawn while the height is above zero, not while
                // `find_open`: the bar leaves by the same slide it arrived by, so a
                // closing bar is still on screen, shrinking, until the slide is done.
                // Without the clip the bar would reveal at full height inside a growing
                // slot; with it, the grid sees one height change per tick instead of a
                // jump that shoves the session.
                let hits = self.snapshot.find_matches.len();
                let border = cx.theme().border;
                let muted = cx.theme().muted_foreground;
                let height = px(self.find_height);
                this.child(
                    div()
                        .w_full()
                        .flex_shrink_0()
                        .h(height)
                        .overflow_hidden()
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .px_3()
                                .py_1()
                                .border_b_1()
                                .border_color(border)
                                .text_sm()
                                .child(
                                    gpui_kit::component::Icon::new(
                                        gpui_kit::assets::IconName::Search,
                                    )
                                    .size_4()
                                    .text_color(muted),
                                )
                                .child(SharedString::from(self.find_query.clone()))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        // The count, not just a match indicator: "3" tells the
                                        // user how much is off screen, and "0" is the difference
                                        // between a wrong query and a scrolled view.
                                        .child(SharedString::from(format!("{hits}"))),
                                ),
                        ),
                )
            })
            // The grid, inset a few pixels from the pane's edge when the setting
            // asks: output that starts flush at the corner reads as clipped. The
            // inset is the box the grid fills, so every geometry that flows from
            // the painted bounds — the click-to-cell conversion, the selection
            // and match rectangles, the scrollbar — inherits the same origin and
            // stays in step for free. The box is the positioned element itself
            // rather than a padded wrapper, because padding does not offset an
            // absolutely positioned child, and a wrapper that only holds one is
            // a box of zero height.
            .child(div().flex_1().relative().child(
                div()
                    .absolute()
                    .when(self.padding, |this| {
                        this.top(px(6.)).left(px(10.)).right(px(4.)).bottom(px(4.))
                    })
                    .when(!self.padding, |this| this.inset_0())
                    .child(terminal_grid(
                self.snapshot.clone(),
                metrics,
                style,
                cursor,
                background,
                move |bounds, window, cx| {
                    // Registered during paint because that is the only phase
                    // `handle_input` accepts, and only while focused. This is
                    // what makes typed characters and IME composition arrive at
                    // `replace_text_in_range` instead of nowhere.
                    window.handle_input(&focus, ElementInputHandler::new(bounds, this.clone()), cx);
                    // And the same phase is the only place the grid's bounds are
                    // known, which is what turns a later click into a cell.
                    // `View::update` cannot come from here, so the bounds are
                    // handed to the view through a shared cell instead.
                    let old_size = grid_bounds_cell.get().map(|old| old.size);
                    grid_bounds_cell.set(Some(bounds));
                    // A dock toggle can resize an otherwise idle terminal.
                    // Its PTY size is read on the next render, so request that
                    // frame instead of waiting for keyboard input or output.
                    if old_size != Some(bounds.size) {
                        window.request_animation_frame();
                    }
                },
            )),
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    // Any press re-takes keyboard focus, so typing after a click
                    // goes to the terminal rather than to whatever had focus.
                    window.focus(&this.focus, cx);
                    // The program's event, if it asked for the mouse. Shift is the
                    // escape hatch that keeps a local selection possible inside a
                    // full-screen program.
                    if this.report_mouse(0, 0, event.position, event.modifiers.shift) {
                        return;
                    }
                    this.begin_selection_at(
                        event.position,
                        event.modifiers.control,
                        event.modifiers.shift,
                        cx,
                    );
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, _, window, cx| {
                    // The X11 habit, behind the setting for the same reason as the other
                    // extras: a program that wants the middle button for itself would
                    // otherwise get a paste it never asked for.
                    if this.paste_shortcuts {
                        this.paste_from_clipboard(window, cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                // Only while a button is held: a move with nothing pressed is the
                // pointer crossing the grid, which is not a selection.
                if event.pressed_button.is_some() {
                    // A drag inside a mouse-tracking program is the program's, and on
                    // the alternate screen there is no scrollback of ours to select
                    // from anyway.
                    if this.report_mouse(2, 0, event.position, event.modifiers.shift) {
                        return;
                    }
                    this.extend_selection_at(event.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    if this.report_mouse(1, 0, event.position, event.modifiers.shift) {
                        return;
                    }
                    match event.click_count {
                        // A double click selects the word under the pointer, a triple
                        // click the whole line — the macOS Terminal baseline. Both
                        // replace whatever the press started, because the press ran
                        // first and began a one-cell selection.
                        2 => this.select_word_at(event.position, cx),
                        3 => this.select_line_at(event.position, cx),
                        _ => this.finish_selection(cx),
                    }
                }),
            )
            .into_any_element()
    }
}
/// Drain the window's UI messages until the channel closes.
///
/// One path for every session kind: the local PTY feeds this today and a real SSH
/// session feeds the same queue through [`GpuiEventSink`], so the drawing and the
/// render-gate protocol are exercised by the simplest case before the hardest one
/// arrives.
///
/// The refresh and the gate settle happen together and in that order. A producer
/// blocked on a render ticket is waiting for the frame it asked for to have been
/// built, so settling the gate before the snapshot is taken would release it into a
/// terminal that still shows the previous screen — which is precisely the drift the
/// gate exists to prevent.
/// Hand a terminal's highlighting settings to its buffer.
///
/// The buffer is where output is marked up: `TermBuffer::render` colours a run it
/// recognises before anything is painted, so this is the only place the settings have to
/// reach. A poisoned lock is ignored rather than fatal — the next frame renders unmarked
/// output, which is worse to look at but not a reason to take the window down.
fn apply_highlighting(buffer: &Arc<Mutex<TermBuffer>>, appearance: &TerminalSettings) {
    if let Ok(mut buffer) = buffer.lock() {
        buffer.output_highlight = appearance.highlight;
        buffer.custom_highlight_rules = crate::terminal::compile_output_rules(&appearance.rules);
    }
}

async fn pump_messages(
    this: gpui_kit::WeakEntity<TerminalView>,
    cx: &mut gpui_kit::AsyncApp,
    mut messages: tokio::sync::mpsc::UnboundedReceiver<UiMessage>,
    buffer: Arc<Mutex<TermBuffer>>,
    sftp_listings: crate::core::SftpListings,
    sftp_trees: super::session_state::TabTrees,
    statuses: crate::resource::TabStatuses,
    transfers: crate::core::TransferRecords,
    tunnels: super::session_state::TabTunnels,
    opened_file: Arc<Mutex<Option<super::session_state::OpenedFile>>>,
) {
    while let Some(message) = messages.recv().await {
        let update = match message {
            UiMessage::Render { gate } => this.update(cx, |view, cx| {
                view.refresh();
                cx.notify();
                // Settled after the snapshot, and whether or not this tab is the
                // one on screen: a hidden tab still has to release its producer, or
                // a background session's pump blocks forever on a frame nobody
                // asked to see.
                if let Some(through) = gate.begin_flush() {
                    gate.finish_flush(through, true);
                }
            }),
            UiMessage::Events { tab_id, events } => this.update(cx, |view, cx| {
                for event in events {
                    apply_event(
                        &opened_file,
                        view,
                        cx,
                        &buffer,
                        &sftp_listings,
                        &sftp_trees,
                        &statuses,
                        &transfers,
                        &tunnels,
                        &tab_id,
                        event,
                    );
                }
                view.refresh();
                cx.notify();
            }),
        };
        // A failure means the window is gone, and the buffer goes with it — which is
        // what closes the session.
        if update.is_err() {
            break;
        }
    }
}

/// Apply one session event to this view.
///
/// The framework-neutral parts — the terminal buffer, the SFTP listings, the shared
/// maps — are written here directly, so the state a future view reads is the same
/// state this one leaves behind rather than a parallel copy of it.
fn apply_event(
    opened_file: &Arc<Mutex<Option<super::session_state::OpenedFile>>>,

    view: &mut TerminalView,
    cx: &mut gpui_kit::Context<TerminalView>,
    buffer: &Arc<Mutex<TermBuffer>>,
    sftp_listings: &crate::core::SftpListings,
    sftp_trees: &super::session_state::TabTrees,
    statuses: &crate::resource::TabStatuses,
    transfers: &crate::core::TransferRecords,
    tunnels: &super::session_state::TabTunnels,
    tab_id: &str,
    event: SessionEvent,
) {
    match event {
        SessionEvent::Output(chunk) => {
            if let Ok(mut buffer) = buffer.lock() {
                // `ingest` is where the DEC graphics charset translation, the
                // OSC/CSI state machine and the scroll detection live — the
                // session's configured encoding was already decoded to UTF-8 at
                // the pump. Its return value is the bytes the terminal would
                // answer a query with; there is no channel back to the PTY for
                // those yet, so a program asking for the cursor position gets no
                // answer rather than a wrong one.
                let _ = buffer.ingest(chunk.as_bytes());
            }
        }
        SessionEvent::Status(text) => {
            // The first thing a connect attempt does is send a "connecting" status —
            // and a status arriving while this view records a closed session is the
            // reconnect attempt doing the same. Either way the phase is "dialling"
            // again, which is what keeps the status line's spinner honest.
            if view.conn_state == 2 {
                view.conn_state = 0;
            }
            view.status = Some(text);
        }
        // The tunnel list is shared rather than held here: the panel that shows it is a
        // separate view, and this is the one place the events arrive. Written under the
        // tab id so a second tab's tunnels cannot be read as this one's.
        SessionEvent::TunnelUpdate(rows) => {
            if let Ok(mut tunnels) = tunnels.lock() {
                tunnels.insert(tab_id.to_string(), rows);
            }
        }
        SessionEvent::Connected => {
            // The tab's own state, which is what the tab strip's dot reads. Recorded
            // here rather than inferred from the status text, so the dot is right even
            // when nothing is drawing a status line.
            if let Ok(mut statuses) = statuses.lock() {
                if let Some(status) = statuses.get_mut(tab_id) {
                    status.state = 1;
                }
            }
            // The tab strip's dot lives on the page, which is cached between its
            // own notifications: a transition is a rare event, and telling the
            // page is what repaints the chip.
            if let Some(page) = view.page.as_ref() {
                if let Some(page) = page.upgrade() {
                    let _ = page.update(cx, |_, cx| cx.notify());
                }
            }
            view.conn_state = 1;
            view.status = Some(crate::i18n::t("已连接", "Connected").to_string())
        }
        SessionEvent::Closed(reason) => {
            // A closed session is not a live one. State 2 is the "disconnected" state,
            // and the tab strip reads it to stop showing a green dot over a shell that
            // is gone.
            if let Ok(mut statuses) = statuses.lock() {
                if let Some(status) = statuses.get_mut(tab_id) {
                    status.state = 2;
                }
            }
            // As above: the strip's greyed dot waits on the page being told.
            if let Some(page) = view.page.as_ref() {
                if let Some(page) = page.upgrade() {
                    let _ = page.update(cx, |_, cx| cx.notify());
                }
            }
            // Release the heavy scrollback first: a disconnected tab is kept for
            // the reconnect path, and holding a firehose's worth of history while
            // idle is the allocation this exists to drop.
            if let Ok(mut buffer) = buffer.lock() {
                buffer.release_scrollback();
                // Printed into the terminal rather than only into the status line,
                // because a terminal that has gone quiet with no explanation reads
                // as a hang. Colour codes so it stands out from the shell's output.
                let _ = buffer.ingest(
                    format!(
                        "\r\n\x1b[31m{}\x1b[0m\r\n",
                        crate::i18n::t(
                            "连接已断开,按 Enter 重新连接",
                            "Disconnected — press Enter to reconnect"
                        )
                    )
                    .as_bytes(),
                );
            }
            view.conn_state = 2;
            view.status = Some(format!(
                "{} — {reason}",
                crate::i18n::t("已断开", "Disconnected")
            ));
        }
        // The listing events go into the shared store, which is the authority the panel
        // projects from. Writing them anywhere else would leave the panel showing one
        // directory while the session believes another.
        SessionEvent::CwdChanged(path) => {
            if let Ok(mut map) = sftp_listings.lock() {
                map.entry(tab_id.to_string()).or_default().set_path(path);
            }
        }
        SessionEvent::SftpEntries { path, entries } => {
            if let Ok(mut map) = sftp_listings.lock() {
                map.entry(tab_id.to_string())
                    .or_default()
                    .load(path, &entries);
            }
        }
        // The tree, on the same terms and for the same reason: the session owns which
        // directories are open, and the panel is drawn by the shell rather than by this view.
        SessionEvent::SftpTreeUpdate(nodes) => {
            if let Ok(mut map) = sftp_trees.lock() {
                map.insert(tab_id.to_string(), nodes);
            }
            // The tree lands in its own store, but the panel reads both through
            // one sync — bump the listing's generation so the tree the session
            // just rebuilt reaches the panel on the next frame too.
            if let Ok(mut map) = sftp_listings.lock() {
                map.entry(tab_id.to_string()).or_default().touch();
            }
        }
        // The built-in viewer's text: the shell opens the view, because a view needs a
        // window and this handler has only the view it draws. So the text is left here for
        // the shell to find, the same hand-off the listing above uses.
        SessionEvent::SftpFileText {
            path,
            name,
            content,
            edit,
            error,
        } => {
            if let Ok(mut slot) = opened_file.lock() {
                *slot = Some(super::session_state::OpenedFile {
                    path,
                    name,
                    content,
                    editable: edit,
                    error,
                });
            }
        }
        SessionEvent::SftpStatus(text) => {
            view.status = Some(text);
        }
        SessionEvent::SftpError(text) => {
            // The text goes to the status line, and the listing's generation is
            // bumped so the panel's spinner — set when the request went out —
            // ends on this event rather than on its timeout. An error that
            // leaves the spinner turning reads as "still trying", which is a
            // lie.
            view.status = Some(text);
            if let Ok(mut map) = sftp_listings.lock() {
                map.entry(tab_id.to_string()).or_default().touch();
            }
        }
        // Transfer progress goes into the shared store, which is the authority the
        // transfer manager projects. Every row's identity, order and completion state is
        // decided there — a second copy on a view would be a second answer to "how far
        // along is this download".
        SessionEvent::SftpTransfer {
            id,
            name,
            is_upload,
            transferred,
            total,
            state,
            msg,
        } => {
            if let Ok(mut store) = transfers.lock() {
                store.upsert(crate::core::Transfer::from_event(
                    id,
                    name,
                    is_upload,
                    transferred,
                    total,
                    state,
                    msg,
                ));
            }
        }
        // The three auth prompts need a `Window` and an `App` to open a dialog, and
        // this function has neither — it runs inside a view update. So the prompt is
        // parked on the view and opened by the next `render`, which has both. The
        // alternative would be threading a window through the whole event path for the
        // sake of three rare events.
        SessionEvent::HostKeyPrompt {
            host,
            port,
            key_type,
            fingerprint,
            changed,
            responder,
        } => {
            view.pending_prompt = Some(PendingPrompt::HostKey {
                host,
                port,
                key_type,
                fingerprint,
                changed,
                responder,
            });
        }
        SessionEvent::CredentialPrompt {
            session_id,
            host,
            user,
            need_user,
            need_password,
            responder,
        } => {
            view.pending_prompt = Some(PendingPrompt::Credential {
                session_id,
                host,
                user,
                need_user,
                need_password,
                responder,
            });
        }
        SessionEvent::MfaPrompt {
            session_id,
            host,
            prompt,
            echo,
            responder,
        } => {
            view.pending_prompt = Some(PendingPrompt::Mfa {
                session_id,
                host,
                prompt,
                echo,
                responder,
            });
        }
        // The remote machine's sample and its process table go into the shared per-tab
        // status, which is the authority the resource panel projects — the same store
        // both panels read, so they describe one measurement.
        // Recorded here rather than in the panel because this runs once per sample
        // whether or not the panel is on screen, and a history that only advanced while
        // someone was looking would have holes in it.
        SessionEvent::ResourceStats {
            cpu_percent,
            mem_used_kib,
            mem_total_kib,
            swap_used_kib,
            swap_total_kib,
            net,
            disks,
            current_user: _,
            procs: _,
            sys,
        } => {
            if let Ok(mut statuses) = statuses.lock() {
                if let Some(status) = statuses.get_mut(tab_id) {
                    status.cpu = cpu_percent;
                    status.mem_used_kib = mem_used_kib;
                    status.mem_total_kib = mem_total_kib;
                    status.swap_used_kib = swap_used_kib;
                    status.swap_total_kib = swap_total_kib;
                    status.net = net;
                    status.disks = disks;
                    if let Some(sys) = sys {
                        status.sys = sys;
                    }
                    // A sample means the monitor channel is alive, which is a stronger
                    // statement than "the socket is up": it is what makes the tab's dot
                    // green even if the `Connected` event was missed.
                    if status.state != 1 {
                        status.state = 1;
                    }
                    // The view's own phase reads the same fact, so a session whose
                    // `Connected` event was missed still stops its status-line spinner.
                    view.conn_state = 1;
                    // Append the selected interface's total rate to its own sparkline.
                    let (_, rx, tx) = crate::resource::selected_iface(status);
                    crate::resource::push_ring(&mut status.net_hist, (rx + tx) as f32);
                }
            }
        }
        // Process stats arrive on their own channel, which is what keeps a slow `ps` from
        // freezing the resource sample. The store they are written into is the one the
        // process window reads, so recording them here is what gives that window its rows.
        SessionEvent::ProcessStats {
            current_user,
            procs,
        } => {
            if let Ok(mut statuses) = statuses.lock() {
                if let Some(status) = statuses.get_mut(tab_id) {
                    if !current_user.is_empty() {
                        status.user = current_user;
                    }
                    status.procs = procs;
                }
            }
        }
        // Tunnels and the editor events belong to views this shell does not have yet;
        // #18 draws them.
        _ => {}
    }
}

/// A prompt waiting for a window to be shown in.
///
/// Parked on the view by [`apply_event`], which has no window, and opened by
/// [`TerminalView::render`], which does. Carries the responder, because answering it is
/// the whole point and the prompt is worthless without it.
pub(crate) enum PendingPrompt {
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        changed: bool,
        responder: HostKeyResponder,
    },
    Credential {
        session_id: String,
        host: String,
        user: String,
        need_user: bool,
        need_password: bool,
        responder: CredentialResponder,
    },
    Mfa {
        session_id: String,
        host: String,
        prompt: String,
        echo: bool,
        responder: MfaResponder,
    },
}

impl PendingPrompt {
    /// Open the dialog for this prompt, whatever kind it is.
    fn open(self, window: &mut Window, cx: &mut gpui_kit::App) {
        match self {
            Self::HostKey {
                host,
                port,
                key_type,
                fingerprint,
                changed,
                responder,
            } => super::auth_dialogs::show_host_key(
                host,
                port,
                key_type,
                fingerprint,
                changed,
                responder,
                window,
                cx,
            ),
            Self::Credential {
                session_id,
                host,
                user,
                need_user,
                need_password,
                responder,
            } => super::auth_dialogs::show_credential(
                session_id,
                host,
                user,
                need_user,
                need_password,
                responder,
                window,
                cx,
            ),
            Self::Mfa {
                session_id,
                host,
                prompt,
                echo,
                responder,
            } => {
                super::auth_dialogs::show_mfa(session_id, host, prompt, echo, responder, window, cx)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the PTY receives for a GPUI-named key.
    fn bytes_for(key: &str) -> Vec<u8> {
        match gpui_key_name(key) {
            Some(named) => terminal::key_to_pty_bytes(named, false, false, false),
            None => panic!("{key} is not a named key"),
        }
    }

    #[test]
    fn a_named_key_becomes_its_sequence_and_never_its_own_name() {
        // The bug this exists for: pressing Enter at a real prompt put the seven
        // letters of "enter" on the screen, because the shared encoder treats an
        // unrecognised word as text. Every assertion here is really the same one —
        // that the name did not survive to the PTY.
        assert_eq!(bytes_for("enter"), vec![0x0d], "a PTY wants CR, not LF");
        assert_eq!(bytes_for("backspace"), vec![0x7f]);
        assert_eq!(bytes_for("escape"), vec![0x1b]);
        assert_eq!(bytes_for("tab"), vec![0x09]);
        assert_eq!(bytes_for("space"), vec![0x20]);
        assert_eq!(bytes_for("up"), b"\x1b[A".to_vec());
        assert_eq!(bytes_for("delete"), b"\x1b[3~".to_vec());
        assert_eq!(bytes_for("f5"), b"\x1b[15~".to_vec());

        for key in [
            "enter",
            "backspace",
            "escape",
            "tab",
            "space",
            "up",
            "delete",
            "f5",
        ] {
            let bytes = bytes_for(key);
            assert!(
                !bytes.starts_with(key.as_bytes()) || bytes.len() == key.len(),
                "{key} leaked its own name as {bytes:?}"
            );
        }
    }

    #[test]
    fn a_printable_key_is_not_a_named_key_so_key_char_is_used() {
        // `a` has no name mapping, which is what routes it to `key_char` — the
        // character the layout would type, and the correct source for text on a
        // layout whose labels differ from its output.
        assert_eq!(gpui_key_name("a"), None);
        assert_eq!(gpui_key_name("1"), None);
        assert_eq!(gpui_key_name("nonsense"), None);
    }

    /// Settings with the given highlighter, and nothing else that matters here.
    fn appearance_with(
        highlight: crate::terminal::OutputHighlightPreset,
        rules: Vec<crate::config::OutputHighlightRule>,
    ) -> TerminalSettings {
        TerminalSettings {
            family: SharedString::from("test"),
            font_size: 13,
            bold: true,
            padding: false,
            line_spacing: 1.0,
            cursor_style: CursorStyle::Block,
            cursor_color: None,
            highlight,
            rules,
            review_multiline_paste: true,
            paste_shortcuts: true,
        }
    }

    fn rule(pattern: &str, color: &str) -> crate::config::OutputHighlightRule {
        crate::config::OutputHighlightRule {
            pattern: pattern.to_string(),
            regex: false,
            case_sensitive: false,
            whole_line: false,
            color: color.to_string(),
            enabled: true,
        }
    }

    #[test]
    fn the_buffer_is_given_the_highlighter_the_settings_ask_for() {
        // The bug this exists for: the highlighter lives in the buffer — `render` colours
        // a run it recognises before anything is painted — and a shell that never set it
        // drew plain output while the settings page said otherwise.
        let buffer = Arc::new(Mutex::new(TermBuffer::new(24, 80)));
        let appearance = appearance_with(
            crate::terminal::OutputHighlightPreset::DevOps,
            vec![rule("ERROR", "red")],
        );

        apply_highlighting(&buffer, &appearance);

        let buffer = buffer.lock().expect("the lock is not poisoned in a test");
        assert_eq!(
            buffer.output_highlight,
            crate::terminal::OutputHighlightPreset::DevOps
        );
        assert_eq!(
            buffer.custom_highlight_rules.len(),
            1,
            "the rule has to reach the buffer as a matcher, not as text"
        );
    }

    #[test]
    fn a_disabled_highlighter_means_no_rules_and_no_preset() {
        let buffer = Arc::new(Mutex::new(TermBuffer::new(24, 80)));
        let appearance = appearance_with(crate::terminal::OutputHighlightPreset::Off, Vec::new());

        apply_highlighting(&buffer, &appearance);

        let buffer = buffer.lock().expect("the lock is not poisoned in a test");
        assert_eq!(
            buffer.output_highlight,
            crate::terminal::OutputHighlightPreset::Off
        );
        assert!(buffer.custom_highlight_rules.is_empty());
    }
}
