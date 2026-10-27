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

fn select_fixture_output(fixture: &Fixture, cx: &mut VisualTestContext) {
    fixture.view.update(cx, |view, cx| {
        {
            let mut buffer = view.buffer.lock().unwrap();
            buffer.ingest(b"\x1b[2J\x1b[Hselected output");
            buffer.begin_selection(0, 0, false, false);
            buffer.extend_selection(0, 14);
        }
        view.refresh();
        cx.notify();
    });
    draw(cx);
    assert_eq!(selected_output(fixture, cx), "selected output");
}

fn selected_output(fixture: &Fixture, cx: &mut VisualTestContext) -> String {
    fixture.view.read_with(cx, |view, _| view.buffer.lock().unwrap().extract_selection_text())
}

fn link_fixture_output(fixture: &Fixture, text: &str, cx: &mut VisualTestContext) {
    fixture.view.update(cx, |view, cx| {
        let bounds = view.grid_bounds.get().unwrap();
        let (cols, rows) = view.metrics.unwrap().grid_size(bounds.size);
        {
            let mut buffer = view.buffer.lock().unwrap();
            *buffer = TermBuffer::new(rows, cols);
            buffer.ingest(text.as_bytes());
        }
        view.refresh();
        cx.notify();
    });
    draw(cx);
}

fn link_cell(fixture: &Fixture, row: u16, col: u16, cx: &mut VisualTestContext) -> gpui_kit::Point<Pixels> {
    fixture.view.read_with(cx, |view, _| {
        let bounds = view.grid_bounds.get().unwrap();
        let metrics = view.metrics.unwrap();
        gpui_kit::point(
            bounds.left() + metrics.width * (f32::from(col) + 0.5),
            bounds.top() + metrics.height * (f32::from(row) + 0.5),
        )
    })
}

fn terminal_link_modifier() -> gpui_kit::Modifiers {
    gpui_kit::Modifiers::secondary_key()
}

#[test]
fn terminal_link_modifier_matches_platform_without_stealing_other_chords() {
    assert!(link_modifiers(terminal_link_modifier()));
    assert_eq!(link_modifiers(gpui_kit::Modifiers { control: true, ..Default::default() }), !cfg!(target_os = "macos"));
    assert_eq!(link_modifiers(gpui_kit::Modifiers { platform: true, ..Default::default() }), cfg!(target_os = "macos"));
    assert!(!link_modifiers(gpui_kit::Modifiers { control: true, platform: true, ..Default::default() }));
}

#[gpui_kit::gpui::test]
fn terminal_link_modifier_release_and_repress_cannot_resurrect_click(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    link_fixture_output(&fixture, "\x1b[?1000h\x1b[?1006hhttps://example.test/path", cx);
    let at = link_cell(&fixture, 0, 10, cx);
    cx.simulate_mouse_down(at, MouseButton::Left, terminal_link_modifier());
    cx.simulate_modifiers_change(Default::default());
    cx.simulate_modifiers_change(terminal_link_modifier());
    cx.simulate_mouse_up(at, MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url(), None);
    assert!(raw_input(&mut fixture).is_empty());
}

#[gpui_kit::gpui::test]
fn terminal_link_pointer_leave_and_return_cannot_resurrect_click(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    link_fixture_output(&fixture, "\x1b[?1000h\x1b[?1006hhttps://example.test/path", cx);
    let at = link_cell(&fixture, 0, 10, cx);
    cx.simulate_mouse_down(at, MouseButton::Left, terminal_link_modifier());
    cx.simulate_mouse_move(gpui_kit::point(px(-20.), px(-20.)), MouseButton::Left, terminal_link_modifier());
    cx.simulate_mouse_move(at, MouseButton::Left, terminal_link_modifier());
    cx.simulate_mouse_up(at, MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url(), None);
    assert!(raw_input(&mut fixture).is_empty());
}

#[gpui_kit::gpui::test]
fn terminal_link_focus_and_activation_loss_cancel_even_after_restore(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    link_fixture_output(&fixture, "\x1b[?1000h\x1b[?1006hhttps://example.test/path", cx);
    let at = link_cell(&fixture, 0, 10, cx);
    cx.simulate_mouse_down(at, MouseButton::Left, terminal_link_modifier());
    let other = cx.update(|window, cx| {
        let other = cx.focus_handle();
        window.focus(&other, cx);
        other
    });
    draw(cx);
    cx.update(|window, cx| {
        let focus = fixture.view.read(cx).focus.clone();
        window.focus(&focus, cx);
    });
    draw(cx);
    cx.simulate_mouse_up(at, MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url(), None);
    drop(other);

    cx.simulate_mouse_down(at, MouseButton::Left, terminal_link_modifier());
    cx.deactivate_window();
    draw(cx);
    cx.update(|window, _| window.activate_window());
    draw(cx);
    cx.simulate_mouse_up(at, MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url(), None);
    assert!(raw_input(&mut fixture).is_empty());
}

#[gpui_kit::gpui::test]
fn terminal_link_opens_only_on_release_without_copying_or_sending_input(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    link_fixture_output(&fixture, "https://example.test:8443/a?b=1&c=2\r\nkeep", cx);
    fixture.view.update(cx, |view, _| {
        let mut buffer = view.buffer.lock().unwrap();
        buffer.begin_selection(1, 0, false, false);
        buffer.extend_selection(1, 3);
    });
    clipboard("clipboard-stays", cx);
    let at = link_cell(&fixture, 0, 10, cx);
    cx.simulate_mouse_down(at, MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url(), None, "press alone must not open a browser");
    cx.simulate_mouse_up(at, MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.test:8443/a?b=1&c=2"));
    assert_eq!(selected_output(&fixture, cx), "keep");
    assert_eq!(cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())).as_deref(), Some("clipboard-stays"));
    assert!(raw_input(&mut fixture).is_empty());
    // A fresh click after the previous release must use the new output.
    link_fixture_output(&fixture, "http://example.test/next", cx);
    cx.simulate_click(at, terminal_link_modifier());
    assert_eq!(cx.opened_url().as_deref(), Some("http://example.test/next"));
}

#[gpui_kit::gpui::test]
fn terminal_link_does_not_change_plain_click_or_other_modifier_chords(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    link_fixture_output(&fixture, "https://example.test/path", cx);
    let at = link_cell(&fixture, 0, 10, cx);
    for modifiers in [
        Default::default(),
        gpui_kit::Modifiers { shift: true, ..terminal_link_modifier() },
        gpui_kit::Modifiers { alt: true, ..terminal_link_modifier() },
        gpui_kit::Modifiers { control: true, platform: true, ..Default::default() },
        gpui_kit::Modifiers { function: true, ..terminal_link_modifier() },
    ] {
        cx.simulate_click(at, modifiers);
        assert_eq!(cx.opened_url(), None);
    }
    assert!(raw_input(&mut fixture).is_empty());
}

#[gpui_kit::gpui::test]
fn terminal_link_drag_cancels_open_and_preserves_additive_selection(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    link_fixture_output(&fixture, "https://example.test/path\r\nkeep", cx);
    fixture.view.update(cx, |view, _| {
        let mut buffer = view.buffer.lock().unwrap();
        buffer.begin_selection(1, 0, false, false);
        buffer.extend_selection(1, 3);
    });
    let start = link_cell(&fixture, 0, 0, cx);
    let end = link_cell(&fixture, 0, 4, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, terminal_link_modifier());
    cx.simulate_mouse_move(end, MouseButton::Left, terminal_link_modifier());
    cx.simulate_mouse_up(end, MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url(), None);
    let expected = if cfg!(target_os = "macos") { "https" } else { "keep\nhttps" };
    assert_eq!(selected_output(&fixture, cx), expected);
    assert_eq!(cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())).as_deref(), Some(expected));
    assert!(raw_input(&mut fixture).is_empty());
}

#[gpui_kit::gpui::test]
fn terminal_link_cancelled_modifier_output_escape_and_outside_release_never_open(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    link_fixture_output(&fixture, "https://example.test/first", cx);
    let at = link_cell(&fixture, 0, 10, cx);
    cx.simulate_mouse_down(at, MouseButton::Left, terminal_link_modifier());
    cx.simulate_mouse_up(at, MouseButton::Left, Default::default());
    assert_eq!(cx.opened_url(), None);

    cx.simulate_mouse_down(at, MouseButton::Left, terminal_link_modifier());
    link_fixture_output(&fixture, "https://example.test/changed", cx);
    cx.simulate_mouse_up(at, MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url(), None);

    cx.simulate_mouse_down(at, MouseButton::Left, terminal_link_modifier());
    cx.simulate_keystrokes("escape");
    cx.simulate_mouse_up(at, MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url(), None);
    // Escape may be forwarded normally; it is not a browser action.
    let _ = raw_input(&mut fixture);

    cx.simulate_mouse_down(at, MouseButton::Left, terminal_link_modifier());
    cx.simulate_mouse_up(gpui_kit::point(px(-20.), px(-20.)), MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url(), None);
    assert!(fixture.view.read_with(cx, |view, _| view.link_press.is_none()));
    cx.simulate_mouse_up(at, MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url(), None, "a release cannot reuse a cancelled press");
}

#[gpui_kit::gpui::test]
fn terminal_link_has_priority_over_mouse_reporting_only_on_a_safe_link(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    link_fixture_output(&fixture, "\x1b[?1000h\x1b[?1006hhttps://example.test/path\r\nplain", cx);
    let link = link_cell(&fixture, 0, 10, cx);
    cx.simulate_click(link, Default::default());
    assert!(!raw_input(&mut fixture).is_empty(), "unmodified click still reaches the TUI");
    assert_eq!(cx.opened_url(), None);

    cx.simulate_mouse_down(link, MouseButton::Left, terminal_link_modifier());
    cx.simulate_mouse_up(link, MouseButton::Left, Default::default());
    assert!(raw_input(&mut fixture).is_empty(), "cancelled link sends no orphan release");
    assert_eq!(cx.opened_url(), None);

    let plain = link_cell(&fixture, 1, 2, cx);
    cx.simulate_click(plain, terminal_link_modifier());
    let report = raw_input(&mut fixture);
    assert!(report.starts_with(b"\x1b[<0;3;2M"));
    assert!(report.ends_with(b"\x1b[<0;3;2m"));

    cx.simulate_mouse_down(link, MouseButton::Left, terminal_link_modifier());
    cx.simulate_mouse_move(plain, MouseButton::Left, terminal_link_modifier());
    cx.simulate_mouse_up(plain, MouseButton::Left, terminal_link_modifier());
    assert!(raw_input(&mut fixture).is_empty());
    assert_eq!(cx.opened_url(), None, "TUI drag does not open");

    cx.simulate_click(link, terminal_link_modifier());
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.test/path"));
    assert!(raw_input(&mut fixture).is_empty());
}

#[gpui_kit::gpui::test]
fn terminal_link_rejects_unsafe_schemes_and_retains_non_link_multiselect(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    for text in ["file:///tmp/a", "javascript:alert(1)", "data:text/html,hello", "shell:calc"] {
        link_fixture_output(&fixture, text, cx);
        let at = link_cell(&fixture, 0, 2, cx);
        cx.simulate_click(at, terminal_link_modifier());
        assert_eq!(cx.opened_url(), None);
    }
    link_fixture_output(&fixture, "first second", cx);
    let first = link_cell(&fixture, 0, 0, cx);
    let first_end = link_cell(&fixture, 0, 4, cx);
    let second = link_cell(&fixture, 0, 6, cx);
    let second_end = link_cell(&fixture, 0, 11, cx);
    cx.simulate_mouse_down(first, MouseButton::Left, Default::default());
    cx.simulate_mouse_move(first_end, MouseButton::Left, Default::default());
    cx.simulate_mouse_up(first_end, MouseButton::Left, Default::default());
    let control = gpui_kit::Modifiers { control: true, ..Default::default() };
    cx.simulate_mouse_down(second, MouseButton::Left, control);
    cx.simulate_mouse_move(second_end, MouseButton::Left, control);
    cx.simulate_mouse_up(second_end, MouseButton::Left, control);
    assert_eq!(selected_output(&fixture, cx), "first\nsecond");
    assert_eq!(cx.opened_url(), None);
    assert!(raw_input(&mut fixture).is_empty());
}

#[gpui_kit::gpui::test]
fn terminal_link_other_button_moves_keep_tui_release_suppressed(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    link_fixture_output(&fixture, "\x1b[?1000h\x1b[?1006hhttps://example.test/path", cx);
    let at = link_cell(&fixture, 0, 10, cx);
    for pressed in [Some(MouseButton::Right), None] {
        cx.simulate_mouse_down(at, MouseButton::Left, terminal_link_modifier());
        cx.simulate_mouse_move(at, pressed, terminal_link_modifier());
        assert!(fixture.view.read_with(cx, |view, _| view.link_press.as_ref().is_some_and(|press| press.cancelled)));
        cx.simulate_mouse_up(at, MouseButton::Left, terminal_link_modifier());
        assert!(raw_input(&mut fixture).is_empty(), "no release without a remote press");
        assert_eq!(cx.opened_url(), None);
        assert!(fixture.view.read_with(cx, |view, _| view.link_press.is_none()));
    }
    cx.simulate_mouse_down(at, MouseButton::Left, terminal_link_modifier());
    cx.simulate_mouse_down(at, MouseButton::Right, terminal_link_modifier());
    cx.simulate_mouse_up(at, MouseButton::Right, terminal_link_modifier());
    cx.simulate_mouse_up(at, MouseButton::Left, terminal_link_modifier());
    assert_eq!(cx.opened_url(), None, "a second button cancels even without motion");
    assert!(raw_input(&mut fixture).is_empty());
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
fn ctrl_c_copies_selected_output_and_only_interrupts_without_selection(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    for alternate_screen in [false, true] {
        fixture.view.update(cx, |view, cx| {
            {
                let mut buffer = view.buffer.lock().unwrap();
                buffer.ingest(if alternate_screen { b"\x1b[?1049h" } else { b"\x1b[?1049l" });
                buffer.ingest("\x1b[2J\x1b[Hselected 中文".as_bytes());
                buffer.begin_selection(0, 0, false, false);
                buffer.extend_selection(0, 12);
            }
            view.refresh();
            cx.notify();
        });
        draw(cx);
        for chord in ["ctrl-c", "ctrl-shift-c", "ctrl-c"] {
            clipboard("replace-on-explicit-copy", cx);
            cx.simulate_keystrokes(chord);
            assert_eq!(
                cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
                Some("selected 中文".into()),
                "{chord}, alternate_screen={alternate_screen}",
            );
            assert!(raw_input(&mut fixture).is_empty(), "copy must not interrupt");
        }
        fixture.view.update(cx, |view, cx| {
            view.buffer.lock().unwrap().clear_selection();
            view.refresh();
            cx.notify();
        });
        draw(cx);
        clipboard("keep-without-selection", cx);
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(raw_input(&mut fixture), b"\x03");
        cx.simulate_keystrokes("ctrl-shift-c");
        assert!(raw_input(&mut fixture).is_empty());
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some("keep-without-selection".into()),
        );
    }
}

#[gpui_kit::gpui::test]
fn single_cell_word_copy_and_find_bar_do_not_send_interrupts(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    fixture.view.update(cx, |view, cx| {
        {
            let mut buffer = view.buffer.lock().unwrap();
            buffer.ingest(b"x");
            buffer.render();
            assert_eq!(buffer.select_word_at(0, 0).as_deref(), Some("x"));
        }
        view.refresh();
        cx.notify();
    });
    draw(cx);
    clipboard("not-x", cx);
    cx.simulate_keystrokes("ctrl-c");
    assert!(raw_input(&mut fixture).is_empty());
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("x".into()),
    );
    // Ctrl+C must belong to find even when a terminal selection also exists.
    cx.simulate_keystrokes("ctrl-f");
    cx.simulate_input("query");
    draw(cx);
    // Expanding find may reflow the terminal and clear old cell coordinates.
    // This is an input-routing test, not a real-time animation test: freeze the
    // final height, let the layout settle, then re-establish and assert the
    // selection so resize cannot silently turn this into the no-selection case.
    fixture.view.update(cx, |view, cx| {
        view.find_anim = None;
        view.find_height = FIND_BAR_HEIGHT;
        cx.notify();
    });
    draw(cx);
    draw(cx);
    fixture.view.update(cx, |view, _| {
        let mut buffer = view.buffer.lock().unwrap();
        buffer.render();
        assert_eq!(buffer.select_word_at(0, 0).as_deref(), Some("x"));
        assert!(!buffer.extract_selection_text().is_empty());
    });
    for selected in [true, false] {
        if !selected {
            fixture.view.update(cx, |view, _| view.buffer.lock().unwrap().clear_selection());
        }
        clipboard("find-must-not-replace-this", cx);
        cx.simulate_keystrokes("ctrl-c");
        assert!(raw_input(&mut fixture).is_empty(), "find owns the keyboard");
        assert_eq!(fixture.view.read_with(cx, |view, _| view.find_query.clone()), "query");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some("find-must-not-replace-this".into()),
        );
    }
}

#[gpui_kit::gpui::test]
fn standard_paste_after_copy_keeps_bracketed_paste_and_review_safety(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    fixture.view.update(cx, |view, cx| {
        {
            let mut buffer = view.buffer.lock().unwrap();
            buffer.ingest("copy 中文".as_bytes());
            buffer.begin_selection(0, 0, false, false);
            buffer.extend_selection(0, 8);
        }
        view.refresh();
        cx.notify();
    });
    draw(cx);
    cx.simulate_keystrokes("ctrl-c");
    assert!(raw_input(&mut fixture).is_empty());
    for bracketed in [false, true] {
        fixture.view.update(cx, |view, _| {
            view.buffer.lock().unwrap().ingest(if bracketed { b"\x1b[?2004h" } else { b"\x1b[?2004l" });
        });
        for chord in ["ctrl-v", "ctrl-shift-v"] {
            cx.simulate_keystrokes(chord);
            let expected = if bracketed { "\x1b[200~copy 中文\x1b[201~" } else { "copy 中文" };
            assert_eq!(raw_input(&mut fixture), expected.as_bytes());
        }
    }
    // Embedded escape/interrupt bytes cannot terminate a bracketed paste.
    clipboard("safe\x1b[201~\x03中文", cx);
    cx.simulate_keystrokes("ctrl-v");
    assert_eq!(raw_input(&mut fixture), "\x1b[200~safe[201~中文\x1b[201~".as_bytes());
    for chord in ["ctrl-v", "ctrl-shift-v"] {
        clipboard("first\r\nsecond\n", cx);
        cx.simulate_keystrokes(chord);
        draw(cx);
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(raw_input(&mut fixture).is_empty());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(raw_input(&mut fixture).is_empty(), "cancel must not paste or execute");
        cx.update(|window, cx| window.focus(&fixture.view.read(cx).focus.clone(), cx));
        draw(cx);
    }
}

#[gpui_kit::gpui::test]
fn accepted_user_input_clears_old_selection_but_copy_keeps_it(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    for (action, expected) in [
        ("type", &b"run"[..]),
        ("enter", &b"\r"[..]),
        ("tab", &b"\t"[..]),
        ("paste", &b"payload"[..]),
        ("platform-paste", &b"payload"[..]),
    ] {
        select_fixture_output(&fixture, cx);
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(selected_output(&fixture, cx), "selected output");
        assert!(raw_input(&mut fixture).is_empty());
        match action {
            "type" => cx.simulate_input("run"),
            "paste" => { clipboard("payload", cx); cx.simulate_keystrokes("ctrl-v"); }
            "platform-paste" => cx.update(|window, cx| fixture.view.update(cx, |view, cx| {
                view.paste(gpui_kit::ClipboardItem::new_string("payload".into()), window, cx);
            })),
            chord => cx.simulate_keystrokes(chord),
        }
        assert_eq!(raw_input(&mut fixture), expected, "{action}");
        assert!(selected_output(&fixture, cx).is_empty(), "{action}");
        draw(cx);
        assert!(fixture.view.read_with(cx, |view, _| view.snapshot.selection.is_empty()));
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(raw_input(&mut fixture), b"\x03", "new commands remain interruptible after {action}");
    }
}

#[gpui_kit::gpui::test]
fn empty_rejected_and_non_user_input_do_not_discard_selection(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    // Removing a session handle must not change the status-bar geometry here:
    // that would test resize-driven selection clearing instead of send failure.
    fixture.view.update(cx, |view, cx| {
        view.status = Some("Synthetic session".into());
        cx.notify();
    });
    draw(cx);
    draw(cx);
    select_fixture_output(&fixture, cx);
    cx.update(|window, cx| fixture.view.update(cx, |view, cx| {
        view.replace_text_in_range(None, "", window, cx);
    }));
    assert!(raw_input(&mut fixture).is_empty());
    assert_eq!(selected_output(&fixture, cx), "selected output");

    // Raw mouse/protocol traffic is not a user's new command.
    fixture.view.update(cx, |view, _| { view.send_bytes(b"\x1b[1;1R"); });
    assert_eq!(raw_input(&mut fixture), b"\x1b[1;1R");
    assert_eq!(selected_output(&fixture, cx), "selected output");

    let handle = fixture.view.update(cx, |view, _| {
        view.handles.borrow_mut().remove(&view.tab_id).unwrap()
    });
    cx.simulate_input("not-connected");
    assert!(raw_input(&mut fixture).is_empty());
    assert_eq!(selected_output(&fixture, cx), "selected output");
    fixture.view.update(cx, |view, _| {
        view.handles.borrow_mut().insert(view.tab_id.clone(), handle);
    });
    fixture.input.close();
    cx.simulate_input("queue-refuses-this");
    assert!(raw_input(&mut fixture).is_empty());
    assert_eq!(selected_output(&fixture, cx), "selected output");
}

#[gpui_kit::gpui::test]
fn preedit_and_cancelled_multiline_paste_preserve_selected_output(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    select_fixture_output(&fixture, cx);
    cx.update(|window, cx| fixture.view.update(cx, |view, cx| {
        view.replace_and_mark_text_in_range(None, "ni", Some(0..2), window, cx);
    }));
    assert!(raw_input(&mut fixture).is_empty());
    assert_eq!(selected_output(&fixture, cx), "selected output");
    cx.update(|window, cx| fixture.view.update(cx, |view, cx| view.unmark_text(window, cx)));
    assert_eq!(selected_output(&fixture, cx), "selected output");
    clipboard("first\nsecond\n", cx);
    cx.simulate_keystrokes("ctrl-v");
    draw(cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(raw_input(&mut fixture).is_empty());
    assert_eq!(selected_output(&fixture, cx), "selected output");
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(raw_input(&mut fixture).is_empty());
    assert_eq!(selected_output(&fixture, cx), "selected output");
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

#[gpui_kit::gpui::test]
fn online_enter_is_sent_once_without_text_fallback(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    // No selection or Ctrl+C precondition: reach the live Enter path directly.
    cx.simulate_keystrokes("enter");
    assert_eq!(raw_input(&mut fixture), b"\r", "online Enter must be encoded once");

    // A failed enqueue still belongs to the terminal key path, not to a text
    // fallback. Keep geometry stable so selection loss cannot come from resize.
    fixture.view.update(cx, |view, cx| {
        view.status = Some("Synthetic session".into());
        cx.notify();
    });
    draw(cx);
    draw(cx);
    select_fixture_output(&fixture, cx);
    fixture.input.close();
    assert!(fixture.view.read_with(cx, |view, _| {
        view.handles.borrow().get(&view.tab_id).unwrap().commands.is_closed()
    }));
    // Backspace reaches the encoder even with a closed queue, unlike bare
    // Enter, which correctly requests reconnection before encoding in that state.
    let result = cx.update(|window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::KeyDown(KeyDownEvent {
                keystroke: gpui_kit::Keystroke::parse("backspace").unwrap(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        )
    });
    assert!(!result.propagate, "a rejected encoded key must not fall back to text");
    assert!(raw_input(&mut fixture).is_empty());
    assert_eq!(selected_output(&fixture, cx), "selected output");
}

#[gpui_kit::gpui::test]
fn ordinary_text_and_ime_keep_the_input_handler_path(cx: &mut TestAppContext) {
    let (mut fixture, cx) = fixture(cx);
    // These printable keydowns must remain unconsumed so GPUI supplies text.
    cx.simulate_keystrokes("a shift-b");
    assert_eq!(raw_input(&mut fixture), b"aB");
    cx.simulate_input("中文");
    assert_eq!(raw_input(&mut fixture), "中文".as_bytes());

    cx.update(|window, cx| fixture.view.update(cx, |view, cx| {
        view.replace_and_mark_text_in_range(None, "ni", Some(0..2), window, cx);
    }));
    assert!(raw_input(&mut fixture).is_empty());
    assert_eq!(fixture.view.read_with(cx, |view, _| view.composition.clone()), Some("ni".into()));
    cx.update(|window, cx| fixture.view.update(cx, |view, cx| {
        view.replace_text_in_range(None, "你", window, cx);
    }));
    assert_eq!(raw_input(&mut fixture), "你".as_bytes());
    assert!(fixture.view.read_with(cx, |view, _| view.composition.is_none()));

    cx.update(|window, cx| fixture.view.update(cx, |view, cx| {
        view.replace_and_mark_text_in_range(None, "hao", Some(0..3), window, cx);
        view.unmark_text(window, cx);
    }));
    assert!(raw_input(&mut fixture).is_empty());
    assert!(fixture.view.read_with(cx, |view, _| view.composition.is_none()));
}
