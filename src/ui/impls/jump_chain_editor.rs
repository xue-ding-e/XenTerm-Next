//! Ordered bastion routes. The view uses stable session IDs and never copies
//! credentials; each hop continues using its saved authentication settings.
use super::session_editor::SessionEditor;
use crate::{
    config::{ConfigStore, SessionKind},
    core::SessionDraft,
};
use gpui_kit::component::Disableable;
use gpui_kit::{
    assets::IconName,
    component::{
        button::{Button, ButtonVariants, DropdownButton},
        h_flex,
        menu::PopupMenuItem,
        setting::{SettingField, SettingGroup, SettingItem, SettingPage},
        v_flex, ActiveTheme, Sizable as _,
    },
    div,
    prelude::*,
    AnyElement, App, IntoElement, SharedString, WeakEntity,
};
use std::{cell::RefCell, rc::Rc};

fn route(draft: &SessionDraft, store: &ConfigStore) -> anyhow::Result<Vec<String>> {
    store
        .resolve_jump_chain(&draft.to_session(None))
        .map(|hops| hops.into_iter().rev().map(|hop| hop.id).collect())
}

/// Every explicit edit replaces only the target's route. Other sessions and
/// their nested routes remain untouched. Empty deliberately means direct.
fn set_route(draft: &mut SessionDraft, ids: Vec<String>) {
    draft.jump_session_id.clear();
    draft.jump_session_ids = ids;
}

fn edit_route(
    draft: &Rc<RefCell<SessionDraft>>,
    store: &Rc<RefCell<ConfigStore>>,
    editor: &WeakEntity<SessionEditor>,
    cx: &mut App,
    edit: impl FnOnce(&mut Vec<String>),
) {
    let resolved = route(&draft.borrow(), &store.borrow());
    if let Ok(mut ids) = resolved {
        edit(&mut ids);
        set_route(&mut draft.borrow_mut(), ids);
    }
    if let Some(editor) = editor.upgrade() {
        editor.update(cx, |_, cx| cx.notify());
    }
}

pub(super) fn page(
    draft: Rc<RefCell<SessionDraft>>,
    store: Rc<RefCell<ConfigStore>>,
    editor: WeakEntity<SessionEditor>,
) -> SettingPage {
    let field = SettingField::element(
        move |_: &gpui_kit::component::setting::RenderOptions,
              _: &mut gpui_kit::Window,
              cx: &mut App| {
            let theme = cx.theme();
            let current = route(&draft.borrow(), &store.borrow());
            let valid = current.is_ok();
            let ids = current.as_ref().cloned().unwrap_or_default();
            let target_id = draft.borrow().id.clone();
            let candidates: Vec<(String, String)> = store
                .borrow()
                .sessions()
                .iter()
                .filter(|s| s.kind == SessionKind::Ssh && s.id != target_id)
                .map(|s| {
                    (
                        s.id.clone(),
                        format!(
                            "{} · {}@{}:{} · {}",
                            s.name,
                            s.user,
                            s.host,
                            s.port,
                            s.auth.as_str()
                        ),
                    )
                })
                .collect();
            let mut rows: Vec<AnyElement> = Vec::new();
            for (index, id) in ids.iter().enumerate() {
                let label = candidates
                    .iter()
                    .find(|(candidate, _)| candidate == id)
                    .map(|(_, label)| label.clone())
                    .unwrap_or_else(|| id.clone());
                let choices = candidates.clone();
                let selected = ids.clone();
                let d = draft.clone();
                let s = store.clone();
                let e = editor.clone();
                let choose =
                    DropdownButton::new(SharedString::from(format!("jump-select-{index}")))
                        .button(
                            Button::new(SharedString::from(format!("jump-select-trigger-{index}")))
                                .label(label)
                                .outline(),
                        )
                        .dropdown_menu(move |menu, _, _| {
                            let mut menu = menu;
                            for (id, label) in &choices {
                                if selected
                                    .iter()
                                    .enumerate()
                                    .any(|(i, chosen)| i != index && chosen == id)
                                {
                                    continue;
                                }
                                let id = id.clone();
                                let d = d.clone();
                                let s = s.clone();
                                let e = e.clone();
                                menu = menu.item(PopupMenuItem::new(label.clone()).on_click(
                                    move |_, _, cx| {
                                        edit_route(&d, &s, &e, cx, |ids| {
                                            if let Some(slot) = ids.get_mut(index) {
                                                *slot = id.clone();
                                            }
                                        });
                                    },
                                ));
                            }
                            menu
                        });
                let mut row = h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child(format!("{}", index + 1)))
                    .child(div().flex_1().min_w_0().child(choose));
                for (suffix, icon, label, offset, disabled) in [
                    (
                        "up",
                        IconName::ArrowUp,
                        crate::i18n::t("向前移动", "Move earlier"),
                        -1_i32,
                        index == 0,
                    ),
                    (
                        "down",
                        IconName::ArrowDown,
                        crate::i18n::t("向后移动", "Move later"),
                        1,
                        index + 1 == ids.len(),
                    ),
                ] {
                    let d = draft.clone();
                    let s = store.clone();
                    let e = editor.clone();
                    row = row.child(
                        Button::new(SharedString::from(format!("jump-{suffix}-{index}")))
                            .debug_selector(move || format!("jump-{suffix}-{index}"))
                            .icon(icon)
                            .ghost()
                            .small()
                            .disabled(disabled)
                            .tooltip(label)
                            .accessibility_label(label)
                            .on_click(move |_, _, cx| {
                                edit_route(&d, &s, &e, cx, |ids| {
                                    let next = index as i32 + offset;
                                    if next >= 0 && (next as usize) < ids.len() && index < ids.len()
                                    {
                                        ids.swap(index, next as usize);
                                    }
                                })
                            }),
                    );
                }
                let d = draft.clone();
                let s = store.clone();
                let e = editor.clone();
                row = row.child(
                    Button::new(SharedString::from(format!("jump-remove-{index}")))
                        .debug_selector(move || format!("jump-remove-{index}"))
                        .icon(IconName::Trash)
                        .ghost()
                        .small()
                        .tooltip(crate::i18n::t("移除此跳板", "Remove this hop"))
                        .accessibility_label(crate::i18n::t("移除此跳板", "Remove this hop"))
                        .on_click(move |_, _, cx| {
                            edit_route(&d, &s, &e, cx, |ids| {
                                if index < ids.len() {
                                    ids.remove(index);
                                }
                            })
                        }),
                );
                rows.push(row.into_any_element());
            }
            let choices: Vec<_> = candidates
                .into_iter()
                .filter(|(id, _)| !ids.contains(id))
                .collect();
            let d = draft.clone();
            let s = store.clone();
            let e = editor.clone();
            let add = DropdownButton::new("jump-add")
                .button(
                    Button::new("jump-add-trigger")
                        .icon(IconName::Plus)
                        .label(crate::i18n::t("添加跳板", "Add bastion"))
                        .outline()
                        .disabled(!valid || ids.len() >= 16 || choices.is_empty()),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (id, label) in &choices {
                        let id = id.clone();
                        let d = d.clone();
                        let s = s.clone();
                        let e = e.clone();
                        menu = menu.item(PopupMenuItem::new(label.clone()).on_click(
                            move |_, _, cx| {
                                edit_route(&d, &s, &e, cx, |ids| {
                                    if ids.len() < 16 && !ids.contains(&id) {
                                        ids.push(id.clone());
                                    }
                                });
                            },
                        ));
                    }
                    menu
                });
            let d = draft.clone();
            let e = editor.clone();
            let clear = Button::new("jump-clear")
                .label(crate::i18n::t(
                    "清空并直连",
                    "Clear route / connect directly",
                ))
                .ghost()
                .disabled(valid && ids.is_empty())
                .on_click(move |_, _, cx| {
                    set_route(&mut d.borrow_mut(), Vec::new());
                    if let Some(e) = e.upgrade() {
                        e.update(cx, |_, cx| cx.notify());
                    }
                });
            let path = if ids.is_empty() {
                crate::i18n::t("本机 → 目标", "This computer → target").to_owned()
            } else {
                format!(
                    "{} → {} → {}",
                    crate::i18n::t("本机", "This computer"),
                    (1..=ids.len())
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(" → "),
                    crate::i18n::t("目标", "target")
                )
            };
            v_flex().w_full().min_w_0().gap_3()
            .child(div().text_sm().child(path))
            .child(div().text_xs().text_color(theme.muted_foreground).child(crate::i18n::t(
                "按连接顺序排列：最外层在前，目标前的跳板在后。每级使用自己的密码或私钥；上下按钮可任意调整顺序。",
                "Connection order: outermost first, target's nearest bastion last. Each hop uses its own password or key. Move rows up or down to reorder.")))
            .when_some(current.err(), |list, error| list.child(div().text_sm().child(format!("{}: {error}", crate::i18n::t("链路无效，请清空重建或修复引用的会话", "Invalid route: repair the referenced session or clear and rebuild")))))
            .children(rows)
            .child(h_flex().gap_2().child(add).child(clear))
            .into_any_element()
        },
    );
    SettingPage::new(crate::i18n::t("多级跳板", "SSH bastions")).group(SettingGroup::new().item(
        SettingItem::new(crate::i18n::t("连接链路", "Connection route"), field),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_route_edit_clears_legacy_link_and_supports_reorder_remove() {
        let mut draft = SessionDraft::new_ssh();
        draft.jump_session_id = "legacy".into();
        set_route(&mut draft, vec!["outer".into(), "inner".into()]);
        assert!(draft.jump_session_id.is_empty());
        let mut ids = draft.jump_session_ids.clone();
        ids.swap(0, 1);
        set_route(&mut draft, ids);
        assert_eq!(draft.jump_session_ids, ["inner", "outer"]);
        set_route(&mut draft, Vec::new());
        assert!(draft.to_session(None).jump_session_id.is_empty());
        assert!(draft.to_session(None).jump_session_ids.is_empty());
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use crate::config::{ConfigFile, SavedState, Session};
    use gpui_kit::{
        component::setting::Settings, gpui::TestAppContext, Context, Entity, Render, Window,
    };

    struct Probe {
        draft: Rc<RefCell<SessionDraft>>,
        store: Rc<RefCell<ConfigStore>>,
        editor: Entity<SessionEditor>,
    }
    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            Settings::new("jump-route-probe").page(page(
                self.draft.clone(),
                self.store.clone(),
                self.editor.downgrade(),
            ))
        }
    }
    fn session(id: &str, legacy: &str) -> Session {
        let mut value = Session::new_empty();
        value.id = id.into();
        value.name = id.into();
        value.host = format!("{id}.example");
        value.jump_session_id = legacy.into();
        value
    }

    #[gpui_kit::gpui::test]
    fn jump_editor_reorders_legacy_chain_and_renders_without_mutating_saved_hops(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let target = session("target", "inner");
        let store = Rc::new(RefCell::new(ConfigStore {
            path: std::env::temp_dir().join(format!("xenterm-jump-ui-{}.db", uuid::Uuid::new_v4())),
            backup_dir: None,
            cache: ConfigFile {
                sessions: vec![
                    session("outer", ""),
                    session("inner", "outer"),
                    target.clone(),
                ],
                ..Default::default()
            },
            key: [0; 32],
            keyring_enabled: false,
            saved_state: std::sync::Mutex::new(SavedState::default()),
        }));
        let draft = Rc::new(RefCell::new(SessionDraft::from_session(&target)));
        let (view, cx) = cx.add_window_view(move |_, cx| {
            let editor = cx.new(|_| SessionEditor::edit(store.clone(), target));
            Probe {
                draft,
                store,
                editor,
            }
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        view.read_with(cx, |probe, _| {
            assert_eq!(
                route(&probe.draft.borrow(), &probe.store.borrow()).unwrap(),
                ["outer", "inner"]
            );
        });
        let down = cx
            .debug_bounds("jump-down-0")
            .expect("visible reorder button");
        cx.simulate_click(down.center(), gpui_kit::Modifiers::default());
        view.update(cx, |probe, cx| {
            assert_eq!(probe.draft.borrow().jump_session_ids, ["inner", "outer"]);
            assert!(probe.draft.borrow().jump_session_id.is_empty());
            assert_eq!(
                probe.store.borrow().get("inner").unwrap().jump_session_id,
                "outer"
            );
            assert_eq!(
                probe.store.borrow().get("target").unwrap().jump_session_id,
                "inner"
            );
            // The real editor is the observed owner; this isolated page probe
            // deliberately owns a separate entity, so refresh it explicitly.
            cx.notify();
        });
        for remaining in [1, 0] {
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
            let remove = cx
                .debug_bounds("jump-remove-0")
                .expect("visible remove button");
            cx.simulate_click(remove.center(), gpui_kit::Modifiers::default());
            view.update(cx, |probe, cx| {
                assert_eq!(
                    route(&probe.draft.borrow(), &probe.store.borrow())
                        .unwrap()
                        .len(),
                    remaining
                );
                cx.notify();
            });
        }
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
}
