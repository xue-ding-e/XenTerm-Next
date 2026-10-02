use super::*;

#[test]
fn inverse_default_colours_paint_a_visible_background() {
    let (fg, bg) = vt_span_colors(
        vt100::Color::Default,
        vt100::Color::Default,
        false,
        true,
        true,
    );
    // The same two colours this has always asserted: the point of this test is the
    // inverse-video swap, so the numbers must outlive the type they are written
    // against.
    assert_eq!(
        fg,
        Rgba {
            r: 0x0e,
            g: 0x0f,
            b: 0x13,
            a: 0xff
        }
    );
    assert_eq!(
        bg,
        Rgba {
            r: 0xd4,
            g: 0xd4,
            b: 0xd4,
            a: 0xff
        }
    );

    let mut parser = vt100::Parser::new(3, 30, 0);
    parser.process(b"abc \x1b[7m20260705\x1b[27m end");
    let (_plain, runs, _wrapped) = build_row(parser.screen(), 0, 30);
    let hit = runs
        .iter()
        .find(|span| span.text.contains("20260705"))
        .expect("reverse-video search hit should be a separate span");
    assert!(hit.inverse);
    assert!(matches!(hit.fg, vt100::Color::Default));
    assert!(matches!(hit.bg, vt100::Color::Default));
}
