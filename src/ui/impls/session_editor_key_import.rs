//! Explicit private-key import actions. No background clipboard reads and no
//! editable plaintext preview or undo history for imported key material.

use super::*;
use crate::ssh::key_import::{ImportError, ImportedKey};
use zeroize::Zeroizing;

#[derive(Default)]
pub(super) struct KeyImportState {
    generation: u64,
    task: Option<gpui_kit::Task<()>>,
    message: Option<&'static str>,
}

impl KeyImportState {
    pub(super) fn busy(&self) -> bool {
        self.task.is_some()
    }

    pub(super) fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.task = None;
        self.message = None;
    }
}

impl SessionEditor {
    fn accept_key_import(&mut self, result: Result<ImportedKey, ImportError>) {
        self.key_import.task = None;
        match result {
            Ok(mut key) => {
                let mut draft = self.draft.borrow_mut();
                draft.private_key_inline.zeroize();
                draft.private_key_inline = std::mem::take(&mut *key.content);
                draft.private_key_inline_mode = true;
                draft.auth = "key".into();
                self.key_import.message = Some(if key.encrypted {
                    crate::i18n::t("已读取加密私钥容器，尚未验证口令及私钥内容；请填写上方口令，保存后在连接时验证。仅在点击保存后加密保存。", "Encrypted key container loaded; its passphrase and private contents are not yet verified. Enter the passphrase above; verification occurs when connecting after Save. Stored encrypted only after Save.")
                } else {
                    crate::i18n::t(
                        "私钥已导入草稿；点击保存后加密保存。",
                        "Private key imported into the draft; stored encrypted only after Save.",
                    )
                });
            }
            Err(error) => {
                // Never format the selected path, clipboard text, or parser error.
                self.key_import.message = Some(match error {
                    ImportError::File => crate::i18n::t("请选择一个可读取的本地普通私钥文件，不支持目录、设备或符号链接。", "Choose one readable local regular private-key file, not a directory, device or symlink."),
                    ImportError::TooLarge => crate::i18n::t("私钥内容不能超过 1 MiB。", "Private-key content must not exceed 1 MiB."),
                    ImportError::Invalid => crate::i18n::t("无法识别 SSH 私钥，请勿粘贴公钥、证书或文件路径。", "Not a supported SSH private key. Do not paste a public key, certificate, or file path."),
                    ImportError::Parameters => crate::i18n::t("RSA 参数超过安全上限（最大 8192 位），请使用受支持的密钥。", "RSA parameters exceed supported limits (maximum 8192 bits). Use a supported key."),
                    ImportError::UnsupportedAlgorithm => crate::i18n::t("当前 SSH 后端不能使用此签名算法或 OpenSSH 加密算法。支持 RSA、Ed25519 和 NIST P-256/P-384/P-521 ECDSA；不支持 DSA、FIDO 硬件私钥及 TripleDES。", "The SSH backend cannot use this signing algorithm or OpenSSH cipher. Supported keys: RSA, Ed25519 and NIST P-256/P-384/P-521 ECDSA; DSA, FIDO hardware keys and TripleDES are unsupported."),
                    ImportError::UnsupportedEncryption => crate::i18n::t("该加密格式或参数不能安全导入：支持 OpenSSH（bcrypt ≤128 轮）、PPK、受限 PBKDF2/scrypt PKCS#8；旧式加密 PEM 请转换格式或使用原私钥路径。", "Cannot safely import this encryption format or work factor. Supported: OpenSSH (bcrypt ≤128 rounds), PPK, bounded PBKDF2/scrypt PKCS#8. Convert legacy encrypted PEM or use the existing key path."),
                });
            }
        }
    }

    fn start_key_import(
        &mut self,
        operation: impl FnOnce() -> Result<ImportedKey, ImportError> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.key_import.cancel();
        let generation = self.key_import.generation;
        let auth = self.draft.borrow().auth.clone();
        self.key_import.message = Some(crate::i18n::t(
            "正在读取并检查私钥…",
            "Reading and checking private key…",
        ));
        self.key_import.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _permit = crate::ssh::key_import::work_permit().await?;
                    operation()
                })
                .await;
            let _ = this.update(cx, |editor, cx| {
                editor.finish_key_import(generation, &auth, result, cx);
            });
        }));
        cx.notify();
    }

    fn finish_key_import(
        &mut self,
        generation: u64,
        original_auth: &str,
        result: Result<ImportedKey, ImportError>,
        cx: &mut Context<Self>,
    ) {
        if self.key_import.generation != generation {
            return;
        }
        let still_current = {
            let draft = self.draft.borrow();
            self.outcome.is_none() && draft.kind == "ssh" && draft.auth == original_auth
        };
        if still_current {
            self.accept_key_import(result);
        } else {
            // Retire this generation without replacing a newer choice. The
            // unaccepted result drops its Zeroizing content here as well.
            self.key_import.cancel();
        }
        cx.notify();
    }

    fn use_key_path(&mut self, cx: &mut Context<Self>) {
        self.key_import.cancel();
        let mut draft = self.draft.borrow_mut();
        draft.private_key_inline.zeroize();
        draft.private_key_inline_mode = false;
        cx.notify();
    }

    pub(super) fn key_import_field(&self, cx: &Context<Self>) -> SettingField<SharedString> {
        let editor = cx.entity().downgrade();
        let busy = self.key_import.busy();
        let inline = self.draft.borrow().private_key_inline_mode;
        let message = self.key_import.message;
        SettingField::element(
            move |options: &gpui_kit::component::setting::RenderOptions,
                  _: &mut Window,
                  cx: &mut gpui_kit::App| {
                let clipboard_editor = editor.clone();
                let path_editor = editor.clone();
                let drop_editor = editor.clone();
                let field = v_flex()
                    .id("private-key-import")
                    .debug_selector(|| "private-key-import".into())
                    // A horizontal custom SettingField is measured inside an
                    // intrinsically sized wrapper. Match ordinary text fields;
                    // a percentage alone would collapse to the child buttons.
                    .map(|field| {
                        if options.layout().is_horizontal() {
                            field.w_64()
                        } else {
                            field.w_full()
                        }
                    })
                    .gap_2()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .on_drop(move |paths: &gpui_kit::ExternalPaths, _, cx| {
                        if let Some(editor) = drop_editor.upgrade() {
                            editor.update(cx, |editor, cx| {
                                if let [path] = paths.paths() {
                                    let path = path.clone();
                                    editor.start_key_import(
                                        move || crate::ssh::key_import::read_key_file(&path),
                                        cx,
                                    );
                                } else {
                                    editor.key_import.cancel();
                                    editor.key_import.message = Some(crate::i18n::t(
                                        "请每次拖入一个私钥文件。",
                                        "Drop one private-key file at a time.",
                                    ));
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .child(div().text_sm().child(if inline {
                        crate::i18n::t(
                            "当前使用内联私钥，可拖入文件或粘贴内容替换。",
                            "Using inline private-key content. Drop a file or paste to replace it.",
                        )
                    } else {
                        crate::i18n::t(
                            "将 SSH 私钥文件拖到这里，或粘贴私钥内容。",
                            "Drop an SSH private-key file here, or paste its contents.",
                        )
                    }))
                    .child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                editor_action_button(
                                    "private-key-paste",
                                    !busy,
                                    move |_, _, cx| {
                                        // Read once, and only in this explicit user action. The
                                        // platform's clipboard remains owned by the user.
                                        let text =
                                            cx.read_from_clipboard().and_then(|item| item.text());
                                        if let Some(editor) = clipboard_editor.upgrade() {
                                            editor.update(cx, |editor, cx| {
                                                let text = Zeroizing::new(text.unwrap_or_default());
                                                editor.start_key_import(
                                                    move || {
                                                        crate::ssh::key_import::import_key(text)
                                                    },
                                                    cx,
                                                );
                                            });
                                        }
                                    },
                                )
                                .label(crate::i18n::t(
                                    "粘贴私钥内容",
                                    "Paste private-key contents",
                                )),
                            )
                            .child(
                                editor_action_button(
                                    "private-key-use-path",
                                    inline || busy,
                                    move |_, _, cx| {
                                        if let Some(editor) = path_editor.upgrade() {
                                            editor.update(cx, |editor, cx| editor.use_key_path(cx));
                                        }
                                    },
                                )
                                .label(crate::i18n::t(
                                    "改用路径 / 丢弃导入",
                                    "Use path / discard import",
                                )),
                            ),
                    )
                    .when_some(message, |field, message| {
                        field.child(div().text_sm().child(message))
                    });
                // Observe the actual Settings mode without adding a wrapper
                // which would change the intrinsic-sizing behavior under test.
                #[cfg(test)]
                let field = field.debug_selector(move || {
                    tests::import_layout_selector(options.layout().is_horizontal()).into()
                });
                field.into_any_element()
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::button_tests as buttons;
    use super::*;
    use gpui_kit::gpui::{TestAppContext, VisualTestContext};
    use gpui_kit::Axis;

    // Serial exact-test execution is required: two immediate permits hold the
    // actual production admission pool without blocking a GPUI executor thread.
    #[gpui_kit::gpui::test]
    fn busy_save_enter_preserves_dialog_draft_and_pending_import(cx: &mut TestAppContext) {
        use futures::FutureExt as _;
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        let first = crate::ssh::key_import::work_permit()
            .now_or_never().expect("run this regression serially").unwrap();
        let second = crate::ssh::key_import::work_permit()
            .now_or_never().expect("run this regression serially").unwrap();
        let fixture = super::super::tests::StoreFixture::new();
        let cx = buttons::editor_window(cx);
        let editor = buttons::open_editor(cx, &fixture);
        let synthetic = include_str!("../../../tests/fixtures/ppk-v3-ed25519.ppk");
        editor.update(cx, |editor, _| {
            editor.accept_key_import(crate::ssh::key_import::import_key(
                Zeroizing::new(synthetic.into())));
        });
        buttons::focus_action(cx, "editor-save");
        let ran = Arc::new(AtomicBool::new(false));
        let operation_ran = ran.clone();
        editor.update(cx, |editor, cx| {
            editor.start_key_import(move || {
                operation_ran.store(true, Ordering::SeqCst);
                crate::ssh::key_import::import_key(Zeroizing::new(synthetic.into()))
            }, cx);
        });
        buttons::draw(cx);
        assert!(editor.read_with(cx, |editor, _| editor.key_import.busy()));
        assert!(!ran.load(Ordering::SeqCst));
        buttons::key_down(cx, "enter");
        buttons::key_up(cx, "enter");
        buttons::assert_open_unsaved(cx, &editor, &fixture);
        editor.read_with(cx, |editor, _| {
            assert!(editor.key_import.busy());
            assert!(editor.draft.borrow().private_key_inline == synthetic, "synthetic draft key changed");
            assert!(editor.draft.borrow().private_key_inline_mode);
        });
        assert!(!ran.load(Ordering::SeqCst));
        // Cancel before releasing admission, so no queued operation outlives
        // the synthetic editor. Permits are RAII-cleaned if an assertion fails.
        editor.update(cx, |editor, _| editor.key_import.cancel());
        drop((first, second));
        buttons::close_editor(cx, &editor);
    }

    #[gpui_kit::gpui::test]
    fn ordinary_save_click_persists_synthetic_import(cx: &mut TestAppContext) {
        let fixture = super::super::tests::StoreFixture::new();
        let cx = buttons::editor_window(cx);
        let editor = buttons::open_editor(cx, &fixture);
        let synthetic = include_str!("../../../tests/fixtures/ppk-v3-ed25519.ppk");
        editor.update(cx, |editor, cx| {
            editor.accept_key_import(crate::ssh::key_import::import_key(
                Zeroizing::new(synthetic.into())));
            cx.notify();
        });
        buttons::click_action(cx, "editor-save");
        assert_eq!(editor.read_with(cx, |editor, _| editor.outcome), Some(EditorOutcome::Saved));
        let sessions = fixture.disk_sessions();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].private_key_inline.is_local_ciphertext());
        buttons::close_editor(cx, &editor);
    }

    #[gpui_kit::gpui::test]
    fn ordinary_cancel_click_discards_synthetic_import_without_writes(cx: &mut TestAppContext) {
        let fixture = super::super::tests::StoreFixture::new();
        let cx = buttons::editor_window(cx);
        let editor = buttons::open_editor(cx, &fixture);
        editor.update(cx, |editor, cx| {
            editor.accept_key_import(crate::ssh::key_import::import_key(Zeroizing::new(
                include_str!("../../../tests/fixtures/ppk-v3-ed25519.ppk").into())));
            cx.notify();
        });
        buttons::click_action(cx, "editor-cancel");
        assert_eq!(editor.read_with(cx, |editor, _| editor.outcome), Some(EditorOutcome::Cancelled));
        assert!(editor.read_with(cx, |editor, _| editor.draft.borrow().private_key_inline.is_empty()));
        assert!(!fixture.store.borrow().path.exists());
        let store = fixture.store.borrow();
        let saved = store.saved_state.lock().unwrap();
        assert_eq!(saved.submitted, 0);
        assert_eq!(saved.attempted, 0);
        assert!(saved.sessions.is_empty());
        drop(saved);
        drop(store);
        buttons::close_editor(cx, &editor);
    }

    #[gpui_kit::gpui::test]
    fn import_buttons_enter_activate_once_without_dismissing_or_saving(cx: &mut TestAppContext) {
        let fixture = super::super::tests::StoreFixture::new();
        // The process-wide import admission pool is shared with Tokio tests.
        // Releasing their permit may wake this scheduler from another thread.
        cx.background_executor.allow_parking();
        let cx = buttons::editor_window(cx);
        let editor = buttons::open_editor(cx, &fixture);
        buttons::show_import_controls(cx);
        cx.update(|_, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                include_str!("../../../tests/fixtures/ppk-v3-ed25519.ppk").into(),
            ));
        });
        for (id, inline_after) in [("private-key-paste", true), ("private-key-use-path", false)] {
            buttons::focus_action(cx, id);
            let generation = editor.read_with(cx, |editor, _| editor.key_import.generation);
            buttons::key_down(cx, "enter");
            assert_eq!(
                editor.read_with(cx, |editor, _| editor.key_import.generation),
                generation.wrapping_add(1)
            );
            // A drained GPUI queue does not imply that a contended process-wide
            // permit has arrived. Wait on import state, not a guessed duration.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while editor.read_with(cx, |editor, _| editor.key_import.busy()) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "synthetic import did not finish"
                );
                buttons::draw(cx);
                if editor.read_with(cx, |editor, _| editor.key_import.busy()) {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            editor.read_with(cx, |editor, _| {
                assert_eq!(editor.key_import.generation, generation.wrapping_add(1));
                assert_eq!(editor.draft.borrow().private_key_inline_mode, inline_after);
                assert_eq!(
                    editor.draft.borrow().private_key_inline.is_empty(),
                    !inline_after
                );
            });
            buttons::assert_open_unsaved(cx, &editor, &fixture);
            // The same physical press must not start a second import on release.
            buttons::key_up(cx, "enter");
            assert_eq!(
                editor.read_with(cx, |editor, _| editor.key_import.generation),
                generation.wrapping_add(1)
            );
            buttons::assert_open_unsaved(cx, &editor, &fixture);
        }
    }

    #[gpui_kit::gpui::test]
    fn disabled_import_buttons_consume_enter_without_activating(cx: &mut TestAppContext) {
        let fixture = super::super::tests::StoreFixture::new();
        let cx = buttons::editor_window(cx);
        let editor = buttons::open_editor(cx, &fixture);
        buttons::show_import_controls(cx);
        // Retain the real focus when an enabled control becomes disabled.
        editor.update(cx, |editor, cx| {
            editor.draft.borrow_mut().private_key_inline_mode = true;
            cx.notify();
        });
        buttons::focus_action(cx, "private-key-use-path");
        editor.update(cx, |editor, cx| {
            editor.draft.borrow_mut().private_key_inline_mode = false;
            cx.notify();
        });
        let generation = editor.read_with(cx, |editor, _| editor.key_import.generation);
        buttons::key_down(cx, "enter");
        buttons::key_up(cx, "enter");
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.key_import.generation),
            generation
        );
        buttons::assert_open_unsaved(cx, &editor, &fixture);

        buttons::focus_action(cx, "private-key-paste");
        editor.update(cx, |editor, cx| {
            editor.key_import.task = Some(cx.spawn(async move |_, _| {
                std::future::pending::<()>().await;
            }));
            cx.notify();
        });
        buttons::key_down(cx, "enter");
        buttons::key_up(cx, "enter");
        editor.read_with(cx, |editor, _| {
            assert_eq!(editor.key_import.generation, generation);
            assert!(editor.key_import.busy());
            assert!(editor.draft.borrow().private_key_inline.is_empty());
        });
        buttons::assert_open_unsaved(cx, &editor, &fixture);
        editor.update(cx, |editor, _| editor.key_import.cancel());
    }

    #[gpui_kit::gpui::test]
    fn key_import_stays_in_draft_until_save_and_uses_encrypted_store(cx: &mut TestAppContext) {
        let fixture = super::super::tests::StoreFixture::new();
        let ppk = include_str!("../../../tests/fixtures/ppk-v3-ed25519.ppk");
        cx.update(|cx| {
            let editor =
                cx.new(|_| SessionEditor::new_session(fixture.store.clone(), String::new()));
            editor.update(cx, |editor, cx| {
                editor.draft.borrow_mut().host = "synthetic.invalid".into();
                editor.accept_key_import(crate::ssh::key_import::import_key(Zeroizing::new(
                    ppk.into(),
                )));
                assert!(editor.draft.borrow().private_key_inline_mode);
                assert!(!fixture.store.borrow().path.exists());
                editor.save(cx);
                assert_eq!(editor.outcome, Some(EditorOutcome::Saved));
            });
            assert!(fixture.disk_sessions()[0]
                .private_key_inline
                .is_local_ciphertext());
            assert!(!std::fs::read(&fixture.store.borrow().path)
                .unwrap()
                .windows(ppk.len())
                .any(|part| part == ppk.as_bytes()));
        });
    }

    #[gpui_kit::gpui::test]
    fn discard_or_invalid_import_does_not_replace_the_saved_key(cx: &mut TestAppContext) {
        let fixture = super::super::tests::StoreFixture::new();
        cx.update(|cx| {
            let editor =
                cx.new(|_| SessionEditor::new_session(fixture.store.clone(), String::new()));
            editor.update(cx, |editor, cx| {
                let key = include_str!("../../../tests/fixtures/ppk-v3-ed25519.ppk");
                editor.accept_key_import(crate::ssh::key_import::import_key(Zeroizing::new(
                    key.into(),
                )));
                editor.accept_key_import(Err(ImportError::Invalid));
                assert_eq!(editor.draft.borrow().private_key_inline, key);
                editor.use_key_path(cx);
                assert!(editor.draft.borrow().private_key_inline.is_empty());
                assert!(!editor.draft.borrow().private_key_inline_mode);
                assert!(!fixture.store.borrow().path.exists());
            });
        });
    }
    #[gpui_kit::gpui::test]
    fn late_import_cannot_replace_a_new_auth_choice_or_cancelled_generation(
        cx: &mut TestAppContext,
    ) {
        let fixture = super::super::tests::StoreFixture::new();
        cx.update(|cx| {
            let entity =
                cx.new(|_| SessionEditor::new_session(fixture.store.clone(), String::new()));
            entity.update(cx, |editor, cx| {
                let ppk = include_str!("../../../tests/fixtures/ppk-v3-ed25519.ppk");
                let imported = || crate::ssh::key_import::import_key(Zeroizing::new(ppk.into()));
                let generation = editor.key_import.generation;
                editor.draft.borrow_mut().auth = "password".into();
                editor.finish_key_import(generation, "key", imported(), cx);
                assert_eq!(editor.draft.borrow().auth, "password");
                assert!(editor.draft.borrow().private_key_inline.is_empty());
                assert!(!editor.key_import.busy());
                editor.draft.borrow_mut().auth = "key".into();
                editor.key_import.cancel();
                editor.finish_key_import(generation, "key", imported(), cx);
                assert!(editor.draft.borrow().private_key_inline.is_empty());
                assert!(!fixture.store.borrow().path.exists());
            });
        });
    }

    pub(super) fn import_layout_selector(horizontal: bool) -> &'static str {
        if horizontal {
            "private-key-import-layout-horizontal"
        } else {
            "private-key-import-layout-vertical"
        }
    }

    struct ImportLayoutHarness {
        editor: Entity<SessionEditor>,
        layout: Axis,
    }

    impl Render for ImportLayoutHarness {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let field = self
                .editor
                .update(cx, |editor, cx| editor.key_import_field(cx));
            div().size_full().child(
                Settings::new("synthetic-import-layout").page(
                    SettingPage::new("Synthetic import").group(
                        SettingGroup::new()
                            .item(SettingItem::new("SSH private key", field).layout(self.layout)),
                    ),
                ),
            )
        }
    }

    fn settle_import_layout(cx: &mut VisualTestContext, horizontal: bool) {
        let mut previous = None;
        let mut stable = 0;
        // No dialog animation is present in this focused field harness. Drive
        // real Settings/container-query frames and observe its actual mode.
        for _ in 0..12 {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.refresh();
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            cx.run_until_parked();
            let bounds = cx.debug_bounds(import_layout_selector(horizontal));
            if bounds.is_some()
                && cx
                    .debug_bounds(import_layout_selector(!horizontal))
                    .is_none()
                && bounds == previous
            {
                stable += 1;
                if stable == 3 {
                    return;
                }
            } else {
                stable = 0;
            }
            previous = bounds;
        }
        panic!("private-key import did not settle in the actual requested Settings mode");
    }

    #[gpui_kit::gpui::test]
    fn import_control_uses_actual_setting_layout_and_keeps_actions_inside(cx: &mut TestAppContext) {
        let fixture = super::super::tests::StoreFixture::new();
        cx.update(|cx| {
            gpui_kit::init(cx);
            // w_64 uses rems; pin the same baseline as toolkit Settings tests.
            gpui_kit::component::Theme::global_mut(cx).font_size = px(16.);
        });
        let mut harness = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let editor =
                cx.new(|_| SessionEditor::new_session(fixture.store.clone(), String::new()));
            editor.update(cx, |editor, _| {
                // Include a real long rejection message, without any key data.
                editor.accept_key_import(Err(ImportError::UnsupportedAlgorithm));
            });
            let view = cx.new(|_| ImportLayoutHarness {
                editor,
                layout: Axis::Horizontal,
            });
            harness = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let harness = harness.unwrap();
        cx.simulate_resize(gpui_kit::size(px(1200.), px(900.)));
        let mut horizontal_width = None;
        for layout in [Axis::Horizontal, Axis::Vertical, Axis::Horizontal] {
            harness.update(cx, |harness, cx| {
                harness.layout = layout;
                cx.notify();
            });
            let horizontal = layout.is_horizontal();
            settle_import_layout(cx, horizontal);
            let field = cx.debug_bounds(import_layout_selector(horizontal)).unwrap();
            assert!(cx
                .debug_bounds(import_layout_selector(!horizontal))
                .is_none());
            if horizontal {
                assert!(
                    field.size.width >= px(240.) && field.size.width <= px(256.),
                    "horizontal import must use the normal field width: {field:?}"
                );
                if let Some(width) = horizontal_width {
                    assert_eq!(field.size.width, width);
                }
                horizontal_width = Some(field.size.width);
            } else {
                assert!(
                    field.size.width > horizontal_width.unwrap() + px(40.),
                    "vertical import must fill the wider content column: {field:?}"
                );
            }
            let paste = cx.debug_bounds("private-key-paste").unwrap();
            let discard = cx.debug_bounds("private-key-use-path").unwrap();
            for action in [paste, discard] {
                assert!(
                    action.left() >= field.left()
                        && action.right() <= field.right()
                        && action.top() >= field.top()
                        && action.bottom() <= field.bottom(),
                    "import action must stay inside the field: {action:?} in {field:?}"
                );
            }
            assert!(
                paste.right() <= discard.left()
                    || discard.right() <= paste.left()
                    || paste.bottom() <= discard.top()
                    || discard.bottom() <= paste.top(),
                "wrapped import actions must not overlap"
            );
        }
        assert!(!fixture.store.borrow().path.exists());
    }
}
