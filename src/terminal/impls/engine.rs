use rio_vt::ansi::CursorShape;
use rio_vt::config::colors::{AnsiColor, ColorRgb};
use rio_vt::crosswords::grid::Grid;
use rio_vt::crosswords::pos::{Column, Line, Pos};
use rio_vt::crosswords::square::Square;
use rio_vt::crosswords::style::StyleFlags;
use rio_vt::crosswords::{Crosswords, CrosswordsSize, Mode};
use rio_vt::event::{VoidListener, WindowId};
use rio_vt::performer::handler::Processor;

use crate::terminal::{HistSpan, Line as RenderLine};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TerminalColor {
    #[default]
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MouseProtocolEncoding {
    #[default]
    Default,
    Utf8,
    Sgr,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MouseProtocolMode {
    #[default]
    None,
    Press,
    PressRelease,
    ButtonMotion,
    AnyMotion,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalCell {
    pub(crate) text: String,
    pub(crate) fg: TerminalColor,
    pub(crate) bg: TerminalColor,
    pub(crate) bold: bool,
    pub(crate) inverse: bool,
    pub(crate) is_wide: bool,
    pub(crate) is_wide_continuation: bool,
}

impl TerminalCell {
    pub(crate) fn contents(&self) -> String {
        self.text.clone()
    }
    pub(crate) fn fgcolor(&self) -> TerminalColor {
        self.fg
    }
    pub(crate) fn bgcolor(&self) -> TerminalColor {
        self.bg
    }
    pub(crate) fn bold(&self) -> bool {
        self.bold
    }
    pub(crate) fn inverse(&self) -> bool {
        self.inverse
    }
    pub(crate) fn is_wide(&self) -> bool {
        self.is_wide
    }
    pub(crate) fn is_wide_continuation(&self) -> bool {
        self.is_wide_continuation
    }
}

pub(crate) struct TerminalRow<'a> {
    grid: &'a Grid<Square>,
    row: Line,
}
impl TerminalRow<'_> {
    pub(crate) fn cell(&self, column: u16) -> Option<TerminalCell> {
        read_cell(self.grid, self.row, column as usize)
    }

    pub(crate) fn wrapped(&self) -> bool {
        self.grid[self.row]
            .inner
            .last()
            .is_some_and(|square| square.wrapline())
    }
}

pub(crate) struct TerminalEngine {
    terminal: Crosswords<VoidListener>,
    processor: Processor,
    normal_history_len: usize,
    normal_lines_evicted: u64,
}

impl TerminalEngine {
    pub(crate) fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        let size = CrosswordsSize::new(cols.max(1) as usize, rows.max(1) as usize);
        Self {
            terminal: Crosswords::new(
                size,
                CursorShape::Block,
                VoidListener,
                WindowId::from(0),
                0,
                scrollback,
            ),
            processor: Processor::default(),
            normal_history_len: 0,
            normal_lines_evicted: 0,
        }
    }
    pub(crate) fn process(&mut self, bytes: &[u8]) {
        self.refresh_normal_history();
        self.processor.advance(&mut self.terminal, bytes);
        // XenTerm already batches output at presentation checkpoints. Flush
        // Rio's synchronized-update buffer at this boundary: a final chunk or
        // disconnected peer must not leave text waiting for another PTY read.
        if self.processor.sync_timeout().sync_timeout().is_some() {
            self.processor.stop_sync(&mut self.terminal);
        }
        self.refresh_normal_history();
    }

    fn refresh_normal_history(&mut self) {
        if !self.alternate_screen() {
            self.normal_history_len = self.terminal.history_size();
            self.normal_lines_evicted = self.terminal.lines_evicted();
        }
    }

    pub(crate) fn set_size(&mut self, rows: u16, cols: u16) {
        self.resize(rows, cols);
    }

    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        let size = CrosswordsSize::new(cols.max(1) as usize, rows.max(1) as usize);
        self.terminal.resize(size);
        self.refresh_normal_history();
    }

    pub(crate) fn screen(&self) -> &Self {
        self
    }

    pub(crate) fn size(&self) -> (u16, u16) {
        (
            self.terminal.screen_lines() as u16,
            self.terminal.columns() as u16,
        )
    }

    pub(crate) fn cursor_position(&self) -> (u16, u16) {
        let pos = self.terminal.grid.cursor.pos;
        (pos.row.0.max(0) as u16, pos.col.0 as u16)
    }
    pub(crate) fn alternate_screen(&self) -> bool {
        self.terminal.mode().contains(Mode::ALT_SCREEN)
    }

    pub(crate) fn application_cursor(&self) -> bool {
        self.terminal.mode().contains(Mode::APP_CURSOR)
    }

    pub(crate) fn application_keypad(&self) -> bool {
        self.terminal.mode().contains(Mode::APP_KEYPAD)
    }

    pub(crate) fn bracketed_paste(&self) -> bool {
        self.terminal.mode().contains(Mode::BRACKETED_PASTE)
    }

    pub(crate) fn cursor_hidden(&self) -> bool {
        !self.terminal.mode().contains(Mode::SHOW_CURSOR)
    }

    pub(crate) fn mouse_protocol_encoding(&self) -> MouseProtocolEncoding {
        let mode = self.terminal.mode();
        if mode.contains(Mode::SGR_MOUSE) {
            MouseProtocolEncoding::Sgr
        } else if mode.contains(Mode::UTF8_MOUSE) {
            MouseProtocolEncoding::Utf8
        } else {
            MouseProtocolEncoding::Default
        }
    }
    pub(crate) fn mouse_protocol_mode(&self) -> MouseProtocolMode {
        let mode = self.terminal.mode();
        if mode.contains(Mode::MOUSE_MOTION) {
            MouseProtocolMode::AnyMotion
        } else if mode.contains(Mode::MOUSE_DRAG) {
            MouseProtocolMode::ButtonMotion
        } else if mode.contains(Mode::MOUSE_REPORT_CLICK) {
            MouseProtocolMode::PressRelease
        } else if mode.contains(Mode::MOUSE_REPORT_X10) {
            MouseProtocolMode::Press
        } else {
            MouseProtocolMode::None
        }
    }

    pub(crate) fn cell(&self, row: u16, column: u16) -> Option<TerminalCell> {
        if row >= self.size().0 {
            return None;
        }
        read_cell(&self.terminal.grid, Line(row as i32), column as usize)
    }

    pub(crate) fn row_wrapped(&self, row: u16) -> bool {
        self.row(row).is_some_and(|row| row.wrapped())
    }

    pub(crate) fn build_live_row(&self, row: u16) -> Option<RenderLine> {
        let (rows, columns) = self.size();
        (row < rows).then(|| build_line(&self.terminal.grid, Line(row as i32), columns as usize))
    }
    pub(crate) fn row(&self, row: u16) -> Option<TerminalRow<'_>> {
        (row < self.size().0).then_some(TerminalRow {
            grid: &self.terminal.grid,
            row: Line(row as i32),
        })
    }

    pub(crate) fn history_len(&self) -> usize {
        if self.alternate_screen() {
            self.normal_history_len
        } else {
            self.terminal.history_size()
        }
    }

    pub(crate) fn history_row(&self, index: usize) -> Option<TerminalRow<'_>> {
        if self.alternate_screen() {
            return None;
        }
        let len = self.history_len();
        (index < len).then_some(TerminalRow {
            grid: &self.terminal.grid,
            row: Line(index as i32 - len as i32),
        })
    }

    pub(crate) fn clear_history(&mut self) {
        self.terminal.clear_saved_history();
        self.refresh_normal_history();
    }
}
fn read_cell(grid: &Grid<Square>, row: Line, column: usize) -> Option<TerminalCell> {
    let square = *grid[row].inner.get(column)?;
    let pos = Pos::new(row, Column(column));
    let continuation = square.is_spacer() || square.is_leading_spacer();
    let text = if continuation {
        String::new()
    } else {
        let text: String = grid
            .cell_text(pos)
            .map(|ch| if ch == '\0' { ' ' } else { ch })
            .collect();
        text
    };
    let style = grid.style_of(&square);
    Some(TerminalCell {
        text,
        fg: color(style.fg),
        bg: color(style.bg),
        bold: style.flags.contains(StyleFlags::BOLD),
        inverse: style.flags.contains(StyleFlags::INVERSE),
        is_wide: square.is_wide(),
        is_wide_continuation: continuation,
    })
}
fn color(value: AnsiColor) -> TerminalColor {
    match value {
        AnsiColor::Indexed(index) => TerminalColor::Idx(index),
        AnsiColor::Spec(ColorRgb { r, g, b }) => TerminalColor::Rgb(r, g, b),
        AnsiColor::Named(name) => {
            let index = name as usize;
            if index < 16 {
                TerminalColor::Idx(index as u8)
            } else {
                TerminalColor::Default
            }
        }
    }
}
impl TerminalEngine {
    pub(crate) fn display_offset(&self) -> usize {
        self.terminal.display_offset()
    }

    pub(crate) fn set_display_offset(&mut self, offset: usize) {
        use rio_vt::crosswords::grid::Scroll;
        let target = offset.min(self.terminal.history_size()) as i32;
        let current = self.terminal.display_offset() as i32;
        self.terminal
            .scroll_display(Scroll::Delta(target - current));
    }
    pub(crate) fn lines_evicted(&self) -> u64 {
        if self.alternate_screen() {
            self.normal_lines_evicted
        } else {
            self.terminal.lines_evicted()
        }
    }

    /// Read native cells in the same oldest-first coordinates as selection.
    #[cfg(feature = "desktop")]
    pub(crate) fn row_at_abs(&self, index: usize) -> Option<TerminalRow<'_>> {
        let history = if self.alternate_screen() { 0 } else { self.history_len() };
        (index < history + self.size().0 as usize).then_some(TerminalRow {
            grid: &self.terminal.grid,
            row: Line(index as i32 - history as i32),
        })
    }

    /// Oldest-first across native history and the visible grid.
    pub(crate) fn line_at_abs(&self, index: usize) -> Option<RenderLine> {
        let history = if self.alternate_screen() {
            0
        } else {
            self.history_len()
        };
        let (rows, columns) = self.size();
        if index >= history + rows as usize {
            return None;
        }
        let row = Line(index as i32 - history as i32);
        Some(build_line(&self.terminal.grid, row, columns as usize))
    }
}

fn build_line(grid: &Grid<Square>, row: Line, columns: usize) -> RenderLine {
    let mut plain = String::with_capacity(columns);
    let mut runs = Vec::new();
    let mut column = 0;
    while column < columns {
        let cell = read_cell(grid, row, column).expect("column in grid");
        let width = if cell.is_wide { 2 } else { 1 };
        if cell.is_wide_continuation {
            column += 1;
            continue;
        }
        plain.push_str(&cell.text);
        let starts_at = column;
        let mut text = cell.text.clone();
        column += width;
        while column < columns {
            let next = read_cell(grid, row, column).expect("column in grid");
            if next.is_wide
                || next.is_wide_continuation
                || next.fg != cell.fg
                || next.bg != cell.bg
                || next.bold != cell.bold
                || next.inverse != cell.inverse
            {
                break;
            }
            plain.push_str(&next.text);
            text.push_str(&next.text);
            column += 1;
        }
        let invisible_blank =
            text.chars().all(|ch| ch == ' ') && cell.bg == TerminalColor::Default && !cell.inverse;
        if !invisible_blank {
            runs.push(HistSpan {
                text,
                fg: cell.fg,
                bg: cell.bg,
                bold: cell.bold,
                inverse: cell.inverse,
                col: starts_at as i32,
                cells: (column - starts_at) as i32,
            });
        }
    }
    let wrapped = grid[row]
        .inner
        .last()
        .is_some_and(|square| square.wrapline());
    (plain, runs, wrapped)
}
impl TerminalEngine {
    /// Plain visible text, preserving printed spaces and joining wrapped rows.
    pub(crate) fn contents(&self) -> String {
        let (rows, _cols) = self.size();
        let mut output = String::new();
        let mut wrapping = false;
        for row in 0..rows {
            // Preserve explicitly printed trailing spaces (shell prompts),
            // while omitting untouched NUL cells at the end of the row.
            let used = self.terminal.grid[Line(row as i32)]
                .inner
                .iter()
                .rposition(|square| square.c() != '\0')
                .map_or(0, |index| index + 1);
            let mut line = String::new();
            for column in 0..used as u16 {
                if let Some(cell) = self.cell(row, column) {
                    line.push_str(&cell.text);
                }
            }
            if wrapping && line.is_empty() {
                output.push('\n');
            } else {
                output.push_str(&line);
            }
            wrapping = self.row_wrapped(row);
            if !wrapping {
                output.push('\n');
            }
        }
        while output.ends_with('\n') {
            output.pop();
        }
        output
    }
}
#[cfg(test)]
impl TerminalEngine {
    /// Insert plain fixture rows into Rio's native scrollback ring.
    pub(crate) fn seed_history(&mut self, lines: &[RenderLine]) {
        assert!(!self.alternate_screen());
        let (rows, cols) = self.size();
        let visible: Vec<_> = (0..rows)
            .map(|row| self.terminal.grid[Line(row as i32)].clone())
            .collect();
        let cursor = self.terminal.grid.cursor.clone();
        let offset = self.display_offset();
        self.terminal.clear_saved_history();
        let mut fixture_processor = Processor::default();

        for (text, spans, wrapped) in lines {
            assert!(spans.is_empty(), "seed_history accepts plain test rows");
            fixture_processor.advance(&mut self.terminal, b"\x1b[1;1H\x1b[0m\x1b[2K");
            fixture_processor.advance(&mut self.terminal, text.as_bytes());
            self.terminal.grid[Line(0)][Column(cols as usize - 1)].set_wrapline(*wrapped);
            self.terminal
                .grid
                .scroll_up(&(Line(0)..Line(rows as i32)), 1);
        }

        for (index, row) in visible.into_iter().enumerate() {
            self.terminal.grid[Line(index as i32)] = row;
        }
        self.terminal.grid.cursor = cursor;
        self.set_display_offset(offset);
        self.terminal.mark_fully_damaged();
        self.refresh_normal_history();
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_column_terminal_keeps_its_geometry() {
        let mut engine = TerminalEngine::new(2, 1, 8);
        engine.process(b"A");
        assert_eq!(engine.size(), (2, 1));
        assert_eq!(engine.cell(0, 0).unwrap().contents(), "A");
        engine.resize(3, 1);
        assert_eq!(engine.size(), (3, 1));
    }

    #[test]
    fn wide_and_combining_text_uses_native_cell_layout() {
        let mut engine = TerminalEngine::new(2, 8, 8);
        engine.process("中e\u{301}".as_bytes());
        let wide = engine.cell(0, 0).unwrap();
        assert_eq!(wide.contents(), "中");
        assert!(wide.is_wide());
        assert!(engine.cell(0, 1).unwrap().is_wide_continuation());
        assert_eq!(engine.cell(0, 2).unwrap().contents(), "e\u{301}");
    }
    #[test]
    fn emoji_cluster_stays_in_one_wide_cell() {
        let mut engine = TerminalEngine::new(2, 8, 8);
        engine.process("👩\u{200d}💻".as_bytes());
        assert_eq!(engine.cell(0, 0).unwrap().contents(), "👩\u{200d}💻");
        assert!(engine.cell(0, 0).unwrap().is_wide());
        assert!(engine.cell(0, 1).unwrap().is_wide_continuation());
    }

    #[test]
    fn unicode_fixture_enters_native_history_without_changing_live_rows() {
        let mut engine = TerminalEngine::new(2, 8, 8);
        engine.process(b"live");
        engine.seed_history(&[("中e\u{301}".into(), Vec::new(), false)]);
        assert_eq!(engine.history_len(), 1);
        assert_eq!(
            engine.history_row(0).unwrap().cell(0).unwrap().contents(),
            "中"
        );
        assert_eq!(engine.cell(0, 0).unwrap().contents(), "l");
    }
}

impl TerminalEngine {
    /// Read text without allocating per-cell strings or reconstructing styles.
    /// Find and clipboard extraction need text, not display runs.
    pub(crate) fn text_line_at_abs(&self, index: usize) -> Option<(String, bool)> {
        let history = if self.alternate_screen() {
            0
        } else {
            self.history_len()
        };
        let (rows, columns) = self.size();
        if index >= history + rows as usize {
            return None;
        }
        let row = Line(index as i32 - history as i32);
        let grid = &self.terminal.grid;
        let mut text = String::with_capacity(columns as usize);
        for (column, square) in grid[row].inner.iter().enumerate() {
            if square.is_spacer() || square.is_leading_spacer() {
                continue;
            }
            text.extend(grid.cell_text(Pos::new(row, Column(column))).map(|ch| {
                if ch == '\0' {
                    ' '
                } else {
                    ch
                }
            }));
        }
        Some((
            text,
            grid[row]
                .inner
                .last()
                .is_some_and(|square| square.wrapline()),
        ))
    }
}
