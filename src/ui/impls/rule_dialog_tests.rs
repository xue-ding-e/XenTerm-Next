//! Real rule-editor buttons and toolkit dialog dismissal, with an isolated store.
use super::*;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use gpui_kit::test::TestWindowExt as _;
use std::sync::{Arc, Mutex};

fn fixture(cx: &mut TestAppContext) -> (Entity<Shell>, tempfile::TempDir, &mut VisualTestContext) {
    let directory = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::ui::actions::init(cx);
    });
    let store = crate::config::ConfigStore {
        path: directory.path().join("synthetic-rules.db"),
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
    (shell.unwrap(), directory, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn settle(cx: &mut VisualTestContext) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut previous = None;
    let mut stable = 0;
    loop {
        draw(cx);
        let card = cx.update(|window, _| window.within("dialog").find(0usize).bounds());
        if previous == Some(card) {
            stable += 1;
        } else {
            stable = 0;
        }
        if stable == 3 {
            break;
        }
        assert!(std::time::Instant::now() < end, "dialog did not settle");
        previous = Some(card);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn open_rule(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            let editor = cx.new(|cx| RuleEditorView::new(shell.state.store.clone(), window, cx));
            shell.overlay = Overlay::RuleEditor(editor.clone());
            Shell::overlay_dialog(editor, "Highlight rule".into(), 620., 480., window, cx);
            cx.notify();
        });
    });
    settle(cx);
    cx.update(|window, cx| window.focus_next(cx));
    draw(cx);
    assert_eq!(
        cx.update(|window, cx| window
            .focused_input(cx)
            .expect("rule pattern input")
            .value(cx)
            .to_string()),
        ""
    );
}

fn type_pattern(text: &str, cx: &mut VisualTestContext) {
    cx.simulate_input(text);
    draw(cx);
    assert_eq!(
        cx.update(|window, cx| window
            .focused_input(cx)
            .expect("pattern remains focused")
            .value(cx)
            .to_string()),
        text
    );
}

fn click(id: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx.update(|window, _| window.within("dialog").find(id).bounds());
    cx.simulate_click(bounds.center(), Default::default());
    for _ in 0..3 {
        draw(cx);
    }
}

fn patterns(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
    shell.read_with(cx, |shell, _| {
        shell
            .state
            .store
            .borrow()
            .output_highlight_rules()
            .iter()
            .map(|rule| rule.pattern.clone())
            .collect()
    })
}

fn assert_closed(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for _ in 0..3 {
        draw(cx);
    }
    assert!(
        !cx.update(|window, cx| window.has_active_dialog(cx)),
        "successful Add must dismiss the actual dialog"
    );
    assert!(shell.read_with(cx, |shell, _| matches!(shell.overlay, Overlay::None)));
}

#[gpui_kit::gpui::test]
fn adding_a_rule_closes_the_dialog_and_cannot_repeat_the_old_save(cx: &mut TestAppContext) {
    let (shell, _directory, cx) = fixture(cx);
    open_rule(&shell, cx);
    type_pattern("QA_RULE_A", cx);
    let save = cx.update(|window, _| window.within("dialog").find("rule-save").bounds());
    click("rule-save", cx);
    assert_eq!(patterns(&shell, cx), vec!["QA_RULE_A"]);
    assert_closed(&shell, cx);
    // A delayed second pointer click cannot reach the retired editor.
    cx.simulate_click(save.center(), Default::default());
    draw(cx);
    assert_eq!(patterns(&shell, cx), vec!["QA_RULE_A"]);
    open_rule(&shell, cx);
    type_pattern("QA_RULE_B", cx);
    click("rule-save", cx);
    assert_closed(&shell, cx);
    assert_eq!(patterns(&shell, cx), vec!["QA_RULE_A", "QA_RULE_B"]);
}

#[gpui_kit::gpui::test]
fn invalid_regex_stays_open_until_corrected_and_saves_only_once(cx: &mut TestAppContext) {
    let (shell, _directory, cx) = fixture(cx);
    open_rule(&shell, cx);
    type_pattern("[", cx);
    click("rule-regex", cx);
    click("rule-save", cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(shell.read_with(cx, |shell, _| matches!(
        shell.overlay,
        Overlay::RuleEditor(_)
    )));
    assert!(patterns(&shell, cx).is_empty());
    // The regex switch takes keyboard focus. Move back to the same visible
    // pattern field before correcting its rejected draft.
    cx.simulate_keystrokes("shift-tab");
    draw(cx);
    assert_eq!(
        cx.update(|window, cx| window
            .focused_input(cx)
            .expect("pattern before regex switch")
            .value(cx)
            .to_string()),
        "["
    );
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    type_pattern("QA_RULE_FIXED", cx);
    click("rule-save", cx);
    assert_eq!(patterns(&shell, cx), vec!["QA_RULE_FIXED"]);
    assert_closed(&shell, cx);
}

#[gpui_kit::gpui::test]
fn dismissing_a_rule_draft_with_escape_or_close_never_adds_it(cx: &mut TestAppContext) {
    let (shell, _directory, cx) = fixture(cx);
    for outer_close in [false, true] {
        open_rule(&shell, cx);
        type_pattern("QA_UNSAVED_RULE", cx);
        if outer_close {
            click("close", cx);
        } else {
            cx.simulate_keystrokes("escape");
        }
        assert_closed(&shell, cx);
        assert!(patterns(&shell, cx).is_empty());
    }
}
