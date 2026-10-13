use crate::terminal::{
    build_row, cell_prefix, char_after_cell_end, char_at_cell_start,
    highlight_plain_output, render_term_span, BuiltScreen, CsiState, Line, TermBuffer, TermMatch,
    MAX_HISTORY, TerminalEngine, MouseProtocolMode,
};

// Rio owns the grid and its scrollback; history is read lazily by the frontend.

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

/// Longest OSC payload buffered before the rest of the sequence is dropped.
///
/// Buffering a bounded complete OSC keeps payload bytes out of the text grid
/// and avoids allocations controlled by an untrusted remote stream.
pub(crate) const OSC_CAP: usize = 4096;

/// Longest CSI parameter string buffered before it is handed to the parser.
///
/// Long but valid sequences exist — an SGR run carrying several RGB colours can
/// pass 64 bytes — so this is deliberately generous. Past it the buffered prefix
/// is forwarded and this scanner gives up on the sequence, which is exactly what
/// the parser is for.
const CSI_CAP: usize = 1024;

impl TermBuffer {
    /// A fresh buffer with an empty screen of the given size.
    ///
    /// A constructor rather than a struct literal at each call site because there
    /// are several fields and state machines that must start in a particular
    /// state: a `charset` mid-designation or a `csi_state` that is not
    /// `Normal` would both be a buffer that mis-renders
    /// its first byte. Callers go through this rather than building the literal
    /// inline, so the initialisation has one place to get wrong instead of many.
    pub(crate) fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: TerminalEngine::new(rows, cols, MAX_HISTORY),
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
            view_offset: 0,
            scroll_accum: 0.0,
            displayed_text: Vec::new(),
            csi_state: CsiState::Normal,
            csi_pending: Vec::new(),
        }
    }

    /// Release all retained terminal output and recreate the parser at the
    /// current size. This is used when a session is disconnected or the user
    /// explicitly clears the terminal, so a dead tab does not keep a large
    /// scrollback allocation alive until it is closed.
    pub(crate) fn release_scrollback(&mut self) {
        let (rows, cols) = self.parser.screen().size();
        self.parser = TerminalEngine::new(rows, cols, MAX_HISTORY);
        self.find_query.clear();
        self.view_offset = 0;
        self.sel_anchor = None;
        self.sel_focus = None;
        self.sel_ranges.clear();
        self.displayed_text = Vec::new();
        self.csi_state = CsiState::Normal;
        self.csi_pending = Vec::new();
        self.charset = crate::terminal::CharsetTracker::default();
        self.mouse_tracked = false;
    }

    /// Native scrollback size, preserving the normal-screen count while in a TUI.
    pub(crate) fn history_len(&self) -> usize { self.parser.history_len() }

    /// Oldest-first scrollback rows, materialized only when selection/find need them.
    pub(crate) fn history_lines(&self) -> impl Iterator<Item = Line> + '_ {
        let count = if self.parser.alternate_screen() { 0 } else { self.history_len() };
        (0..count).filter_map(|index| self.parser.line_at_abs(index))
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
        if self.parser.alternate_screen() { return 0; }
        let rows = self.parser.screen().size().0 as usize;
        let hist_len = self.history_len();
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
        let hist_len = self.history_len();
        let combined_len = hist_len + live.len();
        let Some(match_idx) = (0..hist_len)
            .filter_map(|index| self.parser.text_line_at_abs(index).map(|line| line.0))
            .chain(live.iter().map(|line| line.0.clone()))
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

    /// Jump to the next (`forward`) or previous row matching the active query.
    ///
    /// Enter in the find bar steps through the matches rather than only ever
    /// showing the first one. The anchor is the row the view currently starts
    /// at, and the scan is strictly past it (or strictly before it), wrapping at
    /// the ends — so repeated steps march through the scrollback and a wrap
    /// returns to the beginning rather than getting stuck. The target lands at
    /// the top of the view, the same anchor [`Self::scroll_to_first_find_match`]
    /// uses, so the two read as one motion.
    pub(crate) fn step_find_match(&mut self, forward: bool) -> bool {
        let query = self.find_query.clone();
        if query.is_empty() || self.parser.screen().alternate_screen() {
            return false;
        }
        let q = query.to_lowercase();
        let (live, _) = self.live_rows();
        let rows = self.parser.screen().size().0 as usize;
        let hist_len = self.history_len();
        let combined_len = hist_len + live.len();
        if combined_len == 0 {
            return false;
        }
        let line_matches = |index: usize| -> bool {
            let line = if index < hist_len {
                self.parser.text_line_at_abs(index).map(|line| line.0)
            } else {
                live.get(index - hist_len).map(|line| line.0.clone())
            };
            line.is_some_and(|line| line.to_lowercase().contains(&q))
        };
        let anchor = self.view_top_abs();
        let mut probe = if forward {
            (anchor + 1) % combined_len
        } else {
            (anchor + combined_len - 1) % combined_len
        };
        let mut target = None;
        for _ in 0..combined_len {
            if line_matches(probe) {
                target = Some(probe);
                break;
            }
            probe = if forward {
                (probe + 1) % combined_len
            } else {
                (probe + combined_len - 1) % combined_len
            };
        }
        let Some(target) = target else {
            return false;
        };
        let top = target.min(combined_len.saturating_sub(rows));
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
        let max_off = self.history_len();
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
        let hist_len = if self.parser.alternate_screen() { 0 } else { self.history_len() };
        let combined_len = hist_len + live_used;
        // Clamp into real content so a focus parked on a blank row below the
        // prompt doesn't emit trailing empty lines.
        let hi_r = hi_r.min(combined_len.saturating_sub(1));
        let mut out = String::new();
        for r in lo_r..=hi_r {
            let historical = (r < hist_len).then(|| self.parser.text_line_at_abs(r)).flatten();
            let live_row = live.get(r.saturating_sub(hist_len));
            let line = if r < hist_len {
                historical.as_ref().map(|line| line.0.as_str()).unwrap_or("")
            } else { live_row.map(|line| line.0.as_str()).unwrap_or("") };
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
                historical.as_ref().is_some_and(|line| line.1)
            } else { live_row.is_some_and(|line| line.2) };
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

    /// Feed bytes into the native grid, which records scrolling directly.
    /// XenTerm keeps its bounded OSC scanner and explicitly answers the
    /// protocol queries it advertises, including across split reads.
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
                    } else if byte == 0x0e {
                        // The session's DEC drawing option owns charset shifts
                        // (#376); do not let the engine bypass that option.
                        if self.vt100_drawing { self.charset.shift_out(); }
                    } else if byte == 0x0f {
                        // SI: back to G0.
                        if self.vt100_drawing { self.charset.shift_in(); }
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
                        // SCS is owned by this scanner. Drop the interrupted
                        // prefix so the native parser cannot remain inside a
                        // designation whose replacement was consumed here.
                        self.csi_pending.clear();
                        self.csi_pending.push(byte);
                        self.csi_state = CsiState::Esc;
                    } else {
                        self.csi_pending.push(byte);
                        if self.vt100_drawing {
                            self.charset.designate(set, byte);
                        }
                        // Output is already translated when drawing is enabled;
                        // swallow SCS so native charset support cannot translate
                        // again or enable drawing for a disabled session option.
                        self.csi_pending.clear();
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
                        // the native engine sees the same abort it would execute.
                        display.extend(self.csi_pending.drain(..));
                        self.csi_state = CsiState::Normal;
                    } else if byte == 0x1b {
                        self.csi_state = CsiState::OscEsc;
                    } else if self.csi_pending.len() > OSC_CAP {
                        // Unbounded OSC: drop what was buffered and keep dropping
                        // until the terminator, so no half-parsed sequence reaches
                        // the native parser.
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
                            if self.csi_pending == b"\x1b[3J" {
                                self.clear_selection();
                                self.view_offset = 0;
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
        if bytes.is_empty() { return; }
        // Native scrolling records every row, including identical rows and
        // arbitrarily long wrapped lines (#385), without screen comparisons.
        let evicted = self.parser.lines_evicted();
        let was_alt = self.parser.alternate_screen();
        self.parser.set_display_offset(self.view_offset);
        self.parser.process(bytes);
        self.sync_mouse_tracked();
        self.view_offset = if self.parser.alternate_screen() { 0 } else {
            self.parser.display_offset().min(self.history_len())
        };
        // Selections use oldest-first indices. Shift retained endpoints when
        // the ring drops old rows; discard only a selection whose text is gone.
        let dropped = self.parser.lines_evicted().saturating_sub(evicted) as usize;
        if was_alt != self.parser.alternate_screen() {
            self.clear_selection();
        } else if dropped > 0 { self.rebase_selection(dropped); }
    }

    fn rebase_selection(&mut self, dropped: usize) {
        let adjust = |point: &mut (usize, u16)| {
            if let Some(row) = point.0.checked_sub(dropped) { point.0 = row; true } else { false }
        };
        let anchor_ok = self.sel_anchor.as_mut().is_none_or(&adjust);
        let focus_ok = self.sel_focus.as_mut().is_none_or(&adjust);
        let ranges_ok = self.sel_ranges.iter_mut().all(|(anchor, focus)| adjust(anchor) && adjust(focus));
        if !anchor_ok || !focus_ok || !ranges_ok { self.clear_selection(); }
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
        let max_off = self.history_len() as i64;
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
        let clamped = offset.min(self.history_len());
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
        let target = if bottom { 0 } else { self.history_len() };
        self.scroll_to_offset(target)
    }

    /// Reflow native screen and scrollback at the new width (#169). Ordinary
    /// and alternate screens keep their own grids; neither replays raw output.
    pub(crate) fn reflow(&mut self, new_rows: u16, new_cols: u16) {
        self.parser.resize(new_rows, new_cols);
        self.parser.set_display_offset(0);
        self.view_offset = 0;
        self.scroll_accum = 0.0;
        // Grid coordinates changed; old selection endpoints no longer name
        // the same cells, even though all retained text is still in the grid.
        self.clear_selection();
    }

    /// Refresh the cached `mouse_tracked` flag from the parser's current mouse
    /// protocol state (the remote app enables it with e.g. `\x1b[?1000h` /
    /// `\x1b[?1002h`).  Kept as a cached bool so the UI hot path doesn't need
    /// to lock the parser to decide how a click behaves.
    fn sync_mouse_tracked(&mut self) {
        self.mouse_tracked =
            self.parser.screen().mouse_protocol_mode() != MouseProtocolMode::None;
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
                scroll_max: if is_alt { 0 } else { self.history_len() as i32 },
                scroll_offset: 0,
            };
        }

        // --- Scrolled view: window into history ++ live content -------------
        let live: Vec<Line> = {
            let s = self.parser.screen();
            (0..rows).map(|r| build_row(s, r, cols)).collect()
        };
        let hist_len = self.history_len();
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
            let historical = (idx < hist_len).then(|| self.parser.line_at_abs(idx)).flatten();
            let Some(line) = historical.as_ref().or_else(|| live.get(idx.saturating_sub(hist_len))) else { continue; };
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
            scroll_max: self.history_len() as i32,
            scroll_offset: self.view_offset as i32,
        }
    }
}

#[cfg(test)]
mod step_find_tests {
    use super::*;

    /// A 4-row screen with several lines of history scrolled off the top.
    fn buffer_with_history() -> TermBuffer {
        let mut buffer = TermBuffer::new(4, 20);
        for round in 0..4 {
            let lines = format!(
                "round-{} alpha
round-{} beta
round-{} gamma
round-{} alpha
",
                round, round, round, round
            );
            buffer.ingest(lines.as_bytes());
        }
        buffer
    }

    #[test]
    fn enter_steps_forward_through_matches_and_wraps() {
        let mut buffer = buffer_with_history();
        buffer.set_find_query("beta");
        // The query's first scroll lands on the first match; a step from there
        // must find the *next* one, not re-find the first.
        let first = buffer.view_offset;
        assert!(buffer.step_find_match(true), "a later beta exists");
        assert!(buffer.view_offset < first, "stepping forward scrolls up");
        for _ in 0..8 {
            buffer.step_find_match(true);
        }
        // After more steps than matches the walk wraps rather than sticking.
        assert!(buffer.view_offset <= first);
    }

    #[test]
    fn shift_enter_steps_backwards() {
        let mut buffer = buffer_with_history();
        buffer.set_find_query("alpha");
        // Walk forward to the top, then one step back must come down.
        while buffer.step_find_match(true) && buffer.view_offset > 0 {}
        let top = buffer.view_offset;
        assert!(buffer.step_find_match(false));
        assert!(buffer.view_offset > top, "stepping back scrolls down");
    }

    #[test]
    fn an_empty_query_or_alt_screen_steps_nowhere() {
        let mut buffer = buffer_with_history();
        buffer.find_query.clear();
        assert!(!buffer.step_find_match(true));
        // The alt screen owns the view; stepping must refuse rather than
        // scroll a full-screen program out from under the user.
        buffer.ingest(b"[?1049h");
        buffer.set_find_query("alpha");
        assert!(!buffer.step_find_match(true));
    }
}
