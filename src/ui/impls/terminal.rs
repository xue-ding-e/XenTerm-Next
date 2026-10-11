//! The terminal grid, drawn on GPUI primitives.
//!
//! The pipeline above this is `crate::terminal`'s and is not duplicated here:
//! `vt100` parses the byte stream, `TermBuffer` holds the scrollback and the
//! selection, and `render_term_span` turns cells into positioned coloured runs.
//! What this module owns is the last step — turning a [`BuiltScreen`] into paints —
//! and the decision that shapes everything else in it is that the grid is a
//! *monospace cell grid*, not a paragraph of text.
//!
//! That decision is why each span is shaped and painted on its own rather than the
//! whole row being shaped as one line. GPUI's text system lays out a run by glyph
//! advance, which is right for prose and wrong for a terminal: a wide glyph, a
//! fallback font with different metrics, or a stretch of CJK would push everything
//! after it off the column it belongs in, and the drift accumulates along the row.
//! Painting span by span at `col * cell_width` keeps every span where the PTY said
//! it goes, and it costs one `shape_line` per span — which is the same order as the
//! spans themselves, and those are already merged by fg/bg/bold upstream.
//!
//! Cells are measured, not guessed. `cell_width` comes from the advance of a
//! monospace glyph at the current font size, and `line_height` from the font's own
//! ascent plus descent, so a font-size change moves the grid instead of drawing it
//! at the wrong pitch. That is also the answer to the CJK alignment risk: a CJK
//! glyph is drawn into the two columns the terminal reserved for it, and whether it
//! fills them is a font question the grid does not have to answer.

use gpui_kit::{
    canvas, fill, point, prelude::*, px, size, App, Bounds, Font, FontWeight, Hsla, Pixels, Point,
    RenderImage, SharedString, TextAlign, TextRun, UnderlineStyle, Window,
};

use crate::terminal::{BuiltScreen, EmojiImage, Rgba, TermSpan};

/// Extra pixels between rows, so a dense screen of text does not read as a solid
/// block.
///
/// Added on top of the font's own line box, which the settings' line spacing scales:
/// this is the terminal's own breathing room and is not something a user should have to
/// ask for, while `terminal-line-spacing` is theirs to change.
const LINE_SPACING: f32 = 1.0;

/// Everything one frame of the grid needs, captured off the buffer thread.
///
/// A snapshot rather than a handle, because the paint callback runs on GPUI's
/// thread and `TermBuffer` is behind a `std::sync::Mutex` that the session's pump
/// threads hold while they parse. Locking it from inside `paint` would put a
/// blocking lock on the frame path and let a firehose of output stall the window.
#[derive(Clone, Default)]
pub(crate) struct GridSnapshot {
    pub(crate) spans: Vec<TermSpan>,
    pub(crate) cursor_row: i32,
    pub(crate) cursor_col: i32,
    pub(crate) rows_used: i32,
    /// Lines of scrollback above the live screen, so a scrollbar knows the range.
    pub(crate) scroll_max: i32,
    /// How far the view is scrolled up from the live bottom; 0 is the bottom.
    pub(crate) scroll_offset: i32,
    /// Whether a full-screen program has the terminal on the alternate screen.
    ///
    /// Read by the key handler: on the alt screen the paging keys belong to the program
    /// (`less`, `vim`, `tmux`) and must be forwarded, while on the normal screen they
    /// scroll this terminal's own scrollback.
    pub(crate) is_alt: bool,
    /// Whether the remote asked to be told about the mouse.
    ///
    /// A program that turns this on — btop, htop, mc — wants the clicks and the wheel,
    /// so a press must be forwarded instead of starting a local selection: the user is
    /// aiming at the program's widgets, not at the text.
    pub(crate) mouse_tracked: bool,
    /// The IME composition, drawn underlined at the cursor.
    ///
    /// Part of the frame rather than of the view's paint closure because it is drawn
    /// like any other text: the grid is a character grid, and a composition is
    /// characters the user has not committed yet. Carrying it here keeps
    /// [`terminal_grid`]'s signature as it was and puts the one thing that is neither
    /// output nor cursor in the same place as the output and the cursor.
    pub(crate) composition: Option<String>,
    /// Highlight rectangles for the current selection, in grid cells.
    ///
    /// Converted from the buffer's framework-neutral [`crate::terminal::TermMatch`]
    /// by the view, and drawn here because a highlight is a cell-shaped rectangle on
    /// the same grid as the text: drawing it anywhere else would mean computing the
    /// cell geometry twice and risking two answers.
    pub(crate) selection: Vec<crate::terminal::TermMatch>,
    /// Highlight rectangles for a search's matches, in grid cells.
    ///
    /// A separate list from `selection` rather than one merged list, because the two
    /// want different colours: a search hit and the text the user has selected can
    /// overlap, and drawing them in one colour would hide which is which.
    pub(crate) find_matches: Vec<crate::terminal::TermMatch>,
}

impl GridSnapshot {
    /// Snapshot the visible screen of a built buffer.
    pub(crate) fn of(screen: &BuiltScreen) -> Self {
        Self {
            spans: screen.spans.clone(),
            cursor_row: screen.cursor_row,
            cursor_col: screen.cursor_col,
            rows_used: screen.rows_used,
            scroll_max: screen.scroll_max,
            scroll_offset: screen.scroll_offset,
            is_alt: screen.is_alt,
            mouse_tracked: screen.mouse_tracked,
            composition: None,
            selection: Vec::new(),
            find_matches: Vec::new(),
        }
    }
}

/// The measured size of one terminal cell.
///
/// Both numbers come from the text system rather than from a constant, because a
/// terminal's entire layout is "how many cells fit", and a constant that is close
/// is a grid that drifts by a pixel per column and wraps text at the wrong place.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CellMetrics {
    pub(crate) width: Pixels,
    pub(crate) height: Pixels,
}

impl CellMetrics {
    /// Measure the cell for `family` at `font_size`, with `line_spacing` scaling the row.
    ///
    /// The width is the advance of a single character in the chosen monospace
    /// family, which is exact for a monospace face by definition: every character
    /// that fits in one cell advances by the same amount, so one measurement
    /// answers for all of them. `ascent + descent` is the line box, which is taller
    /// than the glyphs and is what keeps rows from touching.
    ///
    /// `line_spacing` multiplies that line box, which is what the setting of the same
    /// name means: 1.0 is the font's own line, 1.2 is a fifth more air between rows. The
    /// extra `LINE_SPACING` pixels are added after the scaling so that a spacing of 1.0
    /// measures exactly what it always did.
    pub(crate) fn measure(
        cx: &App,
        family: &SharedString,
        font_size: Pixels,
        line_spacing: f32,
    ) -> Self {
        let text_system = cx.text_system();
        let font_id = text_system.resolve_font(&Font {
            family: family.clone(),
            ..Default::default()
        });
        // `ch_advance` is the advance of the font's preferred "0" glyph, which is
        // the conventional monospace cell width. Its error case is a font that
        // failed to load, and the caller has already resolved the family against
        // installed fonts, so a zero here means the grid collapses to a column —
        // visible immediately rather than silent.
        let width = text_system
            .ch_advance(font_id, font_size)
            .unwrap_or(px(f32::from(font_size) * 0.6));
        // `abs`, and this is the bug that made the first working frame unreadable.
        // `TextSystem::descent` returns the font's metric as stored — negative, the
        // signed distance below the baseline — while `LineLayout::descent` is its
        // positive magnitude. Summing the signed one subtracts the descender:
        // Consolas at 14px measures ascent 12.88 and descent -3.51, so `ascent +
        // descent` gave a 10.36px row for glyphs that are 12.88px tall and every row
        // drew on top of the one above it.
        //
        // The font's own line gap is deliberately not added: GPUI exposes ascent and
        // descent through `TextSystem` and not the gap, so the breathing room between
        // rows comes from `LINE_SPACING` instead of from a metric nobody can read.
        let ascent = text_system.ascent(font_id, font_size);
        let descent = text_system.descent(font_id, font_size).abs();
        let natural = ascent + descent;
        Self {
            width,
            height: natural * line_spacing + px(LINE_SPACING),
        }
    }

    /// How many whole columns and rows fit in `size`.
    pub(crate) fn grid_size(&self, available: gpui_kit::Size<Pixels>) -> (u16, u16) {
        let cols = (f32::from(available.width) / f32::from(self.width)).floor();
        let rows = (f32::from(available.height) / f32::from(self.height)).floor();
        // Clamped to at least 1: a PTY told it is zero columns wide is a PTY that
        // reports an error, and the window can genuinely be smaller than one cell
        // for a frame during a resize.
        (cols.max(1.0) as u16, rows.max(1.0) as u16)
    }
}

/// How the grid is drawn: the font, and the cursor.
///
/// Bundled rather than passed as loose arguments because every one of these comes from
/// the same settings page, and a painter that took six parameters would invite a caller
/// to pass five of them from the settings and one from somewhere else.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GridStyle {
    pub(crate) family: SharedString,
    pub(crate) font_size: Pixels,
    /// Whether the session's bold runs are drawn bold.
    ///
    /// The setting is "bold terminal text" (#262): off means a run the session marked
    /// bold draws in the regular face, which is what a reader who finds bold hard to read
    /// wants. It is not the same as "draw everything bold".
    pub(crate) bold: bool,
    pub(crate) line_spacing: f32,
    pub(crate) cursor: CursorStyle,
    /// The cursor's own colour, or the theme's foreground when the setting is empty.
    pub(crate) cursor_color: Option<Hsla>,
}

/// The insertion cursor's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorStyle {
    /// Fills the cell.
    Block,
    /// A thin vertical bar at the cell's leading edge.
    Bar,
    /// A bar along the cell's baseline.
    Underline,
}

impl CursorStyle {
    /// Read the setting.
    ///
    /// The config has already normalised the string, so anything unrecognised is a
    /// value written by hand into the file — and the block is what every terminal did
    /// before there was a setting at all.
    pub(crate) fn from_setting(value: &str) -> Self {
        match value {
            "bar" => Self::Bar,
            "underline" => Self::Underline,
            _ => Self::Block,
        }
    }
}

/// The terminal grid, as an element.
///
/// Takes the snapshot by value rather than borrowing, so the element can outlive
/// the borrow of the view's state that produced it — which is what lets the view
/// hand it to `canvas` and keep rendering.
///
/// `on_paint` runs inside the paint callback after the grid is drawn, and exists for
/// exactly one reason: `Window::handle_input` is only legal during the paint phase
/// and only while the view's focus handle is focused, so a view that wants typed
/// text and IME composition has to register its handler from in here. There is no
/// other moment it could be registered from.
#[allow(clippy::too_many_arguments)]
pub(crate) fn terminal_grid(
    snapshot: GridSnapshot,
    metrics: CellMetrics,
    style: GridStyle,
    cursor_default: Hsla,
    background: Hsla,
    on_paint: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    canvas(
        // Nothing to prepare: everything is decided in paint. Reserved for the
        // shaped lines if measuring shows shaping is the frame's cost, which is the
        // only reason a terminal would need a prepaint step.
        move |bounds, _, _| bounds,
        move |bounds, _, window, cx| {
            paint_grid(
                &snapshot,
                metrics,
                &style,
                cursor_default,
                background,
                bounds,
                window,
                cx,
            );
            on_paint(bounds, window, cx);
        },
    )
    .absolute()
    .size_full()
}

/// Draw one snapshot. Every coordinate is derived from `metrics`, never from the
/// text's own layout, which is what keeps the grid a grid.
#[allow(clippy::too_many_arguments)]
fn paint_grid(
    snapshot: &GridSnapshot,
    metrics: CellMetrics,
    style: &GridStyle,
    cursor_default: Hsla,
    background: Hsla,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let cursor = style.cursor_color.unwrap_or(cursor_default);
    // Cursor alpha is intentional, including zero. Selection, search, scroll
    // and composition feedback keep the terminal palette's opaque contrast.
    let indicator = Hsla { a: 1., ..cursor_default };
    // The terminal's own background, so a screen with no output is the same colour
    // as a screen with output rather than the window's.
    window.paint_quad(fill(bounds, background));

    let origin = bounds.origin;
    let cell = |col: i32, row: i32| {
        point(
            origin.x + metrics.width * col as f32,
            origin.y + metrics.height * row as f32,
        )
    };

    // The selection goes under the text, not over it: a highlight drawn on top would
    // tint the glyphs themselves, which is what makes some terminals' selections
    // unreadable. Here the text is painted afterwards at full contrast.
    paint_selection(snapshot, metrics, &cell, indicator, window);
    // Search hits under the selection, so a hit the user has also selected still reads
    // as selected. Same layer, drawn first.
    paint_find_matches(snapshot, metrics, &cell, indicator, window);

    for span in &snapshot.spans {
        let top_left = cell(span.col, span.row);

        // Background first, and only when it is not transparent: the default
        // background is fully transparent precisely so that a run with no explicit
        // background does not paint over the terminal's. Skipping it also saves a
        // quad per span on the common case, which is most of the screen.
        if span.bg.a != 0 {
            window.paint_quad(fill(
                Bounds::new(
                    top_left,
                    size(metrics.width * span.cells as f32, metrics.height),
                ),
                rgba_to_hsla(span.bg),
            ));
        }

        if span.emoji {
            if let Some(image) = &span.emoji_image {
                paint_emoji_span(image, top_left, metrics, span.cells, window);
            }
        } else {
            paint_text_span(
                span,
                top_left,
                metrics,
                &style.family,
                style.font_size,
                style.bold,
                window,
                cx,
            );
        }
    }

    paint_cursor(snapshot, metrics, cell, style.cursor, cursor, window);
    paint_composition(
        snapshot,
        metrics,
        cell,
        &style.family,
        style.font_size,
        indicator,
        window,
        cx,
    );
    paint_scrollbar(snapshot, bounds, metrics, indicator, window);
}

/// Draw the uncommitted IME composition at the cursor, underlined.
///
/// A native terminal shows a composition at the insertion point, underlined, so the
/// user can see what they are choosing between while the candidate window is open. It
/// is drawn *over* the cell rather than pushed into the grid because it is not the
/// session's output: nothing has been sent to the PTY, and a shell that redrew the line
/// under it would erase it if it were part of the buffer.
///
/// Underlined rather than merely coloured, because the underline is the conventional
/// signal for "this text is not committed yet" — the same convention the platform's own
/// text fields use.
#[allow(clippy::too_many_arguments)]
fn paint_composition(
    snapshot: &GridSnapshot,
    metrics: CellMetrics,
    cell: impl Fn(i32, i32) -> gpui_kit::Point<Pixels>,
    family: &SharedString,
    font_size: Pixels,
    cursor: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(text) = snapshot.composition.as_deref() else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let top_left = cell(snapshot.cursor_col.max(0), snapshot.cursor_row.max(0));
    let run = TextRun {
        len: text.len(),
        font: Font {
            family: family.clone(),
            ..Default::default()
        },
        color: cursor,
        background_color: None,
        underline: Some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(cursor),
            wavy: false,
        }),
        strikethrough: None,
    };
    let shaped = window.text_system().shape_line(
        SharedString::from(text.to_string()),
        font_size,
        &[run],
        None,
    );
    let _ = shaped.paint(top_left, metrics.height, TextAlign::Left, None, window, cx);
}

/// Draw search matches as translucent cell rectangles under the text.
///
/// A different shape from the selection, not just a different tint: search hits are
/// outlined, the selection is filled. Two translucent fills of similar hue are hard to
/// tell apart when one sits inside the other, and the outline reads as "this matched"
/// at a glance in a way a second shade of grey does not.
fn paint_find_matches(
    snapshot: &GridSnapshot,
    metrics: CellMetrics,
    cell: impl Fn(i32, i32) -> gpui_kit::Point<Pixels>,
    cursor: Hsla,
    window: &mut Window,
) {
    for rect in &snapshot.find_matches {
        if rect.len <= 0 {
            continue;
        }
        let bounds = Bounds::new(
            cell(rect.col, rect.row),
            size(metrics.width * rect.len as f32, metrics.height),
        );
        window.paint_quad(fill(bounds, cursor.opacity(0.18)));
    }
}

/// Draw the selection as translucent cell rectangles under the text.
///
/// The rectangles come from `TermBuffer::selection_rects_visible`, which already
/// clipped them to the visible window and measured them in grid cells — including
/// the two-cells-per-glyph rule for CJK, so a selection over wide characters lines
/// up with them rather than drifting (#132).
///
/// Translucent, not opaque: the highlight sits under the glyphs, and a solid fill
/// would replace the text's own background colours and make coloured output
/// unreadable exactly when the user is trying to read it closely enough to select
/// it. The same reasoning as the cursor block.
fn paint_selection(
    snapshot: &GridSnapshot,
    metrics: CellMetrics,
    cell: impl Fn(i32, i32) -> gpui_kit::Point<Pixels>,
    cursor: Hsla,
    window: &mut Window,
) {
    for rect in &snapshot.selection {
        if rect.len <= 0 {
            continue;
        }
        window.paint_quad(fill(
            Bounds::new(
                cell(rect.col, rect.row),
                size(metrics.width * rect.len as f32, metrics.height),
            ),
            cursor.opacity(0.25),
        ));
    }
}

/// Draw the scrollback indicator over the right edge of the grid.
///
/// An overlay rather than a column of its own, which is what Terminal.app does and what
/// keeps the grid at its full column count — a terminal that loses a column to its own
/// scrollbar reflows the user's output to show it.
///
/// Drawn in the terminal's own foreground colour at low opacity, because it sits on top
/// of text: a widget library's scrollbar would arrive with its own track, its own
/// hover state and its own idea of how thick it should be, none of which belongs on top
/// of a character grid.
///
/// Nothing is drawn when there is no scrollback, which is most of the time — a terminal
/// that has just opened shows no indicator at all rather than a full-height thumb that
/// cannot move.
fn paint_scrollbar(
    snapshot: &GridSnapshot,
    bounds: Bounds<Pixels>,
    metrics: CellMetrics,
    cursor: Hsla,
    window: &mut Window,
) {
    if snapshot.scroll_max <= 0 {
        return;
    }
    const THICKNESS: f32 = 4.0;
    const MIN_THUMB: f32 = 24.0;

    let visible_rows = f32::from(metrics.height);
    let total_rows = visible_rows + snapshot.scroll_max as f32;
    let track_height = f32::from(bounds.size.height);
    // Proportional to what is on screen, but never so short it cannot be grabbed.
    let thumb_height = (track_height * (visible_rows / total_rows)).max(MIN_THUMB);
    // `scroll_offset` counts backwards from the live bottom, so 0 is the bottom and
    // `scroll_max` is the top. The thumb follows the same convention: at the bottom
    // when the view is at the bottom.
    let travel = (track_height - thumb_height).max(0.0);
    let progress = snapshot.scroll_offset as f32 / snapshot.scroll_max as f32;
    let thumb_top = f32::from(bounds.origin.y) + travel * progress;

    window.paint_quad(fill(
        Bounds::new(
            point(
                bounds.origin.x + px(f32::from(bounds.size.width) - THICKNESS),
                px(thumb_top),
            ),
            size(px(THICKNESS), px(thumb_height)),
        ),
        cursor.opacity(0.25),
    ));
}

/// Draw one run of text at its column.
///
/// The run is shaped on its own and painted at the cell origin, never laid out
/// relative to the run before it. A span's `col` is the authority on where it goes;
/// the shaping only decides how the glyphs inside that span are spaced.
#[allow(clippy::too_many_arguments)]
fn paint_text_span(
    span: &TermSpan,
    top_left: Point<Pixels>,
    metrics: CellMetrics,
    family: &SharedString,
    font_size: Pixels,
    bold: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let mut run_font = Font {
        family: family.clone(),
        ..Default::default()
    };
    // The setting is the *enabled* flag for bold runs, not a font weight: a session that
    // asked for bold while the setting is off draws regular, which is what a reader who
    // cannot read bold wants.
    if span.bold && bold {
        run_font.weight = FontWeight::BOLD;
    }
    let run = TextRun {
        len: span.text.len(),
        font: run_font,
        color: rgba_to_hsla(span.fg),
        background_color: None,
        underline: None,
        strikethrough: None,
    };

    let shaped = window.text_system().shape_line(
        SharedString::from(span.text.clone()),
        font_size,
        &[run],
        // Force the span onto the grid: the layout re-sits glyph *i* at
        // `i * force_width`, so the stride is ONE CELL — the same arithmetic
        // the cursor, the selection and the find matches use. Without it a
        // proportional face (the bundled MiSans) shapes a span narrower than
        // its cells and everything anchored to cell columns drifts right of
        // the text it points at. Spans never mix widths — the row builder
        // gives a wide character a span of its own — so one stride answers
        // for every glyph in the span. Shaped results are cached per
        // (text, font, size, width).
        Some(metrics.width),
    );

    // Painted at the cell's top-left with the cell's height, so `ShapedLine::paint`
    // puts the baseline inside the cell from the font's own ascent — which is what
    // keeps a row of differently-sized spans on one baseline. The shape's own width
    // is deliberately not used to place anything: the next span's position comes
    // from its `col`.
    let _ = shaped.paint(top_left, metrics.height, TextAlign::Left, None, window, cx);
}

/// Draw one emoji bitmap into the columns the terminal reserved for it.
///
/// The image is scaled to the cell box rather than drawn at its natural size: a
/// Twemoji PNG is 72x72 and a terminal cell is around 8x16, so at natural size one
/// emoji would cover the rest of the row.
fn paint_emoji_span(
    image: &EmojiImage,
    top_left: Point<Pixels>,
    metrics: CellMetrics,
    cells: i32,
    window: &mut Window,
) {
    let target = Bounds::new(top_left, size(metrics.width * cells as f32, metrics.height));
    let Some(render) = render_image(image) else {
        return;
    };
    let _ = window.paint_image(
        target,
        target,
        (0.).into(),
        std::sync::Arc::new(render),
        0,
        false,
    );
}

/// Build a GPUI image from decoded RGBA bytes.
///
/// `RenderImage` wants BGRA, and the terminal layer stores RGBA because that is
/// what the decoders produce and what every other consumer of those bytes expects.
/// The channel swap happens here, once, at the boundary — the alternative is
/// storing the bytes in a toolkit's channel order inside the framework-agnostic
/// layer.
///
/// Returns `None` rather than panicking when the buffer is not the size its
/// dimensions claim, because a malformed emoji asset should cost one glyph and not
/// a frame.
fn render_image(image: &EmojiImage) -> Option<RenderImage> {
    let expected = image.width as usize * image.height as usize * 4;
    if image.rgba.len() < expected || expected == 0 {
        return None;
    }
    let mut bgra = Vec::with_capacity(expected);
    for pixel in image.rgba[..expected].chunks_exact(4) {
        bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    // Still an `RgbaImage` as far as the type is concerned, because that is the
    // buffer GPUI's own conversions take and the channel order is a convention
    // documented on `RenderImage` rather than carried in its type. Naming it
    // honestly would mean an `ImageBuffer<Bgra<u8>>`, which nothing accepts.
    let buffer: image::RgbaImage = image::ImageBuffer::from_raw(image.width, image.height, bgra)?;
    Some(RenderImage::new([image::Frame::new(buffer)]))
}

/// Draw the cursor as a filled cell.
///
/// A block, not an outline: the terminal moves the cursor constantly and a shape
/// that changes what is under it would make the character at the cursor flicker
/// between two renderings. The block is drawn over the cell instead, which is what
/// a real terminal does.
fn paint_cursor(
    snapshot: &GridSnapshot,
    metrics: CellMetrics,
    cell: impl Fn(i32, i32) -> gpui_kit::Point<Pixels>,
    style: CursorStyle,
    cursor: Hsla,
    window: &mut Window,
) {
    if snapshot.cursor_row < 0 || snapshot.cursor_col < 0 {
        return;
    }
    // Past the last row the program wrote, there is no cursor to show: `rows_used`
    // is how many rows the session has actually filled, and a shell that has
    // printed one prompt has its cursor on that row rather than on an empty screen
    // below it. Drawing it anyway would put a block on blank space.
    if snapshot.cursor_row >= snapshot.rows_used {
        return;
    }
    let top_left = cell(snapshot.cursor_col, snapshot.cursor_row);
    // A block is painted translucent so the character beneath it stays legible: an
    // opaque block would hide exactly what the user is about to type over. The two thin
    // shapes cover almost nothing, so they are drawn solid — a half-transparent
    // one-pixel bar is a cursor nobody can find.
    let (bounds, colour) = match style {
        CursorStyle::Block => (
            Bounds::new(top_left, size(metrics.width, metrics.height)),
            cursor.opacity(0.7),
        ),
        CursorStyle::Bar => (
            Bounds::new(
                top_left,
                size(px(BAR_THICKNESS), px(f32::from(metrics.height))),
            ),
            cursor,
        ),
        CursorStyle::Underline => (
            Bounds::new(
                point(
                    top_left.x,
                    top_left.y + px(f32::from(metrics.height) - BAR_THICKNESS),
                ),
                size(metrics.width, px(BAR_THICKNESS)),
            ),
            cursor,
        ),
    };
    window.paint_quad(fill(bounds, colour));
}

/// How thick the bar and underline cursors are. Two pixels is the conventional width
/// and is visible at every font size the settings allow.
const BAR_THICKNESS: f32 = 2.0;

/// The terminal layer's colour, as a GPUI one.
///
/// Straight alpha, because the terminal's alpha means what alpha means: a
/// transparent background has to let the theme through. A premultiplied conversion
/// would darken every semi-transparent cell.
pub(crate) fn rgba_to_hsla(color: Rgba) -> Hsla {
    gpui_kit::Rgba {
        r: color.r as f32 / 255.0,
        g: color.g as f32 / 255.0,
        b: color.b as f32 / 255.0,
        a: color.a as f32 / 255.0,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The channel order swap, which is the one thing in this file that is pure
    /// arithmetic and therefore testable without a window.
    #[test]
    fn a_render_image_is_built_from_rgba_in_bgra_order() {
        let image = EmojiImage {
            width: 1,
            height: 1,
            rgba: std::sync::Arc::new(vec![0x11, 0x22, 0x33, 0x44]),
        };
        let render = render_image(&image).expect("a 1x1 image is well formed");
        assert_eq!(render.size(0).width.0, 1);
        assert_eq!(render.size(0).height.0, 1);
        // BGRA: the red and blue channels are exchanged, alpha is not.
        assert_eq!(render.as_bytes(0), Some(&[0x33, 0x22, 0x11, 0x44][..]));
    }

    #[test]
    fn a_short_buffer_is_refused_rather_than_read_out_of_bounds() {
        // A malformed asset should cost one glyph, not a frame: the alternative is
        // an index panic inside `paint`, which takes the whole window down.
        let image = EmojiImage {
            width: 8,
            height: 8,
            rgba: std::sync::Arc::new(vec![0; 4]),
        };
        assert!(render_image(&image).is_none());
    }

    #[test]
    fn a_zero_sized_image_is_refused() {
        let image = EmojiImage {
            width: 0,
            height: 0,
            rgba: std::sync::Arc::new(Vec::new()),
        };
        assert!(render_image(&image).is_none());
    }

    #[test]
    fn alpha_survives_the_conversion_to_hsla() {
        // A transparent background must stay transparent: it is how a cell with no
        // explicit background lets the terminal's own show through, and it is the
        // property the whole "paint only when alpha is non-zero" path depends on.
        assert_eq!(rgba_to_hsla(Rgba::transparent()).a, 0.0);
        assert_eq!(rgba_to_hsla(Rgba::rgb(0xff, 0x00, 0x00)).a, 1.0);
    }

    #[test]
    fn a_grid_of_the_measured_cell_size_holds_whole_cells() {
        // The property the PTY's dimensions depend on: a 800x400 area of 8x16 cells
        // is 100 columns and 25 rows, and a partial cell at the edge is not a column.
        let metrics = CellMetrics {
            width: px(8.),
            height: px(16.),
        };
        assert_eq!(metrics.grid_size(size(px(800.), px(400.))), (100, 25));
        assert_eq!(metrics.grid_size(size(px(807.), px(415.))), (100, 25));
    }

    #[test]
    fn a_window_smaller_than_one_cell_still_asks_for_one() {
        // Zero columns is a PTY that errors rather than a PTY that is narrow, and a
        // window can genuinely be smaller than a cell for a frame mid-resize.
        let metrics = CellMetrics {
            width: px(8.),
            height: px(16.),
        };
        assert_eq!(metrics.grid_size(size(px(0.), px(0.))), (1, 1));
        assert_eq!(metrics.grid_size(size(px(3.), px(3.))), (1, 1));
    }
}
