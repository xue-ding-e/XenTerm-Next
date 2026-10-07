//! Dispatch the real shell keymap and command-palette action, not just its view.
//! The empty fixture never opens a terminal process, network connection or keyring.

use super::*;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use gpui_kit::test::TestWindowExt as _;
use std::sync::{Arc, Mutex};

fn fixture(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::ui::actions::init(cx);
    });
    let store = crate::config::ConfigStore {
        path: std::env::temp_dir().join(format!("xenterm-palette-{}.db", uuid::Uuid::new_v4())),
        backup_dir: None,
        cache: crate::config::ConfigFile::default(),
        key: [7; 32],
        keyring_enabled: false,
        saved_state: Mutex::new(crate::config::SavedState::default()).into(),
    };
    let state = SessionState::new(
        Arc::new(tokio::runtime::Runtime::new().unwrap()),
        Arc::new(Mutex::new(std::collections::HashMap::new())),
        Rc::new(RefCell::new(store)),
    );
    let mut shell = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let window_width = Rc::new(Cell::new(1280.));
            let terminal = cx.new(|cx| {
                crate::ui::pages::TerminalPage::new(state.clone(), window_width.clone(), window, cx)
            });
            Shell {
                pages: Pages::new(terminal),
                window_width,
                webdav_task: None,
                approval_queue: Vec::new(),
                approval_opened_at: std::collections::HashMap::new(),
                approval_reported: std::collections::HashSet::new(),
                approval_shown: std::collections::HashSet::new(),
                audit_last_prune_day: None,
                // No real approval queue or background persistence in this fixture.
                approval_poll: None,
                process_window: Detached::new(process_window_title),
                audit_window: crate::ui::AuditWindowHandle::new(),
                system_info_window: Detached::new(system_info_window_title),
                open_file: None,
                state,
                overlay: Overlay::None,
                persistence_warning: PersistenceWarning::default(),
                status: None,
                status_epoch: 0,
                status_expiry: None,
                quick_connect_pick: None,
                _quick_connect_subscription: None,
                _root_subscription: None,
            }
        });
        shell = Some(view.clone());
        Root::new(view, window, cx)
    });
    draw(cx);
    // The nav rail has real keyboard focus targets under the Shell key context.
    // Focus one without invoking a click that could open another dialog.
    cx.update(|window, cx| window.focus_next(cx));
    draw(cx);
    (shell.unwrap(), cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn open_palette(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> gpui_kit::EntityId {
    // This reaches Shell::render_body's registered on_action listener while
    // GPUI owns Shell's update lease: the original constructor panicked here.
    cx.simulate_keystrokes("ctrl-shift-p");
    draw(cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    shell.read_with(cx, |shell, _| match &shell.overlay {
        Overlay::Commands(palette) => palette.entity_id(),
        other => panic!("command action did not open its palette: {other:?}"),
    })
}

#[gpui_kit::gpui::test]
fn command_palette_action_can_cancel_and_reopen(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let first = open_palette(&shell, cx);
    // A second chord while its input is focused must not stack another palette.
    cx.simulate_keystrokes("ctrl-shift-p");
    draw(cx);
    shell.read_with(cx, |shell, _| match &shell.overlay {
        Overlay::Commands(palette) => assert_eq!(palette.entity_id(), first),
        other => panic!("palette changed while modal: {other:?}"),
    });
    for _ in 0..2 {
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        shell.read_with(cx, |shell, _| assert_eq!(shell.overlay, Overlay::None));
        assert_ne!(open_palette(&shell, cx), first);
    }
}

#[gpui_kit::gpui::test]
fn command_palette_action_focuses_filter_and_executes_enter(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    open_palette(&shell, cx);
    cx.simulate_input(crate::i18n::t("设置页", "Settings page"));
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.overlay, Overlay::None);
        assert_eq!(shell.pages.active, PageId::Settings);
    });
}

#[gpui_kit::gpui::test]
fn command_palette_wheel_reaches_and_executes_the_last_unfiltered_command(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    shell.update(cx, |shell, _| {
        shell.state.store.borrow_mut().set_theme_pref("dark".into())
    });
    open_palette(&shell, cx);
    // Let the real dialog entrance animation settle before pointer hit tests.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut previous = None;
    let mut stable = 0;
    loop {
        draw(cx);
        let bounds = cx.update(|window, _| window.within("dialog").find(0usize).bounds());
        if previous == Some(bounds) {
            stable += 1;
        } else {
            stable = 0;
        }
        if stable == 3 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "palette animation did not settle"
        );
        previous = Some(bounds);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let card = cx.update(|window, _| window.within("dialog").find(0usize).bounds());
    let last = cx
        .debug_bounds("command-palette-row-ThemeSystem")
        .expect("last command is laid out");
    assert!(
        last.bottom() > card.bottom(),
        "fixture must contain more commands than fit"
    );
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: gpui_kit::point(card.center().x, card.bottom() - px(40.)),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-1200.))),
        modifiers: Default::default(),
        touch_phase: gpui_kit::TouchPhase::Moved,
    });
    draw(cx);
    let last = cx.debug_bounds("command-palette-row-ThemeSystem").unwrap();
    assert!(
        last.top() >= card.top() && last.bottom() <= card.bottom(),
        "wheel must reveal the last command: {last:?} outside {card:?}"
    );
    // Narrowing a scrolled list must bring its sole match back into view;
    // clearing the filter must restore a reachable first row.
    cx.simulate_input(crate::i18n::t("主题：跟随系统", "Theme: follow system"));
    draw(cx);
    let filtered = cx.debug_bounds("command-palette-row-ThemeSystem").unwrap();
    assert!(
        filtered.top() >= card.top()
            && filtered.top() < last.top()
            && filtered.bottom() <= card.bottom()
    );
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a backspace"
    } else {
        "ctrl-a backspace"
    });
    draw(cx);
    let first = cx.debug_bounds("command-palette-row-QuickConnect").unwrap();
    assert!(first.top() >= card.top() && first.bottom() <= card.bottom());
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: gpui_kit::point(card.center().x, card.bottom() - px(40.)),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-1200.))),
        modifiers: Default::default(),
        touch_phase: gpui_kit::TouchPhase::Moved,
    });
    draw(cx);
    let last = cx.debug_bounds("command-palette-row-ThemeSystem").unwrap();
    assert!(last.top() >= card.top() && last.bottom() <= card.bottom());
    cx.simulate_click(last.center(), gpui_kit::Modifiers::default());
    draw(cx);
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.overlay, Overlay::None);
        assert_eq!(shell.state.store.borrow().theme_pref(), "system");
    });
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
}
