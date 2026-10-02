use crate::terminal::{
    build_row, cell_prefix, char_after_cell_end, char_at_cell_start, detect_scroll,
    highlight_plain_output, render_term_span, BuiltScreen, CsiState, Line, TermBuffer, TermMatch,
    MAX_HISTORY, RAW_CAP,
};

/// Lines of scrollback the `vt100::Parser` itself retains: none.
///
/// The parser's scrollback is a `rows + scrollback` grid of `Cell`s that nothing
/// here reads: scrolling, selection and find all work off this module's own
/// `history` deque (capped at [`MAX_HISTORY`]), and resize reflow replays `raw`
/// through a fresh parser rather than asking the parser to retain anything. At
/// 5000 lines that was a large dead allocation per tab — a `Cell` is a `Vec`
/// plus attributes, so 5000 lines x 200 columns is on the order of 40 MB — and
/// `0` is what `vt100`'s own `Default` uses.
///
/// The two buffers stay separate on purpose: the deque stores runs, which is
/// cheaper per line than a cell grid, and it is the one the UI scrolls through.
pub(crate) const PARSER_SCROLLBACK: usize = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalQuery {
    Status,
    CursorPosition { private: bool },
    PrimaryDeviceAttributes,
}

fn terminal_query(sequence: &[u8]) -> Option<TerminalQuery> {
    match sequence {
        b"\x1b[5n" => Some(TerminalQuery::Status),
        b"\x1b[6n" => Some(TerminalQuery::CursorPosition { private: false }),
        b"\x1b[?6n" => Some(TerminalQuery::CursorPosition { private: true }),
        b"\x1b[c" | b"\x1b[0c" => Some(TerminalQuery::PrimaryDeviceAttributes),
        _ => None,
    }
}

/// `CSI 3 J` — "erase saved lines".
const ERASE_SAVED: [u8; 4] = [0x1b, b'[', b'3', b'J'];

/// Longest OSC payload buffered before the rest of the sequence is dropped.
///
/// `vte` caps its own OSC buffer at 1024 bytes, so a longer one could not survive
/// the parser anyway; dropping it keeps a truncated sequence out of the replay
/// ring, which a resize would otherwise feed back through the parser.
pub(crate) const OSC_CAP: usize = 4096;

/// Longest CSI parameter string buffered before it is handed to the parser.
///
/// Long but valid sequences exist — an SGR run carrying several RGB colours can
/// pass 64 bytes — so this is deliberately generous. Past it the buffered prefix
/// is forwarded and this scanner gives up on the sequence, which is exactly what
/// the parser is for.
const CSI_CAP: usize = 1024;

/// Byte index just past the last `CSI 3 J` in `tail` followed by `bytes`.
///
/// `tail` is the tail of the already-retained replay stream, so a sequence split
/// across two reads is still found. The two slices are read as one window
/// instead of being concatenated, so the search allocates nothing.
pub(crate) fn find_last_erase_saved(tail: &[u8], bytes: &[u8]) -> Option<usize> {
    let merged = tail.len() + bytes.len();
    if merged < ERASE_SAVED.len() {
        return None;
    }
    let byte_at = |index: usize| {
        if index < tail.len() {
            tail[index]
        } else {
            bytes[index - tail.len()]
        }
    };
    (0..=merged - ERASE_SAVED.len())
        .rev()
        .find(|&start| {
            (0..ERASE_SAVED.len()).all(|offset| byte_at(start + offset) == ERASE_SAVED[offset])
        })
        .map(|start| start + ERASE_SAVED.len())
}

impl TermBuffer {
    /// A fresh buffer with an empty screen of the given size.
    ///
    /// A constructor rather than a struct literal at each call site because there
    /// are twenty fields and three of them are state machines that must start in a
    /// particular state: a `charset` mid-designation, a `csi_state` that is not
    /// `Normal` or a non-empty `raw` queue would all be a buffer that mis-renders
    /// its first byte. Callers go through this rather than building the literal
    /// inline, so the initialisation has one place to get wrong instead of many.
    pub(crate) fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, PARSER_SCROLLBACK),
            find_query: String::new(),
            is_dark: false,
            output_highlight: crate::terminal::OutputHighlightPreset::Off,
            custom_highlight_rules: Vec::new(),
            json_format_output: false,
            vt100_drawing: false,
            charset: crate::terminal::CharsetTracker::default(),
            interactive_echo_until: std::time::Instant::now(),
            sel_anchor: None,
            sel_focus: None,
            sel_ranges: Vec::new(),
            mouse_tracked: false,
            history: std::collections::VecDeque::new(),
            prev: Vec::new(),
            view_offset: 0,
            scroll_accum: 0.0,
            displayed_text: Vec::new(),
            csi_state: CsiState::Normal,
            csi_pending: Vec::new(),
            raw: Vec::new(),
        }
    }

    /// Release all retained terminal output and recreate the parser at the
    /// current size. This is used when a session is disconnected or the user
    /// explicitly clears the terminal, so a dead tab does not keep a large
    /// scrollback allocation alive until it is closed.
    pub(crate) fn release_scrollback(&mut self) {
        let (rows, cols) = self.parser.screen().size();
        self.parser = vt100::Parser::new(rows, cols, PARSER_SCROLLBACK);
        self.find_query.clear();
        self.history = std::collections::VecDeque::new();
        self.prev = Vec::new();
        self.view_offset = 0;
        self.sel_anchor = None;
        self.sel_focus = None;
        self.sel_ranges.clear();
        self.displayed_text = Vec::new();
        self.csi_state = CsiState::Normal;
        self.csi_pending = Vec::new();
        self.raw = Vec::new();
        self.charset = crate::terminal::CharsetTracker::default();
        self.mouse_tracked = false;
    }

    // ---- Absolute-coordinate selection helpers (#18 follow-up) -------------
    //
    // The "combined" buffer is `history` (oldest first) followed by the live
    // screen rows.  A visible window of `rows` rows looks at a slice of it whose
    // top index depends on whether we're at the live bottom or scrolled up.

    /// Live screen rows plus the count of non-blank ones at the top.
    fn live_rows(&self) -> (Vec<Line>, usize) {
        let s = self.parser.screen();
        let (rows, cols) = s.size();
        let live: Vec<Line> = (0..rows).map(|r| build_row(s, r, cols)).collect();
        let used = live
            .iter()
            .rposition(|(_, runs, _)| !runs.is_empty())
            .map(|i| i + 1)
            .unwrap_or(0);
        (live, used)
    }

    /// Absolute combined-row index of the top visible row for the current view.
    fn view_top_abs(&self) -> usize {
        let rows = self.parser.screen().size().0 as usize;
        let hist_len = self.history.len();
        if self.view_offset == 0 {
            // Live view: visible row 0 is live screen row 0 = combined[hist_len].
            hist_len
        } else {
            // Include the screen's full row count (trailing blanks too) so this
            // mapping matches render()'s scroll window — keeping the live and
            // scrolled views continuous after a shrink/grow (#119-followup).
            let combined_len = hist_len + rows;
            combined_len.saturating_sub(rows + self.view_offset)
        }
    }

    /// Map a visible row (0..rows) to its absolute combined-row index.
    ///
    /// O(1) on purpose: this runs on every pointer move of a drag, and it used to
    /// rebuild the whole screen — every cell of every row — only to feed a row
    /// count into a mapping that ignores it.
    pub(crate) fn vis_to_abs(&self, vis_row: u16) -> usize {
        self.view_top_abs() + vis_row as usize
    }

    /// Highlight rectangles for the current selection, clipped to the visible
    /// window of the current view.
    pub(crate) fn selection_rects_visible(&self, cols: u16) -> Vec<TermMatch> {
        let ranges = if self.sel_ranges.is_empty() {
            match (self.sel_anchor, self.sel_focus) {
                (Some(anchor), Some(focus)) => vec![(anchor, focus)],
                _ => Vec::new(),
            }
        } else {
            self.sel_ranges.clone()
        };
        if ranges.is_empty() {
            return Vec::new();
        }
        // The same O(1) mapping as `vis_to_abs`: this is read on every frame of a
        // drag, so it must not rebuild the screen to answer.
        let top = self.view_top_abs();
        let rows = self.parser.screen().size().0;
        let mut out = Vec::new();
        for ((ar, ac), (fr, fc)) in ranges {
            let (lo_r, lo_c, hi_r, hi_c) = if (ar, ac) <= (fr, fc) {
                (ar, ac, fr, fc)
            } else {
                (fr, fc, ar, ac)
            };
            if (lo_r, lo_c) == (hi_r, hi_c) {
                continue;
            }
            for vis in 0..rows {
                let abs = top + vis as usize;
                if abs < lo_r || abs > hi_r {
                    continue;
                }
                let (c0, c1) = if abs == lo_r && abs == hi_r {
                    (lo_c.min(hi_c), lo_c.max(hi_c))
                } else if abs == lo_r {
                    (lo_c, cols.saturating_sub(1))
                } else if abs == hi_r {
                    (0, hi_c)
                } else {
                    (0, cols.saturating_sub(1))
                };
                out.push(TermMatch::new(
                    vis as i32,
                    c0 as i32,
                    (c1.saturating_sub(c0) + 1) as i32,
                ));
            }
        }
        out
    }
}

/// Highlight rectangles for every case-insensitive occurrence of `query` in `rows`.
///
/// The body of [`TermBuffer::find_matches`], as a free function so a caller holding
/// only the rendered rows — what a view has — can search them without keeping a
/// buffer. One implementation, because two would be two answers to where a match
/// sits after a wide glyph.
pub(crate) fn find_matches_in_rows(rows: &[String], query: &str) -> Vec<TermMatch> {
    let needle: Vec<char> = query.to_ascii_lowercase().chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (row, line) in rows.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        // Lowercased per character rather than on the whole string, so the indices stay
        // aligned with `chars` — a case fold that changed a character's length would
        // shift every column after it.
        let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
        let prefix = cell_prefix(&chars);
        let mut at = 0usize;
        while at + needle.len() <= lower.len() {
            if lower[at..at + needle.len()] == needle[..] {
                let col = prefix[at] as i32;
                let len = (prefix[at + needle.len()] - prefix[at]) as i32;
                out.push(TermMatch::new(row as i32, col, len));
                // Past the whole match rather than one character into it: overlapping
                // hits of one query read as one hit, and stepping by one would draw two
                // rectangles over the same text for "aa" in "aaa".
                at += needle.len();
            } else {
                at += 1;
            }
        }
    }
    out
}

impl TermBuffer {
    /// Set the search query, and scroll to the first match when the query is new.
    ///
    /// The query and the scrolling are set together rather than in two calls, because a
    /// find box that does not move the view looks like a find box that does nothing
    /// whenever the match is off screen.
    pub(crate) fn set_find_query(&mut self, query: &str) {
        if self.find_query == query {
            return;
        }
        self.find_query = query.to_string();
        if !query.is_empty() {
            self.scroll_to_first_find_match(query);
        }
    }

    /// Highlight rectangles for the current query, in grid cells.
    ///
    /// Columns are GRID columns rather than character indices, so wide CJK glyphs count
    /// as two and a highlight lines up over the text after them (#132).
    pub(crate) fn find_matches(&self) -> Vec<TermMatch> {
        find_matches_in_rows(&self.displayed_text, &self.find_query)
    }

    /// Whether a search is active, so a caller can skip the work when none is.
    pub(crate) fn has_find_query(&self) -> bool {
        !self.find_query.is_empty()
    }

    /// If the current find query is outside the visible window, jump to the
    /// first matching row in scrollback/live content so old serial output can be
    /// found without manually scrolling back first (#233).
    pub(crate) fn scroll_to_first_find_match(&mut self, query: &str) -> bool {
        if query.is_empty() || self.parser.screen().alternate_screen() {
            return false;
        }
        let q = query.to_lowercase();
        let (live, _) = self.live_rows();
        let rows = self.parser.screen().size().0 as usize;
        let hist_len = self.history.len();
        let combined_len = hist_len + live.len();
        let Some(match_idx) = self
            .history
            .iter()
            .map(|line| &line.0)
            .chain(live.iter().map(|line| &line.0))
            .position(|line| line.to_lowercase().contains(&q))
        else {
            return false;
        };
        let top = match_idx.min(combined_len.saturating_sub(rows));
        let new_offset = combined_len.saturating_sub(rows + top);
        if self.view_offset == new_offset {
            return false;
        }
        self.view_offset = new_offset;
        true
    }

    /// Extract the selected text from the combined buffer (whole selection,
    /// even the parts currently scrolled out of view).
    pub(crate) fn selection_has_extent(&self) -> bool {
        if self.sel_ranges.is_empty() {
            return matches!(
                (self.sel_anchor, self.sel_focus),
                (Some(anchor), Some(focus)) if anchor != focus
            );
        }
        self.sel_ranges
            .iter()
            .any(|(anchor, focus)| anchor != focus)
    }

    pub(crate) fn clear_selection(&mut self) {
        self.sel_anchor = None;
        self.sel_focus = None;
        self.sel_ranges.clear();
    }

    pub(crate) fn extract_selection_text(&self) -> String {
        let ranges = if self.sel_ranges.is_empty() {
            match (self.sel_anchor, self.sel_focus) {
                (Some(anchor), Some(focus)) => vec![(anchor, focus)],
                _ => Vec::new(),
            }
        } else {
            self.sel_ranges.clone()
        };
        if ranges.is_empty() {
            return String::new();
        }
        ranges
            .iter()
            .map(|&(anchor, focus)| self.extract_range_text(anchor, focus))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Select the shell-oriented word at a visible grid position and return it.
    /// Paths, host names and flags stay together; whitespace and shell control
    /// punctuation delimit words (#287).
    pub(crate) fn select_word_at(&mut self, row: u16, col: u16) -> Option<String> {
        let line = self.displayed_text.get(row as usize)?;
        let chars: Vec<char> = line.chars().collect();
        let prefix = cell_prefix(&chars);
        let at = char_at_cell_start(&prefix, col as usize);
        let ch = *chars.get(at)?;
        let is_word = |c: char| {
            !c.is_whitespace()
                && !matches!(
                    c,
                    '\'' | '"'
                        | '`'
                        | '|'
                        | '&'
                        | ';'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '<'
                        | '>'
                        | ','
                )
        };
        if !is_word(ch) {
            return None;
        }
        let mut start = at;
        while start > 0 && is_word(chars[start - 1]) {
            start -= 1;
        }
        let mut end = at + 1;
        while end < chars.len() && is_word(chars[end]) {
            end += 1;
        }
        let abs_row = self.vis_to_abs(row);
        let start_col = prefix[start].min(u16::MAX as usize) as u16;
        let end_col = prefix[end].saturating_sub(1).min(u16::MAX as usize) as u16;
        let range = ((abs_row, start_col), (abs_row, end_col));
        self.sel_ranges.clear();
        self.sel_ranges.push(range);
        self.sel_anchor = Some(range.0);
        self.sel_focus = Some(range.1);
        Some(chars[start..end].iter().collect())
    }

    /// Begin a drag-selection at a visible grid cell.
    ///
    /// The policy that used to be inline in `src/app.rs`'s pointer callback, moved
    /// here so both frontends share it — the same reasoning as the scroll momentum
    /// and the prompt queue, and for the same reason: there is exactly one correct
    /// answer to "what does ctrl-click mean" and it should not be written twice.
    ///
    /// `ctrl` starts an additional range instead of replacing (a multi-range
    /// selection, so scattered output can be copied in one go); `shift` extends the
    /// most recent range from its existing anchor rather than starting a new one.
    /// Otherwise the selection is replaced, which is what a plain click means.
    pub(crate) fn begin_selection(&mut self, row: u16, col: u16, ctrl: bool, shift: bool) {
        let (rows, cols) = self.parser.screen().size();
        let row = row.min(rows.saturating_sub(1));
        let col = col.min(cols.saturating_sub(1));
        // Anchor and focus live in absolute scrollback coordinates, so they stay
        // pinned to the text they point at while the view scrolls under them.
        let point = (self.vis_to_abs(row), col);
        if ctrl && !shift {
            self.sel_ranges.push((point, point));
        } else if shift && !self.sel_ranges.is_empty() {
            let anchor = self.sel_ranges.last().map(|range| range.0).unwrap_or(point);
            if let Some(range) = self.sel_ranges.last_mut() {
                *range = (anchor, point);
            }
        } else {
            self.sel_ranges.clear();
            self.sel_ranges.push((point, point));
        }
        let (anchor, focus) = self.sel_ranges.last().copied().unwrap_or((point, point));
        self.sel_anchor = Some(anchor);
        self.sel_focus = Some(focus);
    }

    /// Extend the in-progress drag-selection to a visible grid cell.
    ///
    /// Does nothing when no selection is in progress, so a stray move event with no
    /// button held cannot start one.
    pub(crate) fn extend_selection(&mut self, row: u16, col: u16) {
        if self.sel_anchor.is_none() {
            return;
        }
        let (rows, cols) = self.parser.screen().size();
        let row = row.min(rows.saturating_sub(1));
        let col = col.min(cols.saturating_sub(1));
        let point = (self.vis_to_abs(row), col);
        self.sel_focus = Some(point);
        if let Some(range) = self.sel_ranges.last_mut() {
            range.1 = point;
        }
    }

    /// Finish a drag-selection and return the text to copy, or `None`.
    ///
    /// A plain click selects nothing: the endpoints are inclusive, so an anchor-only
    /// range would "select" the single character under the pointer and copy it,
    /// which is not what a click means. The signal is the coordinates, not the
    /// extracted text — an empty extraction can also come from a real drag over
    /// blank cells, and those two cases want the same answer (clear it) for
    /// different reasons, which is why this compares positions (#319).
    ///
    /// Returns `None` and clears the selection when there is nothing to copy.
    pub(crate) fn finish_selection(&mut self) -> Option<String> {
        if !self.selection_has_extent() {
            self.clear_selection();
            return None;
        }
        let extracted = self.extract_selection_text();
        if extracted.is_empty() {
            self.clear_selection();
            None
        } else {
            Some(extracted)
        }
    }

    /// Drop the current selection.

    /// Advance the view by one auto-scroll step while a drag is past an edge, and
    /// re-point the selection's focus at the row that just arrived there.
    ///
    /// `dir` is negative above the top (reveal older lines) and positive below the
    /// bottom (move toward the live tail). Returns whether the view actually moved,
    /// so a caller can skip a repaint when the drag is already against the end.
    ///
    /// The anchor is absolute, so it stays pinned to its text while the view moves —
    /// which is the whole reason a selection can extend past one screen.
    pub(crate) fn autoscroll_selection(&mut self, dir: i32) -> bool {
        // The alternate screen has no scrollback of ours: the program owns the view.
        if dir == 0 || self.parser.screen().alternate_screen() || self.sel_anchor.is_none() {
            return false;
        }
        const STEP: usize = 2;
        let rows = self.parser.screen().size().0;
        let max_off = self.history.len();
        // Keep the column the user last dragged to; only the row follows the edge.
        let focus_col = self.sel_focus.map(|focus| focus.1).unwrap_or(0);
        let edge_vis = if dir < 0 {
            let new_off = (self.view_offset + STEP).min(max_off);
            if new_off == self.view_offset {
                return false;
            }
            self.view_offset = new_off;
            0u16
        } else {
            let new_off = self.view_offset.saturating_sub(STEP);
            if new_off == self.view_offset {
                return false;
            }
            self.view_offset = new_off;
            rows.saturating_sub(1)
        };
        let point = (self.vis_to_abs(edge_vis), focus_col);
        self.sel_focus = Some(point);
        if let Some(range) = self.sel_ranges.last_mut() {
            range.1 = point;
        }
        true
    }

    /// Select the whole visible row under the pointer, for a triple click.
    ///
    /// Trailing blanks are excluded so copying a line does not drag a screenful of
    /// spaces into the clipboard — the same reason `extract_range_text` clamps its
    /// end into real content.
    ///
    /// Its only caller is the terminal view's click handling, for a triple click,
    /// so the trailing-blank rule above lives here rather than in the view.
    pub(crate) fn select_line_at(&mut self, row: u16) -> Option<String> {
        let (rows, cols) = self.parser.screen().size();
        let row = row.min(rows.saturating_sub(1));
        let line = self.displayed_text.get(row as usize)?;
        if line.is_empty() {
            return None;
        }
        let last = line
            .chars()
            .count()
            .saturating_sub(1)
            .min(u16::MAX as usize) as u16;
        let abs_row = self.vis_to_abs(row);
        let range = ((abs_row, 0), (abs_row, last.min(cols.saturating_sub(1))));
        self.sel_ranges.clear();
        self.sel_ranges.push(range);
        self.sel_anchor = Some(range.0);
        self.sel_focus = Some(range.1);
        Some(line.clone())
    }

    fn extract_range_text(&self, (ar, ac): (usize, u16), (fr, fc): (usize, u16)) -> String {
        let (lo_r, lo_c, hi_r, hi_c) = if (ar, ac) <= (fr, fc) {
            (ar, ac, fr, fc)
        } else {
            (fr, fc, ar, ac)
        };
        let (live, live_used) = self.live_rows();
        let hist_len = self.history.len();
        let combined_len = hist_len + live_used;
        // Clamp into real content so a focus parked on a blank row below the
        // prompt doesn't emit trailing empty lines.
        let hi_r = hi_r.min(combined_len.saturating_sub(1));
        let mut out = String::new();
        for r in lo_r..=hi_r {
            let line: &str = if r < hist_len {
                &self.history[r].0
            } else if r - hist_len < live.len() {
                &live[r - hist_len].0
            } else {
                ""
            };
            let chars: Vec<char> = line.chars().collect();
            // `c0`/`c1` are GRID COLUMNS (inclusive). The plain text keeps one
            // char per glyph, so wide (CJK) glyphs make char index != column;
            // map columns → char indices via the cell prefix so the copied text
            // doesn't drift by the number of wide glyphs before it (#132).
            let (c0, c1) = if r == lo_r && r == hi_r {
                (lo_c.min(hi_c), lo_c.max(hi_c))
            } else if r == lo_r {
                (lo_c, u16::MAX)
            } else if r == hi_r {
                (0, hi_c)
            } else {
                (0, u16::MAX)
            };
            let prefix = cell_prefix(&chars);
            let start = char_at_cell_start(&prefix, c0 as usize);
            let end = char_after_cell_end(&prefix, c1 as usize);
            let seg: String = if start < end {
                chars[start..end].iter().collect()
            } else {
                String::new()
            };
            out.push_str(seg.trim_end());
            let wrapped = if r < hist_len {
                self.history[r].2
            } else if r - hist_len < live.len() {
                live[r - hist_len].2
            } else {
                false
            };
            if r != hi_r && !wrapped {
                out.push('\n');
            }
        }
        out
    }

    /// Whether `bytes` can go to the parser untouched.
    ///
    /// True when the run holds no escape (nothing to rewrite and nothing to
    /// answer, see [`terminal_query`]), no `SO`/`SI` (which move between G0 and
    /// G1), and no byte the active charset would translate. Scanned once, without
    /// allocating: the slow path it short-circuits copies the whole chunk and
    /// walks it byte by byte through the state machine below.
    ///
    /// `memchr` would find an escape faster, but a chunk that gets this far is
    /// dominated by the parser's own work, so the extra dependency is not worth
    /// it here.
    pub(crate) fn is_plain_run(&self, bytes: &[u8]) -> bool {
        let translating = self.vt100_drawing && self.charset.dec_graphics_active();
        for &byte in bytes {
            if byte == 0x1b || byte == 0x0e || byte == 0x0f {
                return false;
            }
            // DEC Special Graphics maps 0x60..=0x7e; every other byte passes.
            if translating && (0x60..=0x7e).contains(&byte) {
                return false;
            }
        }
        true
    }

    /// Feed bytes to vt100 and capture scrolled-off lines into history.
    ///
    /// We detect scroll by diffing the screen before/after a `process`, which
    /// can only recover up to one screen of shift per call.  A single large
    /// burst can scroll many screens at once, so we split the input at newline
    /// boundaries into batches of at most ~half a screen of lines and capture
    /// after each — that way no batch ever scrolls more than the diff can see,
    /// and nothing is lost.  (Splitting only on `\n` is safe: VT escape
    /// sequences never contain a newline.)
    /// The returned bytes are terminal-query replies that must be written back
    /// to the PTY immediately (DSR/CPR and primary device attributes, #328).
    pub(crate) fn ingest(&mut self, input: &[u8]) -> Vec<u8> {
        let formatted = self
            .json_format_output
            .then(|| crate::terminal::format_json_output(input));
        let input = formatted.as_deref().unwrap_or(input);

        // A run with nothing to rewrite goes straight to the parser: no copy of
        // the chunk, and no byte-by-byte pass over it. Terminal output is
        // overwhelmingly this case, so it is worth the branch.
        if self.csi_state == CsiState::Normal && self.is_plain_run(input) {
            self.ingest_display_bytes(input);
            return Vec::new();
        }

        let mut replies = Vec::new();
        let mut display = Vec::with_capacity(input.len());

        for &byte in input {
            match self.csi_state {
                CsiState::Normal => {
                    if byte == 0x1b {
                        self.csi_pending.clear();
                        self.csi_pending.push(byte);
                        self.csi_state = CsiState::Esc;
                    } else if self.vt100_drawing && byte == 0x0e {
                        // SO: shift output to G1 (#376). Swallowed before the
                        // parser, which has no charset support.
                        self.charset.shift_out();
                    } else if self.vt100_drawing && byte == 0x0f {
                        // SI: back to G0.
                        self.charset.shift_in();
                    } else if self.vt100_drawing {
                        // DEC Special Graphics maps 0x60–0x7e (always standalone
                        // ASCII in a UTF-8 stream) to box-drawing Unicode (#376).
                        match self.charset.map(byte) {
                            Some(ch) => {
                                let mut buf = [0u8; 4];
                                display.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            }
                            None => display.push(byte),
                        }
                    } else {
                        display.push(byte);
                    }
                }
                CsiState::Esc => {
                    if byte == b'[' {
                        self.csi_pending.push(byte);
                        self.csi_state = CsiState::Csi;
                    } else if byte == b']' {
                        // OSC: keep buffering so its payload is never
                        // charset-translated while DEC graphics is active.
                        self.csi_pending.push(byte);
                        self.csi_state = CsiState::Osc;
                    } else if matches!(byte, b'(' | b')' | b'*' | b'+') {
                        // SCS designator intro (`ESC ( 0`, …); the final byte
                        // completes it (#376).
                        self.csi_pending.push(byte);
                        self.csi_state = CsiState::Designate(byte);
                    } else {
                        display.extend(self.csi_pending.drain(..));
                        if byte == 0x1b {
                            self.csi_pending.push(byte);
                        } else {
                            display.push(byte);
                            self.csi_state = CsiState::Normal;
                        }
                    }
                }
                CsiState::Designate(set) => {
                    if byte == 0x1b {
                        // Malformed: a new escape interrupts the designator.
                        // Pass the buffered bytes through and start over.
                        display.extend(self.csi_pending.drain(..));
                        self.csi_pending.push(byte);
                        self.csi_state = CsiState::Esc;
                    } else {
                        self.csi_pending.push(byte);
                        if self.vt100_drawing {
                            self.charset.designate(set, byte);
                        }
                        display.extend(self.csi_pending.drain(..));
                        self.csi_state = CsiState::Normal;
                    }
                }
                CsiState::Osc => {
                    self.csi_pending.push(byte);
                    if byte == 0x07 {
                        display.extend(self.csi_pending.drain(..));
                        self.csi_state = CsiState::Normal;
                    } else if byte == 0x18 || byte == 0x1a {
                        // CAN/SUB abort an OSC from anywhere (the parser's
                        // Anywhere rule). The buffered bytes are forwarded so
                        // vt100 sees the same abort it would execute itself.
                        display.extend(self.csi_pending.drain(..));
                        self.csi_state = CsiState::Normal;
                    } else if byte == 0x1b {
                        self.csi_state = CsiState::OscEsc;
                    } else if self.csi_pending.len() > OSC_CAP {
                        // Unbounded OSC: drop what was buffered and keep dropping
                        // until the terminator, so no half-parsed sequence reaches
                        // the parser or the replay ring.
                        self.csi_pending.clear();
                        self.csi_state = CsiState::OscDiscard;
                    }
                }
                CsiState::OscEsc => {
                    self.csi_pending.push(byte);
                    if byte == b'\\' {
                        // ST terminator.
                        display.extend(self.csi_pending.drain(..));
                        self.csi_state = CsiState::Normal;
                    } else if byte == 0x18 || byte == 0x1a {
                        display.extend(self.csi_pending.drain(..));
                        self.csi_state = CsiState::Normal;
                    } else {
                        self.csi_state = CsiState::Osc;
                    }
                }
                CsiState::OscDiscard => {
                    // Nothing is kept: the payload is dropped rather than parsed.
                    if byte == 0x07 {
                        self.csi_state = CsiState::Normal;
                    } else if byte == 0x18 || byte == 0x1a {
                        // Same Anywhere rule as the buffered OSC states. Without
                        // this a server could end its oversized OSC with CAN and
                        // every later byte would stay swallowed — the terminal
                        // looks frozen while the session itself is fine.
                        self.csi_state = CsiState::Normal;
                    } else if byte == 0x1b {
                        self.csi_state = CsiState::OscDiscardEsc;
                    }
                }
                CsiState::OscDiscardEsc => {
                    if byte == b'\\' {
                        self.csi_state = CsiState::Normal;
                    } else if byte == 0x18 || byte == 0x1a {
                        self.csi_state = CsiState::Normal;
                    } else {
                        self.csi_state = CsiState::OscDiscard;
                    }
                }
                CsiState::Csi => {
                    self.csi_pending.push(byte);
                    if (0x40..=0x7e).contains(&byte) {
                        if let Some(kind) = terminal_query(&self.csi_pending) {
                            self.ingest_display_bytes(&display);
                            display.clear();
                            // Reply budget per ingest: a server spraying device
                            // status / cursor-position queries would otherwise
                            // turn every chunk into an equally large reply
                            // stream written straight back to it (audit L-17).
                            // Real programs ask a handful of questions; the
                            // overflow queries are simply not answered.
                            const REPLY_BUDGET: usize = 4 * 1024;
                            if replies.len() < REPLY_BUDGET {
                                match kind {
                                    TerminalQuery::Status => replies.extend_from_slice(b"\x1b[0n"),
                                    TerminalQuery::CursorPosition { private } => {
                                        let (row, col) = self.parser.screen().cursor_position();
                                        let response = if private {
                                            format!("\x1b[?{};{}R", row + 1, col + 1)
                                        } else {
                                            format!("\x1b[{};{}R", row + 1, col + 1)
                                        };
                                        replies.extend_from_slice(response.as_bytes());
                                    }
                                    // Identify only as a VT100 with the advanced
                                    // video option; do not claim unsupported features.
                                    TerminalQuery::PrimaryDeviceAttributes => {
                                        replies.extend_from_slice(b"\x1b[?1;2c")
                                    }
                                }
                            }
                        } else {
                            // Rewrite HVP (`CSI … f`) to CUP (`CSI … H`) because
                            // vt100 implements only the latter.
                            if byte == b'f' {
                                if let Some(final_byte) = self.csi_pending.last_mut() {
                                    *final_byte = b'H';
                                }
                            }
                            display.extend(self.csi_pending.drain(..));
                        }
                        self.csi_pending.clear();
                        self.csi_state = CsiState::Normal;
                    } else if byte == 0x18 || byte == 0x1a {
                        // CAN/SUB cancel the CSI (Anywhere rule); flush the
                        // partial sequence so text after it is not held back.
                        display.extend(self.csi_pending.drain(..));
                        self.csi_state = CsiState::Normal;
                    } else if self.csi_pending.len() > CSI_CAP {
                        // Past the cap the sequence is handed over mid-flight: the
                        // parser keeps consuming it under its own (smaller) limits,
                        // so a long-but-valid sequence is never dropped for length.
                        display.extend(self.csi_pending.drain(..));
                        self.csi_state = CsiState::Normal;
                    }
                }
            }
        }

        self.ingest_display_bytes(&display);
        replies
    }

    fn ingest_display_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // CSI 3 J means "erase saved lines". The vt100 crate clears its own
        // scrollback, but XenTerm maintains a separate rendered history and a
        // raw replay stream for resize reflow. Drop both sides of that history,
        // including when the CSI sequence was split across SSH reads (#319).
        //
        // Searched for in the last three retained bytes plus this chunk only:
        // every older occurrence was already drained through when it arrived, so
        // rescanning the whole retained stream — up to RAW_CAP, 2 MB — on every
        // chunk was work that grew with the buffer for no new information.
        let mut tail = [0u8; ERASE_SAVED.len() - 1];
        let tail_len = self.raw.len().min(tail.len());
        tail[..tail_len].copy_from_slice(&self.raw[self.raw.len() - tail_len..]);
        let erase_saved_through = find_last_erase_saved(&tail[..tail_len], bytes)
            .map(|position| self.raw.len() - tail_len + position);
        // Retain the (post-rewrite) stream, capped, so a resize can replay it at
        // the new width and reflow already-printed output (#169).
        self.raw.extend_from_slice(bytes);
        if let Some(end) = erase_saved_through {
            self.raw.drain(..end);
            self.history.clear();
            self.prev.clear();
            self.view_offset = 0;
            self.sel_anchor = None;
            self.sel_focus = None;
            self.sel_ranges.clear();
        }
        self.cap_raw();
        self.feed_batched(bytes);
    }

    /// Feed a (already HVP-rewritten) byte slice to vt100 in bounded batches,
    /// capturing scrolled-off lines into history after each (see the `ingest`
    /// doc comment). Besides newlines, bound the approximate display width:
    /// one very long physical line can wrap through many screens without ever
    /// containing `\n` (#385). Does NOT touch `self.raw`, so it is reused by
    /// both live ingest and resize-reflow replay.
    fn feed_batched(&mut self, bytes: &[u8]) {
        let (rows, cols) = self.parser.screen().size();
        let rows = rows as usize;
        let batch_lines = (rows / 2).max(1);
        let cell_budget = batch_lines.saturating_mul(cols as usize).max(1);
        let mut start = 0usize;
        let mut nl = 0usize;
        let mut cells = 0usize;
        for i in 0..bytes.len() {
            if bytes[i] == b'\n' {
                nl += 1;
            } else if bytes[i] == b'\t' {
                // A tab can advance up to eight terminal cells.
                cells = cells.saturating_add(8);
            } else if bytes[i] >= 0x20 && bytes[i] & 0xc0 != 0x80 {
                // Count ASCII and UTF-8 leading bytes. Wide Unicode occupies at
                // most two cells, and the half-screen budget leaves that margin.
                cells = cells.saturating_add(1);
            }
            if nl >= batch_lines || cells >= cell_budget {
                self.ingest_chunk(&bytes[start..=i]);
                start = i + 1;
                nl = 0;
                cells = 0;
            }
        }
        if start < bytes.len() {
            self.ingest_chunk(&bytes[start..]);
        }
    }

    /// Trim the retained stream to `RAW_CAP`, dropping from the front up to the
    /// next line boundary so a replay never starts mid-escape / mid-wrapped-line.
    fn cap_raw(&mut self) {
        if self.raw.len() <= RAW_CAP {
            return;
        }
        let overflow = self.raw.len() - RAW_CAP;
        self.raw.drain(0..overflow);
        // Drop up to and including the next line boundary, so a replay never
        // starts mid-escape or mid-wrapped-line. No boundary left means what is
        // retained is one unfinished line, which cannot be replayed at all.
        match self.raw.iter().position(|&byte| byte == b'\n') {
            Some(position) => {
                self.raw.drain(..=position);
            }
            None => self.raw.clear(),
        }
    }

    /// Scroll the scrollback by `delta` lines, which may be fractional.
    ///
    /// The policy lives here rather than in each frontend because it is the same
    /// everywhere and a second copy would drift. `delta` arrives as a fraction of a
    /// line — wheel pixels divided by the cell height — and the fraction is *banked*
    /// rather than dropped, which is what lets a decaying momentum tail converge to a
    /// stop instead of rounding every event to nothing or to a whole line.
    ///
    /// Each event is clamped to ±24 lines so a stray huge pixel delta cannot teleport
    /// the view, and the offset is clamped to the scrollback that exists.
    ///
    /// Returns whether the offset moved, so a caller can skip an expensive repaint
    /// when scrolling at a boundary changed nothing.
    pub(crate) fn scroll_by_lines(&mut self, delta: f32) -> bool {
        let max_off = self.history.len() as i64;
        self.scroll_accum += delta.clamp(-24.0, 24.0);
        let whole = self.scroll_accum.trunc();
        if whole != 0.0 {
            self.scroll_accum -= whole;
        }
        let current = self.view_offset as i64;
        let wanted = current + whole as i64;
        let clamped = wanted.clamp(0, max_off);
        // Only a step that was CLIPPED by a boundary drops the banked remainder. A
        // stale fraction must not fire after a direction flip or fresh output, but
        // *resting* at a boundary has to keep banking: clearing whenever the new offset
        // merely equals a boundary wiped the accumulation on every event and left slow
        // scrolling dead until it was fast enough to cross a whole line per event.
        if clamped != wanted {
            self.scroll_accum = 0.0;
        }
        let changed = clamped != current;
        self.view_offset = clamped as usize;
        changed
    }

    /// Jump straight to an absolute scrollback offset, for a scrollbar drag.
    ///
    /// No accumulation: a drag names a position, so the fractional machinery above
    /// would fight it. Returns whether the offset moved.
    pub(crate) fn scroll_to_offset(&mut self, offset: usize) -> bool {
        let clamped = offset.min(self.history.len());
        if clamped == self.view_offset {
            return false;
        }
        self.view_offset = clamped;
        // A jump is a fresh decision, so any banked fraction belongs to the gesture
        // that just ended.
        self.scroll_accum = 0.0;
        true
    }

    /// Jump the scrollback to an end: `true` for the live bottom, `false` for the
    /// oldest retained line.
    ///
    /// Its own method rather than a caller passing `0` or `history.len()`, because the
    /// two constants are this struct's business and a caller spelling them would be a
    /// caller that has to know how the offset is measured.
    // Its callers are the terminal view's paging-key handler and the scroll tests.
    pub(crate) fn scroll_to_boundary(&mut self, bottom: bool) -> bool {
        let target = if bottom { 0 } else { self.history.len() };
        self.scroll_to_offset(target)
    }

    /// Resize-reflow (#169): rebuild the screen + scrollback at a new width by
    /// replaying the retained byte stream through a fresh parser. vt100 itself
    /// can't reflow (`set_size` just truncates/pads each row), and we only keep
    /// rendered grid rows in `history`, so replaying the raw stream is what lets
    /// long lines rewrap to the new width like FinalShell.
    ///
    /// Alt-screen programs (tmux/vim) are the exception this always claimed to
    /// make: they own the canvas and redraw on SIGWINCH, so there is nothing to
    /// rewrap. Resizing the grid in place is not only cheaper than replaying up to
    /// RAW_CAP bytes, it is the only path that keeps `history` — a replay on the
    /// alt screen captures no rows at all (`ingest_chunk` refuses to), so the
    /// cleared history stayed empty and a resize in vim threw the scrollback away.
    pub(crate) fn reflow(&mut self, new_rows: u16, new_cols: u16) {
        if self.parser.screen().alternate_screen() {
            self.parser.set_size(new_rows, new_cols);
            self.prev.clear();
            self.view_offset = 0;
            // The grid changed size under them, so nothing anchored to a cell is
            // meaningful any more.
            self.sel_anchor = None;
            self.sel_focus = None;
            self.sel_ranges.clear();
            return;
        }
        // Taken rather than cloned: the stream is up to RAW_CAP (2 MB) and the
        // replay below never reads `raw`.
        let stream = std::mem::take(&mut self.raw);
        self.parser = vt100::Parser::new(new_rows, new_cols, PARSER_SCROLLBACK);
        self.history.clear();
        self.prev.clear();
        self.view_offset = 0;
        // Scrollback line count changes, so absolute selection coords no longer map.
        self.sel_anchor = None;
        self.sel_focus = None;
        self.sel_ranges.clear();
        self.feed_batched(&stream);
        self.raw = stream;
    }

    /// Refresh the cached `mouse_tracked` flag from the parser's current mouse
    /// protocol state (the remote app enables it with e.g. `\x1b[?1000h` /
    /// `\x1b[?1002h`).  Kept as a cached bool so the UI hot path doesn't need
    /// to lock the parser to decide how a click behaves.
    fn sync_mouse_tracked(&mut self) {
        self.mouse_tracked =
            self.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None;
    }

    /// Process one bounded batch and capture any lines that scrolled off the top
    /// (skipped for alt-screen programs like vim/nano).
    fn ingest_chunk(&mut self, bytes: &[u8]) {
        // Detect full-screen-clear sequences *before* processing so we can
        // suppress history for programs that redraw without alt-screen (e.g.
        // btop configured with `alt-screen = false`).
        // We look for \033[H (cursor-home) and \033[2J / \033[J (erase display)
        // as indicators that the program is doing a full-screen refresh.
        let has_cursor_home = bytes.windows(3).any(|w| w == b"\x1b[H");
        let has_erase_display =
            bytes.windows(4).any(|w| w == b"\x1b[2J") || bytes.windows(3).any(|w| w == b"\x1b[J");
        let is_fullscreen_refresh = has_cursor_home && has_erase_display;

        self.parser.process(bytes);
        self.sync_mouse_tracked();
        let (is_alt, rows, cols) = {
            let s = self.parser.screen();
            let (r, c) = s.size();
            (s.alternate_screen(), r, c)
        };
        if is_alt {
            // Snap to live view whenever we're on the alt screen — this
            // prevents old history (accumulated before alt-screen was entered)
            // from mixing with the full-screen program's output after a scroll.
            self.view_offset = 0;
            self.prev.clear();
            return;
        }
        if is_fullscreen_refresh {
            // Non-alt-screen full-screen refresh (btop, htop with alt disabled…).
            // Don't capture lines into history; they'd mix with the next frame.
            self.view_offset = 0;
            self.prev.clear();
            return;
        }
        let curr: Vec<Line> = {
            let s = self.parser.screen();
            (0..rows).map(|r| build_row(s, r, cols)).collect()
        };
        if !self.prev.is_empty() {
            let k = detect_scroll(&self.prev, &curr);
            for line in self.prev.iter().take(k) {
                self.history.push_back(line.clone());
            }
            while self.history.len() > MAX_HISTORY {
                self.history.pop_front();
            }
            // `view_offset` is measured backwards from the live bottom.  If
            // output scrolls while the user is reading history, keeping the
            // same offset would move their viewport forward by `k` rows. Move
            // the offset back by the number of newly captured rows instead so
            // the content under the scrollbar stays anchored (#306). At the
            // live bottom (`0`) output-following remains unchanged.
            if self.view_offset > 0 && k > 0 {
                self.view_offset = self.view_offset.saturating_add(k).min(self.history.len());
            }
        }
        self.prev = curr;
    }

    /// Render the terminal grid for the current scrollback `view_offset`
    /// (0 = live).  Caches the displayed plain text for find/selection.
    pub(crate) fn render(&mut self) -> BuiltScreen {
        let (is_alt, rows, cols, cur_row, cur_col) = {
            let s = self.parser.screen();
            let (r, c) = s.size();
            let (cr, cc) = s.cursor_position();
            (s.alternate_screen(), r, c, cr, cc)
        };

        // --- Live view (also alt-screen): render the current grid -----------
        if is_alt || self.view_offset == 0 {
            let mut spans = Vec::new();
            let mut displayed = Vec::with_capacity(rows as usize);
            let mut last_content = 0i32;
            let s = self.parser.screen();
            for r in 0..rows {
                let (plain, runs, _wrapped) = build_row(s, r, cols);
                let runs = if is_alt {
                    runs
                } else {
                    highlight_plain_output(
                        runs,
                        self.output_highlight,
                        &self.custom_highlight_rules,
                    )
                };
                if !runs.is_empty() {
                    last_content = r as i32;
                }
                for hs in runs {
                    spans.extend(render_term_span(&hs, r as i32, self.is_dark));
                }
                displayed.push(plain.trim_end().to_string());
            }
            self.displayed_text = displayed;
            let rows_used = if is_alt {
                rows as i32
            } else {
                last_content + 1
            };
            return BuiltScreen {
                spans,
                cursor_row: cur_row as i32,
                cursor_col: cur_col as i32,
                rows_used,
                is_alt,
                mouse_tracked: self.mouse_tracked,
                scroll_max: if is_alt { 0 } else { self.history.len() as i32 },
                scroll_offset: 0,
            };
        }

        // --- Scrolled view: window into history ++ live content -------------
        let live: Vec<Line> = {
            let s = self.parser.screen();
            (0..rows).map(|r| build_row(s, r, cols)).collect()
        };
        let hist_len = self.history.len();
        // Include the screen's trailing blank rows in the scroll range so this
        // scrolled view stays continuous with the live view (view_offset 0).
        // Trimming to only the used rows made the two views misalign after a
        // shrink-then-grow (dragging the SFTP panel over the terminal and back),
        // so scrolling back jumped at the bottom instead of moving line-by-line
        // (#119-followup).
        let combined_len = hist_len + live.len();
        let win = rows as usize;
        let start = combined_len.saturating_sub(win + self.view_offset);
        let end = (start + win).min(combined_len);

        let mut spans = Vec::new();
        let mut displayed = Vec::with_capacity(win);
        for (d, idx) in (start..end).enumerate() {
            let line: &Line = if idx < hist_len {
                &self.history[idx]
            } else {
                &live[idx - hist_len]
            };
            let runs = highlight_plain_output(
                line.1.clone(),
                self.output_highlight,
                &self.custom_highlight_rules,
            );
            for hs in &runs {
                spans.extend(render_term_span(hs, d as i32, self.is_dark));
            }
            displayed.push(line.0.trim_end().to_string());
        }
        while displayed.len() < win {
            displayed.push(String::new());
        }
        self.displayed_text = displayed;
        BuiltScreen {
            spans,
            cursor_row: -1, // hide the live cursor while viewing history
            cursor_col: 0,
            rows_used: win as i32,
            is_alt: false,
            mouse_tracked: self.mouse_tracked,
            scroll_max: self.history.len() as i32,
            scroll_offset: self.view_offset as i32,
        }
    }
}
