//! Registered terminal key bindings and real clipboard routing, with no shell.
use super::*;
use gpui_kit::component::{Root, WindowExt as _};
use gpui_kit::gpui::{Entity, TestAppContext, VisualTestContext};

struct Harness {
    terminal: Entity<TerminalView>,
}

impl Render for Harness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.terminal.clone())
    }
}

struct Fixture {
    view: Entity<TerminalView>,
    input: tokio::sync::mpsc::UnboundedReceiver<SessionCommand>,
    // Only an inert task keeps the synthetic session connected. Nothing runs
    // clipboard contents, and all PTY input is captured in the channel above.
    _runtime: tokio::runtime::Runtime,
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn fixture(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::ui::actions::init(cx);
    });
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (commands, input) = tokio::sync::mpsc::unbounded_channel();
    let handles = Rc::new(std::cell::RefCell::new(HashMap::from([(
        "insert-fixture".into(),
        SessionHandle {
            tab_id: "insert-fixture".into(),
            commands,
            join: runtime.spawn(std::future::pending::<()>()),
        },
    )])));
    let mut view = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let terminal = cx.new(|cx| {
            TerminalView::new(
                "insert-fixture".into(),
                TerminalSettings {
                    wallpaper: Default::default(),
                    wallpaper_asset_dir: Default::default(),
                    family: "test".into(),
                    font_size: 13,
                    bold: false,
                    padding: false,
                    line_spacing: 1.0,
                    cursor_style: CursorStyle::Block,
                    cursor_color: None,
                    highlight: terminal::OutputHighlightPreset::Off,
                    rules: vec![],
                    review_multiline_paste: true,
                    paste_shortcuts: true,
                },
                handles,
                Arc::new(Mutex::new(HashMap::new())),
                Arc::new(Mutex::new(HashMap::new())),
                Arc::new(Mutex::new(HashMap::new())),
                Arc::new(Mutex::new(crate::core::TransferStore::new())),
                Arc::new(Mutex::new(HashMap::new())),
                Arc::new(Mutex::new(None)),
                tokio::sync::mpsc::unbounded_channel().1,
                None,
                cx,
            )
        });
        view = Some(terminal.clone());
        let host = cx.new(|_| Harness { terminal });
        Root::new(host, window, cx)
    });
    let view = view.unwrap();
    cx.update(|window, cx| {
        window.activate_window();
        let focus = view.read(cx).focus.clone();
        window.focus(&focus, cx);
    });
    draw(cx);
    (
        Fixture {
            view,
            input,
            _runtime: runtime,
        },
        cx,
    )
}

fn raw_input(fixture: &mut Fixture) -> Vec<u8> {
    let mut bytes = Vec::new();
    while let Ok(command) = fixture.input.try_recv() {
        if let SessionCommand::RawInput(input) = command {
            bytes.extend(input);
        }
    }
    bytes
}

fn clipboard(text: &str, cx: &mut VisualTestContext) {
    cx.update(|_, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.into())));
}

#[gpui_kit::gpui::test]
fn selection_clamps_rows_and_columns_independently(cx: &mut TestAppContext) {
    let (fixture, cx) = fixture(cx);
    for (cols, rows) in [(120, 20), (18, 65)] {
        fixture.view.update(cx, |view, _| {
            view.metrics = Some(CellMetrics { width: px(8.), height: px(16.) });
            view.grid_bounds.set(Some(Bounds::new(
                gpui_kit::point(px(10.), px(20.)),
                gpui_kit::size(px(cols as f32 * 8.), px(rows as f32 * 16.)),
            )));
            assert_eq!(view.clamped_cell((-10, -20)), (0, 0));
            assert_eq!(view.clamped_cell((rows - 1, cols - 1)), ((rows - 1) as u16, (cols - 1) as u16));
            assert_eq!(view.clamped_cell((rows + 50, cols + 50)), ((rows - 1) as u16, (cols - 1) as u16));
            assert_eq!(view.clamped_cell((rows - 2, cols - 2)), ((rows - 2) as u16, (cols - 2) as u16));
        });
    }
}

#[gpui_kit::gpui::test]
fn real_drag_copies_both_directions_in_wide_and_tall_terminal_panes(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    for (width, height) in [(960., 360.), (240., 900.)] {
        cx.simulate_resize(gpui_kit::size(px(width), px(height)));
        for padding in [false, true] {
            fixture.view.update(cx, |view, cx| { view.padding = padding; cx.notify(); });
            draw(cx);
            let (bounds, metrics, cols, rows) = fixture.view.read_with(cx, |view, _| {
                let bounds = view.grid_bounds.get().expect("painted grid");
                let metrics = view.metrics.expect("measured grid");
                let (cols, rows) = metrics.grid_size(bounds.size);
                (bounds, metrics, cols, rows)
            });
            assert!(cols > 9 && rows > 3 && cols != rows);
            let lines: Vec<_> = (0..rows)
                .map(|row| format!("行-{row:03}-{}", "x".repeat(usize::from(cols - 9))))
                .collect();
            let expected = lines.join("\n");
            fixture.view.update(cx, |view, cx| {
                {
                    let mut buffer = view.buffer.lock().unwrap();
                    *buffer = TermBuffer::new(rows, cols);
                    buffer.ingest(lines.join("\r\n").as_bytes());
                }
                view.refresh();
                cx.notify();
            });
            draw(cx);
            let at = |row: u16, col: u16| gpui_kit::point(
                bounds.left() + metrics.width * (f32::from(col) + 0.5),
                bounds.top() + metrics.height * (f32::from(row) + 0.5),
            );
            for reverse in [false, true] {
                clipboard("unchanged-until-drag", cx);
                let (start, end) = if reverse {
                    (at(rows - 1, cols - 1), at(0, 0))
                } else {
                    (at(0, 0), at(rows - 1, cols - 1))
                };
                cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
                cx.simulate_mouse_move(end, MouseButton::Left, Default::default());
                cx.simulate_mouse_up(end, MouseButton::Left, Default::default());
                draw(cx);
                assert_eq!(
                    cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
                    Some(expected.clone()),
                    "cols={cols}, rows={rows}, padding={padding}, reverse={reverse}",
                );
                let last = fixture.view.read_with(cx, |view, _| {
                    view.snapshot.selection.last().map(|rect| (rect.row, rect.col, rect.len))
                });
                assert_eq!(last, Some((i32::from(rows - 1), 0, i32::from(cols))));
                assert!(raw_input(&mut fixture).is_empty(), "a local drag sends no PTY bytes");
            }
        }
    }
}

#[gpui_kit::gpui::test]
fn disabled_shift_insert_sends_its_modifier_without_pasting_or_submitting(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    fixture
        .view
        .update(cx, |view, _| view.paste_shortcuts = false);
    let payload = "clipboard-must-not-run\nsecond-line\n";
    clipboard(payload, cx);
    for _ in 0..2 {
        cx.simulate_keystrokes("shift-insert");
        assert_eq!(raw_input(&mut fixture), b"\x1b[2;2~");
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }
    cx.simulate_keystrokes("ctrl-alt-v");
    assert_eq!(raw_input(&mut fixture), b"\x16");
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some(payload.into())
    );
}

#[gpui_kit::gpui::test]
fn ordinary_insert_and_modifier_combinations_use_named_key_sequences(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    for (chord, expected) in [
        ("insert", &b"\x1b[2~"[..]),
        ("alt-insert", &b"\x1b[2;3~"[..]),
        ("alt-shift-insert", &b"\x1b[2;4~"[..]),
        ("ctrl-insert", &b"\x1b[2;5~"[..]),
        ("ctrl-alt-insert", &b"\x1b[2;7~"[..]),
        ("ctrl-shift-insert", &b"\x1b[2;6~"[..]),
        ("ctrl-alt-shift-insert", &b"\x1b[2;8~"[..]),
    ] {
        cx.simulate_keystrokes(chord);
        assert_eq!(raw_input(&mut fixture), expected, "{chord}");
    }
}

#[gpui_kit::gpui::test]
fn toggling_extra_paste_preserves_standard_paste_and_bracketed_payloads(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    let text = "literal insert 中文";
    clipboard(text, cx);
    for enabled in [true, false, true] {
        fixture
            .view
            .update(cx, |view, _| view.paste_shortcuts = enabled);
        for bracketed in [false, true] {
            fixture.view.update(cx, |view, _| {
                view.buffer.lock().unwrap().parser.process(if bracketed {
                    b"\x1b[?2004h"
                } else {
                    b"\x1b[?2004l"
                });
            });
            for chord in ["ctrl-v", "ctrl-shift-v", "shift-insert", "ctrl-alt-v"] {
                cx.simulate_keystrokes(chord);
                let expected = match (enabled, chord) {
                    (false, "shift-insert") => b"\x1b[2;2~".to_vec(),
                    (false, "ctrl-alt-v") => vec![0x16],
                    _ if bracketed => format!("\x1b[200~{text}\x1b[201~").into_bytes(),
                    _ => text.as_bytes().to_vec(),
                };
                assert_eq!(
                    raw_input(&mut fixture),
                    expected,
                    "enabled={enabled}, bracketed={bracketed}, {chord}"
                );
            }
        }
    }
}

#[gpui_kit::gpui::test]
fn enabled_extra_paste_still_reviews_multiline_and_cancel_sends_nothing(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    clipboard("first-line\nsecond-line\n", cx);
    for chord in ["shift-insert", "ctrl-alt-v"] {
        cx.simulate_keystrokes(chord);
        draw(cx);
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(raw_input(&mut fixture).is_empty());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(raw_input(&mut fixture).is_empty());
        cx.update(|window, cx| {
            let focus = fixture.view.read(cx).focus.clone();
            window.focus(&focus, cx);
        });
        draw(cx);
    }
}

#[gpui_kit::gpui::test]
fn middle_paste_respects_the_extra_setting(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    clipboard("middle-paste", cx);
    for enabled in [true, false, true] {
        fixture
            .view
            .update(cx, |view, _| view.paste_shortcuts = enabled);
        let center = fixture.view.read_with(cx, |view, _| {
            view.grid_bounds
                .get()
                .expect("painted terminal grid")
                .center()
        });
        cx.simulate_mouse_down(center, MouseButton::Middle, Default::default());
        cx.simulate_mouse_up(center, MouseButton::Middle, Default::default());
        assert_eq!(
            raw_input(&mut fixture),
            if enabled {
                &b"middle-paste"[..]
            } else {
                &[][..]
            }
        );
    }
}

#[gpui_kit::gpui::test]
fn printable_text_and_existing_control_keys_are_unchanged(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    cx.simulate_input("insert 中文");
    assert_eq!(raw_input(&mut fixture), "insert 中文".as_bytes());
    for (chord, expected) in [
        ("ctrl-c", &b"\x03"[..]),
        ("ctrl-x", &b"\x18"[..]),
        ("up", &b"\x1b[A"[..]),
        ("backspace", &b"\x7f"[..]),
    ] {
        cx.simulate_keystrokes(chord);
        assert_eq!(raw_input(&mut fixture), expected, "{chord}");
    }
}

#[gpui_kit::gpui::test]
fn wallpaper_bounds_follow_the_grid_and_exclude_status_and_find_bars(cx: &mut TestAppContext) {
    use crate::wallpaper::{Scope, WallpaperSettings};
    let (fixture, cx) = fixture(cx);
    cx.simulate_resize(gpui_kit::size(px(800.), px(500.)));
    for scope in [Scope::TerminalPane, Scope::ContinuousTerminalCanvas] {
        for padding in [false, true] {
            for (status_open, find_open) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                fixture.view.update(cx, |view, cx| {
                    // Geometry must be correct before an image finishes loading too.
                    let settings = WallpaperSettings {
                        enabled: true,
                        scope,
                        ..Default::default()
                    };
                    view.wallpaper = Some(cx.new(|cx| {
                        super::super::wallpaper_layer::WallpaperLayer::new_in(
                            settings,
                            Default::default(),
                            cx,
                        )
                    }));
                    view.wallpaper_enabled = true;
                    view.padding = padding;
                    view.conn_state = 1;
                    view.status = status_open.then(|| "Connected to test host".into());
                    view.find_open = find_open;
                    view.find_height = if find_open { FIND_BAR_HEIGHT } else { 0. };
                    view.find_query = "test".into();
                    cx.notify();
                });
                draw(cx);
                let wallpaper = cx
                    .debug_bounds("terminal-wallpaper")
                    .expect("wallpaper host");
                let container = cx
                    .debug_bounds("terminal-grid-container")
                    .expect("grid container");
                let grid = fixture
                    .view
                    .read_with(cx, |view, _| view.grid_bounds.get().unwrap());
                assert_eq!(wallpaper, container,
                    "wallpaper must use grid bounds: scope={scope:?}, padding={padding}, status={status_open}, find={find_open}");
                if !padding {
                    assert_eq!(
                        wallpaper, grid,
                        "unpadded wallpaper and terminal cells share bounds"
                    );
                } else {
                    assert!(
                        grid.origin.x > wallpaper.origin.x && grid.origin.y > wallpaper.origin.y
                    );
                    assert!(grid.bottom_right().x < wallpaper.bottom_right().x);
                    assert!(grid.bottom_right().y < wallpaper.bottom_right().y);
                }
                for (selector, visible) in [
                    ("terminal-status-line", status_open),
                    ("terminal-find-bar", find_open),
                ] {
                    let bar = cx.debug_bounds(selector);
                    assert_eq!(bar.is_some(), visible, "{selector}");
                    if let Some(bar) = bar {
                        assert!(bar.size.height > px(0.));
                        assert!(bar.bottom_right().y <= wallpaper.origin.y,
                            "{selector} must remain above the wallpaper, even while both bars are open");
                    }
                }
            }
        }
    }
    fixture.view.update(cx, |view, cx| {
        view.wallpaper = None;
        view.wallpaper_enabled = false;
        cx.notify();
    });
    draw(cx);
    assert!(
        cx.debug_bounds("terminal-wallpaper").is_none(),
        "disabled wallpaper adds no layer"
    );
}
