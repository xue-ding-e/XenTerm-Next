//! Registered terminal key bindings and real clipboard routing, with no shell.
use super::*;
use gpui_kit::component::{Root, WindowExt as _};
use gpui_kit::Subscription;
use gpui_kit::gpui::{Entity, TestAppContext, VisualTestContext};

// In toolkit 0.6 the application content renders the Root-owned dialog layer.
// Mirror that host so Escape reaches the real paste-review dialog in these tests.
struct Harness {
    terminal: Entity<TerminalView>,
    root_subscription: Option<Subscription>,
}

impl Render for Harness {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.root_subscription.is_none() {
            self.root_subscription = crate::ui::follow_root(window, cx);
        }
        div()
            .size_full()
            .child(self.terminal.clone())
            .children(Root::render_dialog_layer(window, cx))
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
        let host = cx.new(|_| Harness {
            terminal,
            root_subscription: None,
        });
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
