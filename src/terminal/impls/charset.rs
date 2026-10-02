//! VT100 character-set (SCS) support (#376): tracks G0/G1 designations
//! (`ESC ( X`, `ESC ) X`) and SO/SI shifts, and maps DEC Special Graphics
//! codes to their Unicode box-drawing equivalents. This mirrors PuTTY's
//! "Enable VT100 line drawing even in UTF-8 mode": the `vt100` crate has no
//! charset support, so designated bytes are translated to Unicode *before*
//! they reach the parser, which also keeps the reflow replay ring correct.

/// Character sets selectable through an SCS designator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Charset {
    UsAscii,
    DecSpecialGraphics,
}

/// Per-terminal G0/G1 charset state plus the SO/SI shift flag.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CharsetTracker {
    g0: Charset,
    g1: Charset,
    /// SO (0x0E) shifted output to G1 until SI (0x0F).
    shifted: bool,
}

impl Default for CharsetTracker {
    fn default() -> Self {
        Self {
            g0: Charset::UsAscii,
            g1: Charset::UsAscii,
            shifted: false,
        }
    }
}

impl CharsetTracker {
    /// Apply an SCS designation. `set` is the designator intro byte (`(` = G0,
    /// `)` = G1); `final_byte` selects the charset (`0` = DEC Special Graphics,
    /// `B` = US ASCII). Unknown set/final combinations are ignored, matching
    /// real VT100s that only reject unsupported pairs.
    pub(crate) fn designate(&mut self, set: u8, final_byte: u8) {
        let charset = match final_byte {
            b'0' => Charset::DecSpecialGraphics,
            b'B' => Charset::UsAscii,
            _ => return,
        };
        match set {
            b'(' => self.g0 = charset,
            b')' => self.g1 = charset,
            // G2/G3 (`*`, `+`) need single-shift (SS2/SS3) support; nothing in
            // the vt100 crate consumes those, so the designation is ignored.
            _ => {}
        }
    }

    pub(crate) fn shift_out(&mut self) {
        self.shifted = true;
    }

    pub(crate) fn shift_in(&mut self) {
        self.shifted = false;
    }

    /// Map a display byte through the active charset. Returns `None` when the
    /// byte passes through unchanged (i.e. US-ASCII is active or the byte is
    /// outside the DEC Special Graphics range).
    pub(crate) fn map(&self, byte: u8) -> Option<char> {
        let active = if self.shifted { self.g1 } else { self.g0 };
        if active != Charset::DecSpecialGraphics || !(0x60..=0x7e).contains(&byte) {
            return None;
        }
        Some(DEC_SPECIAL_GRAPHICS[(byte - 0x60) as usize])
    }

    /// Whether the active character set is DEC Special Graphics, i.e. whether
    /// some bytes in a run would be translated before they reach the parser.
    ///
    /// The ingest fast path asks this per chunk: with US-ASCII active — the
    /// common case even when the VT100 drawing option is on — a run of plain
    /// bytes is already exactly the byte stream the parser should see.
    pub(crate) fn dec_graphics_active(&self) -> bool {
        let active = if self.shifted { self.g1 } else { self.g0 };
        active == Charset::DecSpecialGraphics
    }
}

/// DEC Special Graphics, 0x60–0x7e in order (VT100 user guide, table 3-9).
/// Bytes in this range are always standalone ASCII in a UTF-8 stream, so the
/// byte→char mapping can never split a multi-byte sequence.
const DEC_SPECIAL_GRAPHICS: [char; 31] = [
    '◆', // 0x60 `
    '▒', // 0x61 a
    '␉', // 0x62 b
    '␌', // 0x63 c
    '␍', // 0x64 d
    '␊', // 0x65 e
    '°', // 0x66 f
    '±', // 0x67 g
    '␤', // 0x68 h
    '␋', // 0x69 i
    '┘', // 0x6a j
    '┐', // 0x6b k
    '┌', // 0x6c l
    '└', // 0x6d m
    '┼', // 0x6e n
    '⎺', // 0x6f o
    '⎻', // 0x70 p
    '─', // 0x71 q
    '⎼', // 0x72 r
    '⎽', // 0x73 s
    '├', // 0x74 t
    '┤', // 0x75 u
    '┴', // 0x76 v
    '┬', // 0x77 w
    '│', // 0x78 x
    '≤', // 0x79 y
    '≥', // 0x7a z
    'π', // 0x7b {
    '≠', // 0x7c |
    '£', // 0x7d }
    '·', // 0x7e ~
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_paren_zero_selects_dec_graphics_and_maps_box_chars() {
        let mut t = CharsetTracker::default();
        t.designate(b'(', b'0');
        assert_eq!(t.map(b'l'), Some('┌'));
        assert_eq!(t.map(b'q'), Some('─'));
        assert_eq!(t.map(b'k'), Some('┐'));
        assert_eq!(t.map(b'x'), Some('│'));
        // Below/above the special-graphics range passes through.
        assert_eq!(t.map(0x5f), None);
        assert_eq!(t.map(0x7f), None);
    }

    #[test]
    fn esc_paren_b_restores_ascii() {
        let mut t = CharsetTracker::default();
        t.designate(b'(', b'0');
        t.designate(b'(', b'B');
        assert_eq!(t.map(b'q'), None);
    }

    #[test]
    fn unknown_designator_final_is_ignored() {
        let mut t = CharsetTracker::default();
        t.designate(b'(', b'Z');
        assert_eq!(t.map(b'q'), None);
        t.designate(b'(', b'0');
        t.designate(b'*', b'0'); // G2 has no effect without SS2/SS3
        assert_eq!(t.map(b'q'), Some('─'));
    }

    #[test]
    fn so_si_shift_between_g0_and_g1() {
        let mut t = CharsetTracker::default();
        t.designate(b')', b'0'); // G1 = DEC Special Graphics
        assert_eq!(t.map(b'q'), None); // G0 still US-ASCII
        t.shift_out();
        assert_eq!(t.map(b'q'), Some('─')); // G1 active
        t.shift_in();
        assert_eq!(t.map(b'q'), None);
    }

    #[test]
    fn g1_designation_does_not_affect_g0() {
        let mut t = CharsetTracker::default();
        t.designate(b'(', b'0');
        t.designate(b')', b'B');
        t.shift_out();
        assert_eq!(t.map(b'q'), None); // G1 = US-ASCII
    }

    #[test]
    fn graphics_table_is_contiguous_vt100_set() {
        // Spot-check the visual-critical box corners against U+25xx.
        assert_eq!(DEC_SPECIAL_GRAPHICS[0x6a - 0x60], '┘');
        assert_eq!(DEC_SPECIAL_GRAPHICS[0x6c - 0x60], '┌');
        assert_eq!(DEC_SPECIAL_GRAPHICS[0x71 - 0x60], '─');
        assert_eq!(DEC_SPECIAL_GRAPHICS[0x78 - 0x60], '│');
        assert_eq!(DEC_SPECIAL_GRAPHICS[0x7e - 0x60], '·');
    }
}
