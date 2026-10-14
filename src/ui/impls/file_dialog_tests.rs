//! File events mount the real shell dialog; dismissal uses real keys and buttons.
//! No terminal process, transport, credentials, or user files are opened.

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
                persistence_poll: None,
                process_window: Detached::new(process_window_title),
                execution_log_window: crate::ui::ExecutionLogWindowHandle::new(),
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

fn open_file(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    name: &str,
    editable: bool,
) -> Entity<super::super::file_viewer::FileViewerView> {
    shell.update(cx, |shell, cx| {
        *shell.state.opened_file.lock().unwrap() = Some(super::super::session_state::OpenedFile {
            path: format!("/synthetic/{name}"),
            name: name.into(),
            content: "synthetic content\nsecond line".into(),
            editable,
            error: String::new(),
        });
        cx.notify();
    });
    draw(cx);
    let viewer = shell.read_with(cx, |shell, _| {
        shell.open_file.clone().expect("file event opens viewer")
    });
    settle(cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    viewer
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

fn assert_closed(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    draw(cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    shell.read_with(cx, |shell, _| {
        assert!(
            shell.open_file.is_none(),
            "dismissal must release the file entity"
        );
        assert_eq!(
            shell.overlay,
            Overlay::None,
            "hidden file must not block shell actions"
        );
    });
}

fn assert_shortcuts_recover(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    for key in ["ctrl-k", "ctrl-shift-p"] {
        // Focus a real background shell target, as clicking the terminal does.
        cx.update(|window, cx| window.focus_next(cx));
        draw(cx);
        cx.simulate_keystrokes(key);
        draw(cx);
        assert!(
            cx.update(|window, cx| window.has_active_dialog(cx)),
            "{key} must open after dismissal"
        );
        shell.read_with(cx, |shell, _| {
            assert!(matches!(
                (&shell.overlay, key),
                (Overlay::QuickConnect(_), "ctrl-k") | (Overlay::Commands(_), "ctrl-shift-p")
            ))
        });
        cx.simulate_keystrokes("escape");
        assert_closed(shell, cx);
    }
}

#[gpui_kit::gpui::test]
fn escape_releases_file_state_and_allows_reopen_and_shell_shortcuts(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    for (name, editable) in [("readonly.txt", false), ("reopened-edit.txt", true)] {
        let viewer = open_file(&shell, cx, name, editable);
        cx.simulate_keystrokes("escape");
        assert_closed(&shell, cx);
        assert!(
            viewer
                .update(cx, |viewer, _| viewer.take_action())
                .is_none(),
            "Escape must not queue a file write"
        );
        assert_shortcuts_recover(&shell, cx);
    }
}

#[gpui_kit::gpui::test]
fn outer_close_button_releases_file_state_and_shell_shortcuts(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let viewer = open_file(&shell, cx, "outer-close.txt", false);
    let close = cx.update(|window, _| window.within("dialog").find("close").bounds());
    cx.simulate_click(close.center(), gpui_kit::Modifiers::default());
    assert_closed(&shell, cx);
    assert!(viewer
        .update(cx, |viewer, _| viewer.take_action())
        .is_none());
    assert_shortcuts_recover(&shell, cx);
}

#[gpui_kit::gpui::test]
fn inner_close_button_keeps_the_existing_close_contract(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let viewer = open_file(&shell, cx, "inner-close.txt", true);
    let close = cx.update(|window, _| window.find("file-viewer-close").bounds());
    cx.simulate_click(close.center(), gpui_kit::Modifiers::default());
    assert_closed(&shell, cx);
    assert!(viewer
        .update(cx, |viewer, _| viewer.take_action())
        .is_none());
    assert_shortcuts_recover(&shell, cx);
}

#[gpui_kit::gpui::test]
fn delayed_old_file_cleanup_never_clears_a_replacement_or_other_overlay(cx: &mut TestAppContext) {
    let (shell, cx) = fixture(cx);
    let old = open_file(&shell, cx, "old.txt", false);
    let replacement = open_file(&shell, cx, "replacement.txt", true);
    assert_ne!(old.entity_id(), replacement.entity_id());
    // Model the deferred callback arriving after a new file response has
    // replaced the card. The callback's identity check must leave it intact.
    shell.update(cx, |shell, cx| {
        shell.release_file_viewer(old.entity_id(), cx)
    });
    draw(cx);
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.open_file.as_ref().unwrap().entity_id(), replacement.entity_id());
        assert!(matches!(&shell.overlay, Overlay::File(viewer) if viewer.entity_id() == replacement.entity_id()));
    });
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| shell.open_quick_connect(window, cx));
    });
    // A later non-file overlay must survive too. The hidden file entity can
    // be released, but neither its state nor its dialog may be closed here.
    shell.update(cx, |shell, cx| {
        shell.release_file_viewer(replacement.entity_id(), cx)
    });
    draw(cx);
    shell.read_with(cx, |shell, _| {
        assert!(shell.open_file.is_none());
        assert!(matches!(shell.overlay, Overlay::QuickConnect(_)));
    });
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    cx.simulate_keystrokes("escape");
    assert_closed(&shell, cx);
}

// Exercise the real GUI export entry point without invoking a native dialog.
// Both profile and destination are disposable synthetic directories. These
// tests pin the deliberate no-overwrite policy even after a dialog picks a path.
#[cfg(any(unix, windows))]
fn export_fixture(
    cx: &mut TestAppContext,
) -> (
    Entity<Shell>,
    &mut VisualTestContext,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let (shell, cx) = fixture(cx);
    let profile = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    shell.update(cx, |shell, _| {
        shell.state.store.borrow_mut().path = profile.path().join("synthetic-profile.db");
    });
    (shell, cx, profile, destination)
}

#[cfg(any(unix, windows))]
#[gpui_kit::gpui::test]
fn gui_export_existing_destination_is_preserved_and_explains_rename(cx: &mut TestAppContext) {
    let (shell, cx, _profile, destination) = export_fixture(cx);
    let path = destination.path().join("existing.json");
    std::fs::write(&path, b"KEEP ORIGINAL").unwrap();
    shell.update(cx, |shell, cx| shell.export_sessions_to(path.clone(), cx));
    assert_eq!(std::fs::read(&path).unwrap(), b"KEEP ORIGINAL");
    shell.read_with(cx, |shell, _| {
        let message = &shell.status.as_ref().unwrap().text;
        assert!(message.contains(crate::i18n::t("新文件名", "new filename")));
        #[cfg(unix)]
        assert_eq!(message, crate::i18n::t(
            "未导出：目标已存在。为保护已有文件，请换一个新文件名后重试。",
            "Not exported: the destination already exists. Choose a new filename to protect the existing file.",
        ));
    });
}

#[cfg(any(unix, windows))]
#[gpui_kit::gpui::test]
fn gui_export_protected_profile_cannot_fall_back_to_plain_write(cx: &mut TestAppContext) {
    let (shell, cx, profile, _destination) = export_fixture(cx);
    let path = profile.path().join("synthetic-profile.db");
    std::fs::write(&path, b"KEEP PROFILE").unwrap();
    shell.update(cx, |shell, cx| shell.export_sessions_to(path.clone(), cx));
    assert_eq!(std::fs::read(path).unwrap(), b"KEEP PROFILE");
    shell.read_with(cx, |shell, _| {
        assert!(shell
            .status
            .as_ref()
            .unwrap()
            .text
            .starts_with(crate::i18n::t("导出失败", "export failed")));
    });
}

#[cfg(any(unix, windows))]
#[gpui_kit::gpui::test]
fn gui_export_oversize_payload_preserves_existing_file(cx: &mut TestAppContext) {
    let (shell, cx, _profile, destination) = export_fixture(cx);
    let path = destination.path().join("existing.json");
    std::fs::write(&path, b"KEEP ORIGINAL").unwrap();
    shell.update(cx, |shell, cx| {
        let mut session = crate::config::Session::new_empty();
        session.note = "synthetic".repeat(2 * 1024 * 1024);
        shell.state.store.borrow_mut().cache.sessions.push(session);
        shell.export_sessions_to(path.clone(), cx);
    });
    assert_eq!(std::fs::read(path).unwrap(), b"KEEP ORIGINAL");
    shell.read_with(cx, |shell, _| {
        assert!(shell.status.as_ref().unwrap().text.contains("16 MiB"));
    });
}

#[cfg(unix)]
#[gpui_kit::gpui::test]
fn gui_export_symlink_does_not_write_through_to_profile(cx: &mut TestAppContext) {
    let (shell, cx, profile, destination) = export_fixture(cx);
    let target = profile.path().join("synthetic-profile.db");
    std::fs::write(&target, b"KEEP PROFILE").unwrap();
    let link = destination.path().join("alias.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    shell.update(cx, |shell, cx| shell.export_sessions_to(link.clone(), cx));
    assert_eq!(std::fs::read(target).unwrap(), b"KEEP PROFILE");
    assert!(std::fs::symlink_metadata(link)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(any(unix, windows))]
#[gpui_kit::gpui::test]
fn gui_export_new_file_succeeds_through_private_publisher(cx: &mut TestAppContext) {
    let (shell, cx, _profile, destination) = export_fixture(cx);
    let path = destination.path().join("new.json");
    shell.update(cx, |shell, cx| shell.export_sessions_to(path.clone(), cx));
    assert!(path.is_file());
    shell.read_with(cx, |shell, _| {
        assert_eq!(
            shell.status.as_ref().unwrap().text,
            format!("{} 0", crate::i18n::t("已导出连接", "exported"))
        );
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o077,
            0
        );
    }
}
