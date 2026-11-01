use crate::terminal::{Line, TerminalEngine};

/// Per-session native scrollback cap.
pub(crate) const MAX_HISTORY: usize = 100_000;

/// Grid-column boundaries for the text stored in a native terminal row.
/// A cluster's combining marks, variation selectors and ZWJ continuations all
/// share its starting column; the final boundary carries its complete width.
pub(crate) fn cell_prefix(chars: &[char]) -> Vec<usize> {
    let codepoints: Vec<u32> = chars.iter().map(|ch| *ch as u32).collect();
    let mut prefix = Vec::with_capacity(chars.len() + 1);
    let (mut index, mut column) = (0, 0);
    while index < codepoints.len() {
        let (consumed, width) = rio_vt::grapheme_lut::cluster_width(&codepoints[index..]);
        let consumed = consumed.max(1).min(codepoints.len() - index);
        prefix.extend(std::iter::repeat_n(column, consumed));
        index += consumed;
        column += width as usize;
    }
    prefix.push(column);
    prefix
}

pub(crate) fn char_at_cell_start(prefix: &[usize], target: usize) -> usize {
    let char_count = prefix.len().saturating_sub(1);
    for index in 0..char_count {
        if prefix[index] <= target && target < prefix[index + 1] {
            let column = prefix[index];
            let mut first = index;
            while first > 0 && prefix[first - 1] == column {
                first -= 1;
            }
            return first;
        }
    }
    char_count
}

pub(crate) fn char_after_cell_end(prefix: &[usize], target: usize) -> usize {
    let char_count = prefix.len().saturating_sub(1);
    for (index, column) in prefix.iter().take(char_count).enumerate() {
        if *column > target {
            return index;
        }
    }
    char_count
}

/// Build display runs directly from the engine's live row, without copying a
/// complete grid or inferring history from similarities between screen frames.
pub(crate) fn build_row(screen: &TerminalEngine, row: u16, _columns: u16) -> Line {
    screen.build_live_row(row).unwrap_or_default()
}
