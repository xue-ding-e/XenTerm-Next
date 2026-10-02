#[cfg(test)]
use crate::terminal::TermBuffers;

/// What an IME composition event means for a terminal.
///
/// A terminal has no editable buffer, so the platform is the only thing that knows
/// which text the user has actually chosen. This is the whole of that judgement, kept
/// as a value rather than inline in the view because getting it wrong is silent and
/// expensive: the first version forwarded any composition event that arrived without a
/// selection range, on the theory that a missing range meant "committed". The platform
/// sends that same shape for an ordinary preedit update, so every intermediate pinyin
/// state — `n`, `ni`, `nin` — was typed into the shell before the user had picked a
/// character.
///
/// The inputs are the two calls the platform makes. A preedit is a composition; a
/// commit is text. They are separate calls, and treating them as one is the bug.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompositionEvent<'a> {
    /// The text the user is still choosing between. Drawn, not sent.
    Prelude(&'a str),
    /// The chosen text, and the only thing that goes to the PTY.
    Commit(&'a str),
    /// The composition was abandoned — Escape, or the IME cancelling it.
    Cancel,
}

impl<'a> CompositionEvent<'a> {
    /// The bytes this event sends to the PTY, and the composition it leaves behind.
    ///
    /// Returns both, because the two are decided together and a caller that took only
    /// the bytes would keep drawing a composition that is already gone.
    pub(crate) fn apply(&self) -> (Vec<u8>, Option<String>) {
        match self {
            // Not sent. The user is mid-choice and a PTY cannot un-type what it has
            // already echoed.
            CompositionEvent::Prelude(text) => {
                (Vec::new(), (!text.is_empty()).then(|| text.to_string()))
            }
            // Sent, and the composition is spent: the text that was drawn has been
            // replaced by the text that was chosen.
            CompositionEvent::Commit(text) => (text.as_bytes().to_vec(), None),
            // Dropped without being sent, which is the whole undo.
            CompositionEvent::Cancel => (Vec::new(), None),
        }
    }
}

/// Normalize clipboard line endings to the single CR byte expected for Enter
/// by a terminal, including inside bracketed-paste payloads.
pub(crate) fn normalize_pasted_newlines(text: &str) -> String {
    text.replace("\r\n", "\r").replace('\n', "\r")
}

/// Encode a terminal mouse event for the PTY using the encoding the remote
/// application requested (X10 / SGR) — so btop, htop, mc and other mouse-aware
/// TUI apps get click/drag/wheel events (see #terminal-mouse).
///
/// `btn` follows the xterm conventions (a vt100 `CellMouseButton`):
///   0/1/2 = left / middle / right button press,
///   32    = motion with no button,
///   35    = motion with a button held,
///   64/65 = wheel up / down.
/// `release` marks a button-release event (only meaningful for `btn` 0–2):
/// X10 encodes it as `btn + 3`, SGR keeps the same code but ends the report
/// with a lowercase `m`.
///
/// Coordinates are 1-based grid cells clamped to the screen, matching how the
/// remote draws its UI. `cols`/`rows` are the *screen* dimensions (not the
/// visible viewport) so the report always points at the same cell the program
/// rendered.
pub(crate) fn encode_mouse_event(
    btn: u8,
    release: bool,
    col: i32,
    row: i32,
    cols: u16,
    rows: u16,
    encoding: vt100::MouseProtocolEncoding,
) -> Vec<u8> {
    let c = (col.clamp(0, cols.saturating_sub(1) as i32) as u16 + 1).clamp(1, 223);
    let r = (row.clamp(0, rows.saturating_sub(1) as i32) as u16 + 1).clamp(1, 223);
    match encoding {
        vt100::MouseProtocolEncoding::Sgr => {
            let final_byte = if release { b'm' } else { b'M' };
            format!("\x1b[<{btn};{c};{r}{}", final_byte as char).into_bytes()
        }
        _ => {
            let cb = btn as u16 + if release { 3 } else { 0 } + 32;
            vec![0x1b, b'[', b'M', cb as u8, (c + 32) as u8, (r + 32) as u8]
        }
    }
}

/// Encode a command-bar submission and return the optional non-empty history
/// entry separately. An empty bar still represents an Enter key press (#307),
/// but must not add a blank command to persistent history.
///
/// The line ends with a carriage return, not a line feed, because that is what a
/// terminal's Enter key is: `normalize_pasted_newlines` says the same thing for a paste,
/// and the key itself sends CR. It matters most for a local Windows shell — ConPTY hands
/// the byte to PSReadLine, where CR is Enter and LF *inserts a newline* — so a command
/// bar that sent LF left PowerShell waiting at its continuation prompt instead of running
/// the command. A remote tty maps CR to NL on input, so both frontends work either way;
/// only one of the two bytes works everywhere.
pub(crate) fn encode_command_bar_input(command: &str) -> (Option<String>, Vec<u8>) {
    let command = command.trim_end().to_string();
    let mut bytes = command.clone().into_bytes();
    bytes.push(b'\r');
    let history = (!command.is_empty()).then_some(command);
    (history, bytes)
}

/// Whether a paste has to be shown for review before it reaches the session.
///
/// A pasted block with newlines in it is several commands, and a shell runs all of them:
/// the setting exists because the clipboard is not always what the user thinks it is, and
/// because a paste that ends in a newline executes as soon as it lands.
///
/// A very large paste is reviewed regardless of the setting (#262): a hundred kilobytes
/// arriving at a prompt is not something to discover afterwards, and at that size the
/// preview is the only way to see what is about to happen.
pub(crate) const PASTE_REVIEW_LIMIT: usize = 100 * 1024;

/// Whether a paste has to be shown for review before it reaches the session.
pub(crate) fn paste_needs_review(text: &str, confirm_multiline: bool) -> bool {
    let multi_line = text.contains(['\r', '\n']);
    (multi_line && confirm_multiline) || text.len() > PASTE_REVIEW_LIMIT
}

pub(crate) fn encode_pasted_text(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = normalize_pasted_newlines(text);
    if !bracketed {
        return normalized.into_bytes();
    }

    // Do not allow pasted content to forge the bracketed-paste terminator or
    // inject Ctrl+C while the remote application is accepting the payload.
    let filtered = normalized.replace(['\x1b', '\x03'], "");
    let mut bytes = Vec::with_capacity(filtered.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(filtered.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

#[cfg(test)]
pub(crate) fn terminal_uses_bracketed_paste(bufs: &TermBuffers, tab_id: &str) -> bool {
    let buffer = bufs
        .lock()
        .ok()
        .and_then(|buffers| buffers.get(tab_id).cloned());
    buffer
        .and_then(|buffer| {
            buffer
                .lock()
                .ok()
                .map(|buffer| buffer.parser.screen().bracketed_paste())
        })
        .unwrap_or(false)
}

pub(crate) fn paste_requires_large_review(text: &str) -> bool {
    const COMPACT_CHAR_LIMIT: usize = 600;
    const COMPACT_LINE_LIMIT: usize = 12;
    let bytes = text.as_bytes();
    let mut lines = 1usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                lines += 1;
                if bytes.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
            }
            b'\n' => lines += 1,
            _ => {}
        }
        index += 1;
    }
    text.chars().count() > COMPACT_CHAR_LIMIT || lines > COMPACT_LINE_LIMIT
}

pub(crate) fn should_drop_bare_ctrl_marker(key: &str, ctrl: bool, workaround: bool) -> bool {
    workaround
        && ctrl
        && matches!(
            key.chars().collect::<Vec<_>>().as_slice(),
            ['\u{0011}'] | ['\u{0016}']
        )
}

/// winit may deliver Ctrl+C either as ETX with `ctrl=true` or as the
/// already-translated ETX byte after the modifier flag has been cleared.
/// In both forms it is a real terminal interrupt and must reach the PTY.
pub(crate) fn is_terminal_interrupt(key: &str) -> bool {
    key == "\u{0003}"
}

#[cfg(target_os = "linux")]
pub(crate) fn bare_ctrl_marker_workaround_enabled() -> bool {
    // winit can expose a physical Control press as U+0011 or U+0016 on
    // Linux. This was first observed on Debian (#274) and is now confirmed on
    // Fedora as well (#369), so it is a backend/platform behaviour rather than
    // a distribution-specific quirk. The final letter event still generates
    // genuine Ctrl+Q/Ctrl+V bytes through `key_to_pty_bytes`.
    true
}

#[cfg(target_os = "macos")]
pub(crate) fn bare_ctrl_marker_workaround_enabled() -> bool {
    // Some macOS 26.5 devices repeat U+0017 while physical Control is held.
    // Without filtering it, nano receives Ctrl+W (search) before Ctrl+X (#312).
    true
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn bare_ctrl_marker_workaround_enabled() -> bool {
    false
}

pub(crate) fn key_to_pty_bytes(key: &str, ctrl: bool, alt: bool, app_cursor: bool) -> Vec<u8> {
    let special: Option<&[u8]> = match key {
        "\u{F700}" => Some(if app_cursor { b"\x1bOA" } else { b"\x1b[A" }),
        "\u{F701}" => Some(if app_cursor { b"\x1bOB" } else { b"\x1b[B" }),
        "\u{F702}" => Some(if app_cursor { b"\x1bOD" } else { b"\x1b[D" }),
        "\u{F703}" => Some(if app_cursor { b"\x1bOC" } else { b"\x1b[C" }),
        "\u{F729}" => Some(if app_cursor { b"\x1bOH" } else { b"\x1b[H" }),
        "\u{F72B}" => Some(if app_cursor { b"\x1bOF" } else { b"\x1b[F" }),
        "\u{F72C}" => Some(b"\x1b[5~"),
        "\u{F72D}" => Some(b"\x1b[6~"),
        "\u{007F}" | "\u{F728}" => Some(b"\x1b[3~"),
        "\u{F704}" => Some(b"\x1bOP"),
        "\u{F705}" => Some(b"\x1bOQ"),
        "\u{F706}" => Some(b"\x1bOR"),
        "\u{F707}" => Some(b"\x1bOS"),
        "\u{F708}" => Some(b"\x1b[15~"),
        "\u{F709}" => Some(b"\x1b[17~"),
        "\u{F70A}" => Some(b"\x1b[18~"),
        "\u{F70B}" => Some(b"\x1b[19~"),
        "\u{F70C}" => Some(b"\x1b[20~"),
        "\u{F70D}" => Some(b"\x1b[21~"),
        "\u{F70E}" => Some(b"\x1b[23~"),
        "\u{F70F}" => Some(b"\x1b[24~"),
        _ => None,
    };
    if let Some(sequence) = special {
        return sequence.to_vec();
    }

    if key == "\u{0008}" {
        return vec![0x7f];
    }
    if key == "\n" && !ctrl && !alt {
        return vec![0x0d];
    }
    if key.is_empty() {
        return Vec::new();
    }

    if let Some(character) = key.chars().next() {
        let codepoint = character as u32;
        if key.chars().count() == 1 && !ctrl && (0x10..=0x18).contains(&codepoint) {
            return Vec::new();
        }
    }

    if ctrl {
        if let Some(character) = key.chars().next() {
            let codepoint = character as u32;
            if key.chars().count() == 1 && (0x01..=0x1f).contains(&codepoint) {
                return vec![codepoint as u8];
            }
        }
        if let Some(character) = key.chars().next() {
            if key.chars().count() == 1 {
                let upper = character.to_ascii_uppercase() as u8;
                let control = match upper {
                    b'A'..=b'Z' => Some(upper - b'A' + 1),
                    b'[' => Some(0x1b),
                    b'\\' => Some(0x1c),
                    b']' => Some(0x1d),
                    b'^' => Some(0x1e),
                    b'_' => Some(0x1f),
                    b'@' => Some(0x00),
                    _ => None,
                };
                if let Some(byte) = control {
                    return vec![byte];
                }
            }
        }
    }

    if key
        .chars()
        .any(|character| (0xE000..=0xF8FF).contains(&(character as u32)))
    {
        return Vec::new();
    }
    if alt && !ctrl {
        let mut bytes = vec![0x1b];
        bytes.extend_from_slice(key.as_bytes());
        return bytes;
    }
    key.as_bytes().to_vec()
}

#[cfg(test)]
mod composition_tests {
    use super::*;

    /// The bug this whole type exists for.
    ///
    /// Every intermediate pinyin state must stay out of the PTY. The first version sent
    /// any composition event whose selection range was absent, which is exactly what a
    /// preedit update looks like — so `n`, `ni`, `nin` were typed into the shell and the
    /// user was left with whatever their shell did with three stray letters.
    #[test]
    fn a_preedit_never_reaches_the_pty() {
        for text in ["n", "ni", "nin", "ni hao"] {
            let (bytes, held) = CompositionEvent::Prelude(text).apply();
            assert!(bytes.is_empty(), "preedit {text:?} must not be typed");
            assert_eq!(held.as_deref(), Some(text), "but it is held for drawing");
        }
    }

    /// The commit is the only thing that types, and it types exactly once.
    #[test]
    fn a_commit_types_its_text_and_clears_the_composition() {
        let (bytes, held) = CompositionEvent::Commit("你好").apply();
        assert_eq!(bytes, "你好".as_bytes());
        assert_eq!(
            held, None,
            "the composition has been replaced by its result"
        );
    }

    /// An abandoned composition sends nothing and leaves nothing to draw.
    #[test]
    fn an_abandoned_composition_sends_nothing() {
        let (bytes, held) = CompositionEvent::Cancel.apply();
        assert!(bytes.is_empty());
        assert_eq!(held, None);
    }

    /// Empty preedit text is the IME clearing its composition, which is not a
    /// composition to draw.
    #[test]
    fn an_emptied_preedit_stops_being_drawn() {
        let (bytes, held) = CompositionEvent::Prelude("").apply();
        assert!(bytes.is_empty());
        assert_eq!(held, None);
    }

    /// The whole sequence, in the order the platform sends it: pinyin, refine, then the
    /// chosen character committed through a *different* call. This is the shape the
    /// toolkit's own undo test uses, and it is what the terminal must see.
    #[test]
    fn a_full_pinyin_composition_types_only_the_chosen_character() {
        let mut sent = Vec::new();
        let mut held = None;

        for preedit in ["n", "ni", "nin"] {
            let (bytes, next) = CompositionEvent::Prelude(preedit).apply();
            sent.extend_from_slice(&bytes);
            held = next;
        }
        assert_eq!(held.as_deref(), Some("nin"));

        let (bytes, next) = CompositionEvent::Commit("您").apply();
        sent.extend_from_slice(&bytes);
        held = next;

        assert_eq!(sent, "您".as_bytes(), "only the committed character");
        assert_eq!(held, None);
    }
}

#[cfg(test)]
mod mouse_encoding_tests {
    use super::*;

    #[test]
    fn x10_left_press() {
        // Left press (0) at cell (1,1) → Cb=32+0=32 → ESC [ M sp ! !
        let bytes = encode_mouse_event(
            0,
            false,
            0,
            0,
            80,
            24,
            vt100::MouseProtocolEncoding::Default,
        );
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 33, 33]);
    }

    #[test]
    fn x10_left_release() {
        // Release adds 3 to the button code → Cb=32+3=35.
        let bytes =
            encode_mouse_event(0, true, 0, 0, 80, 24, vt100::MouseProtocolEncoding::Default);
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 35, 33, 33]);
    }

    #[test]
    fn x10_second_column_and_row() {
        // Cell (1,2) → Cx=33+... wait 2 → 34; row 1 → 33.
        let bytes = encode_mouse_event(
            0,
            false,
            1,
            0,
            80,
            24,
            vt100::MouseProtocolEncoding::Default,
        );
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 34, 33]);
    }

    #[test]
    fn sgr_left_press() {
        let bytes = encode_mouse_event(0, false, 0, 0, 80, 24, vt100::MouseProtocolEncoding::Sgr);
        assert_eq!(bytes, b"\x1b[<0;1;1M");
    }

    #[test]
    fn sgr_left_release_uses_lowercase_m() {
        let bytes = encode_mouse_event(0, true, 0, 0, 80, 24, vt100::MouseProtocolEncoding::Sgr);
        assert_eq!(bytes, b"\x1b[<0;1;1m");
    }

    #[test]
    fn x10_wheel() {
        let bytes = encode_mouse_event(
            64,
            false,
            5,
            3,
            80,
            24,
            vt100::MouseProtocolEncoding::Default,
        );
        assert_eq!(
            bytes,
            vec![0x1b, b'[', b'M', 64 + 32, 5 + 1 + 32, 3 + 1 + 32]
        );
    }

    #[test]
    fn sgr_wheel() {
        let bytes = encode_mouse_event(64, false, 5, 3, 80, 24, vt100::MouseProtocolEncoding::Sgr);
        assert_eq!(bytes, b"\x1b[<64;6;4M");
    }

    #[test]
    fn coordinates_clamped_to_screen() {
        // Negative / out-of-range columns clamp into the grid.
        let bytes =
            encode_mouse_event(0, false, -5, 999, 80, 24, vt100::MouseProtocolEncoding::Sgr);
        assert_eq!(bytes, b"\x1b[<0;1;24M");
    }

    #[test]
    fn x10_drag_motion() {
        let bytes = encode_mouse_event(
            32,
            false,
            2,
            2,
            80,
            24,
            vt100::MouseProtocolEncoding::Default,
        );
        assert_eq!(
            bytes,
            vec![0x1b, b'[', b'M', 32 + 32, 2 + 1 + 32, 2 + 1 + 32]
        );
    }

    #[test]
    fn a_single_line_paste_is_never_reviewed() {
        // The setting exists for pastes that run commands. A path or a URL with no newline
        // in it executes nothing, and asking about it would train the user to click
        // through the prompt that matters.
        assert!(!paste_needs_review("/etc/nginx/nginx.conf", true));
        assert!(!paste_needs_review("", true));
    }

    #[test]
    fn a_multi_line_paste_is_reviewed_only_while_the_setting_is_on() {
        assert!(paste_needs_review("one\ntwo", true));
        assert!(paste_needs_review("one\r\ntwo", true));
        assert!(!paste_needs_review("one\ntwo", false));
    }

    #[test]
    fn a_very_large_paste_is_reviewed_even_with_the_setting_off() {
        // A hundred kilobytes arriving at a prompt is not something to discover
        // afterwards, and the setting is not consent to that.
        let huge = "x".repeat(PASTE_REVIEW_LIMIT + 1);
        assert!(paste_needs_review(&huge, false));
    }
}
