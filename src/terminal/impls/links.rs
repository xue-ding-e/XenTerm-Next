//! Conservative text links. OSC 8 labels are deliberately not resolved here.
use crate::terminal::TermBuffer;

// A remote process must not make a click scan an unbounded wrapped history line.
const MAX_LINK_CELLS: usize = 16 * 1024;
const MAX_LINK_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalLink {
    pub(crate) url: String,
    // Releasing over different output must not open the URL remembered on press.
    absolute_cell: (usize, u16),
    lines_evicted: u64,
    alternate_screen: bool,
}

impl TermBuffer {
    /// Resolve a visible native cell, including the trailing half of a wide glyph.
    /// Only soft-wrapped rows are joined; hard newlines and actual spaces remain
    /// boundaries. Native cells keep Unicode/combining widths and history aligned.
    pub(crate) fn link_at(&self, row: u16, col: u16) -> Option<TerminalLink> {
        let (rows, cols) = self.parser.size();
        if row >= rows || col >= cols || usize::from(cols) > MAX_LINK_CELLS {
            return None;
        }
        let absolute = self.vis_to_abs(row);
        let row_budget = MAX_LINK_CELLS / usize::from(cols);
        let mut start = absolute;
        while start > 0 && self.parser.row_at_abs(start - 1)?.wrapped() {
            start -= 1;
            if absolute - start >= row_budget {
                return None;
            }
        }

        let mut text = String::new();
        let mut clicked_byte = None;
        let mut current = start;
        loop {
            if current - start >= row_budget {
                return None;
            }
            let native = self.parser.row_at_abs(current)?;
            for column in 0..cols {
                let cell = native.cell(column)?;
                // Rio uses both a wide-glyph trailing spacer and a leading
                // spacer at a wrap boundary. Neither contributes URL bytes.
                if cell.is_wide_continuation() {
                    continue;
                }
                if current == absolute
                    && (column == col || (cell.is_wide() && column + 1 == col))
                {
                    clicked_byte = Some(text.len());
                }
                text.push_str(&cell.contents());
                if text.len() > MAX_LINK_BYTES {
                    return None;
                }
            }
            if !native.wrapped() {
                break;
            }
            current += 1;
        }
        let url = web_link_at(&text, clicked_byte?)?;
        Some(TerminalLink {
            url,
            absolute_cell: (absolute, col),
            lines_evicted: self.parser.lines_evicted(),
            alternate_screen: self.parser.alternate_screen(),
        })
    }
}

fn link_delimiter(ch: char) -> bool {
    ch.is_whitespace() || ch.is_control() || matches!(ch, '<' | '>' | '"' | '\'' | '`')
}

fn web_link_at(text: &str, clicked_byte: usize) -> Option<String> {
    for (start, _) in text.char_indices() {
        let rest = &text[start..];
        let prefix_len = if rest.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")) {
            8
        } else if rest.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://")) {
            7
        } else {
            continue;
        };
        // Do not reinterpret a URL embedded in a different scheme or hostname.
        // Opening brackets and assignments are common terminal-output wrappers.
        if text[..start].chars().next_back().is_some_and(|ch| {
            !link_delimiter(ch) && !matches!(ch, '(' | '[' | '{' | '=' | '：')
        }) {
            continue;
        }
        let end = rest.find(link_delimiter).unwrap_or(rest.len());
        let candidate = trim_link_end(&rest[..end]);
        if !(start..start + candidate.len()).contains(&clicked_byte) {
            continue;
        }
        let authority = candidate.get(prefix_len..)?;
        if authority.is_empty() || authority.starts_with('/') || candidate.contains('\\') {
            return None;
        }
        // Invisible directional controls make the displayed destination deceptive.
        if candidate.chars().any(|ch| matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) {
            return None;
        }
        let parsed = url::Url::parse(candidate).ok()?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none_or(str::is_empty)
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return None;
        }
        // Hand the platform a parsed, canonical HTTP(S) URL, never a command.
        return Some(parsed.into());
    }
    None
}

fn trim_link_end(mut text: &str) -> &str {
    let mut counts = [0usize; 6];
    for ch in text.chars() {
        if let Some(index) = ['(', ')', '[', ']', '{', '}'].iter().position(|&value| value == ch) {
            counts[index] += 1;
        }
    }
    loop {
        let Some(last) = text.chars().next_back() else { return text; };
        let closer = match last { ')' => Some(1), ']' => Some(3), '}' => Some(5), _ => None };
        let unmatched = closer.is_some_and(|index| counts[index] > counts[index - 1]);
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '。' | '，' | '；' | '！' | '？') || unmatched {
            text = &text[..text.len() - last.len_utf8()];
            if let Some(index) = closer { counts[index] -= 1; }
        } else {
            return text;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_links_accept_http_https_ports_paths_queries_and_balanced_punctuation() {
        for (text, expected) in [
            ("https://example.test:8443/p?a=1&b=2#part", "https://example.test:8443/p?a=1&b=2#part"),
            ("(https://example.test/a(b)).", "https://example.test/a(b)"),
            ("[http://[::1]:8080/a]", "http://[::1]:8080/a"),
            ("URL=https://example.test/", "https://example.test/"),
            ("HTTPS://EXAMPLE.TEST/", "https://example.test/"),
            ("网址：https://example.test/，", "https://example.test/"),
        ] {
            let at = text.find("://").unwrap();
            assert_eq!(web_link_at(text, at).as_deref(), Some(expected), "{text}");
        }
        let text = "(https://example.test/a).";
        assert!(web_link_at(text, 0).is_none());
        assert!(web_link_at(text, text.len() - 2).is_none());
        assert!(web_link_at(text, text.len() - 1).is_none());
    }

    #[test]
    fn text_links_reject_non_web_schemes_malformed_and_deceptive_destinations() {
        for text in [
            "file:///tmp/a", "javascript:alert(1)", "data:text/html,hello", "shell:calc",
            "javascript:https://example.test/", "file://https://example.test/",
            "https:///example.test/", "https://", "https://user:pass@example.test/",
            "https://example.test\\@other.test/", "https://example.test/\u{202e}txt",
            "https://example.test:99999/", "https://[invalid]/",
        ] {
            for at in text.char_indices().map(|(i, _)| i) {
                assert!(web_link_at(text, at).is_none(), "{text:?} at {at}");
            }
        }
    }

    #[test]
    fn native_links_use_wide_combining_cells_and_soft_wraps() {
        let mut buffer = TermBuffer::new(8, 20);
        buffer.ingest("中e\u{301} https://example.test/路径?q=1".as_bytes());
        let expected = url::Url::parse("https://example.test/路径?q=1").unwrap().to_string();
        // Wide CJK and the combining cluster occupy three columns, not four chars.
        assert!(buffer.link_at(0, 3).is_none());
        assert_eq!(buffer.link_at(0, 4).unwrap().url, expected);
        assert_eq!(buffer.link_at(1, 4).unwrap().url, expected);
        // Both halves of 路 (columns 5-6 on the second row) resolve identically.
        assert_eq!(buffer.link_at(1, 5).unwrap().url, expected);
        assert_eq!(buffer.link_at(1, 6).unwrap().url, expected);
        assert!(buffer.link_at(0, 20).is_none());
        assert!(buffer.link_at(8, 0).is_none());
    }

    #[test]
    fn native_links_do_not_join_hard_lines_or_printed_spaces() {
        let mut buffer = TermBuffer::new(5, 30);
        buffer.ingest(b"https://example.test/\r\nnot-part-of-the-url");
        assert_eq!(buffer.link_at(0, 3).unwrap().url, "https://example.test/");
        assert!(buffer.link_at(1, 3).is_none());
        buffer.ingest(b"\r\nhttps://example.test/ path");
        assert_eq!(buffer.link_at(2, 3).unwrap().url, "https://example.test/");
        assert!(buffer.link_at(2, 23).is_none());
    }

    #[test]
    fn native_links_skip_wide_wrap_padding_and_survive_reflow() {
        let mut buffer = TermBuffer::new(5, 12);
        buffer.ingest("https://a/x路".as_bytes());
        let expected = url::Url::parse("https://a/x路").unwrap().to_string();
        // Rio wraps the wide glyph rather than splitting it over the row edge.
        assert_eq!(buffer.link_at(1, 0).unwrap().url, expected);
        assert_eq!(buffer.link_at(1, 1).unwrap().url, expected);
        assert!(buffer.link_at(0, 11).is_none(), "wrap padding is not a link cell");
        buffer.reflow(5, 30);
        assert_eq!(buffer.link_at(0, 11).unwrap().url, expected);
        assert_eq!(buffer.link_at(0, 12).unwrap().url, expected);
    }

    #[test]
    fn native_links_use_alternate_screen_without_normal_history() {
        let mut buffer = TermBuffer::new(3, 40);
        buffer.ingest(b"https://example.test/old\r\nx\r\ny\r\nz");
        buffer.scroll_to_offset(1);
        buffer.ingest(b"\x1b[?1049h\x1b[Hhttps://example.test/tui");
        assert_eq!(buffer.link_at(0, 5).unwrap().url, "https://example.test/tui");
    }

    #[test]
    fn native_links_follow_scrollback_and_do_not_require_visible_start() {
        let mut buffer = TermBuffer::new(3, 12);
        buffer.ingest(b"https://example.test/long/path\r\nother\r\nlast");
        buffer.scroll_to_offset(2);
        // The URL starts in history and ends in the live screen.
        assert_eq!(buffer.link_at(0, 2).unwrap().url, "https://example.test/long/path");
        buffer.scroll_to_offset(0);
        assert_eq!(buffer.link_at(0, 2).unwrap().url, "https://example.test/long/path");
        assert!(buffer.link_at(1, 2).is_none());
    }

    #[test]
    fn native_links_bound_untrusted_wrapped_lines_and_ignore_osc8_targets() {
        let mut buffer = TermBuffer::new(3, 80);
        buffer.ingest(format!("https://example.test/{}", "a".repeat(MAX_LINK_CELLS)).as_bytes());
        assert!(buffer.link_at(0, 2).is_none());
        let mut buffer = TermBuffer::new(3, 80);
        buffer.ingest(b"\x1b]8;;https://example.test/\x1b\\label\x1b]8;;\x1b\\");
        assert!(buffer.link_at(0, 2).is_none());
    }
}
