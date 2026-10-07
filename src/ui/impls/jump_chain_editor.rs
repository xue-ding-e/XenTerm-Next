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
        menu::{PopupMenu, PopupMenuItem},
        setting::{SettingField, SettingGroup, SettingItem, SettingPage},
        tooltip::Tooltip,
        v_flex, ActiveTheme, Sizable as _,
    },
    div,
    prelude::*,
    px, AnyElement, AnyView, App, Axis, IntoElement, Pixels, SharedString, WeakEntity, Window,
};
use std::{cell::RefCell, rc::Rc};

fn menu_width(window: &Window) -> Pixels {
    (window.viewport_size().width - px(32.))
        .min(px(560.))
        .max(px(0.))
}

fn bounded_menu(menu: PopupMenu, window: &Window) -> PopupMenu {
    let width = menu_width(window);
    menu.min_w(width)
        .max_w(width)
        .max_h((window.viewport_size().height * 0.5).min(px(320.)))
        // PopupMenu only applies max_h when scrolling is enabled.
        .scrollable(true)
}

/// Keep the complete value available without letting the tooltip itself grow
/// wider than the window. Long unbroken hostnames wrap at character boundaries.
fn full_label_tooltip(label: String, window: &mut Window, cx: &mut App) -> AnyView {
    let width = (window.viewport_size().width - px(64.))
        .min(px(520.))
        .max(px(0.));
    Tooltip::element(move |_, _| div().w(width).whitespace_normal().child(label.clone()))
        .build(window, cx)
}

fn candidate_item(id: &str, label: String, window: &Window) -> PopupMenuItem {
    let width = (menu_width(window) - px(32.)).max(px(0.));
    let selector = format!("jump-menu-label-{id}");
    // A plain PopupMenuItem label is not clipped by the menu's max_w. Bound
    // the text itself; keep the menu's own click and keyboard dispatch intact.
    PopupMenuItem::element(move |_, _| {
        let tooltip = label.clone();
        let selector = selector.clone();
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .w(width)
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .aria_label(label.clone())
            .hoverable_tooltip(move |window, cx| full_label_tooltip(tooltip.clone(), window, cx))
            .child(label.clone())
    })
}

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
                        .w_full()
                        .min_w_0()
                        .button(
                            Button::new(SharedString::from(format!("jump-select-trigger-{index}")))
                                .debug_selector(move || format!("jump-select-trigger-{index}"))
                                .flex_1()
                                .min_w_0()
                                .accessibility_label(label.clone())
                                .child({
                                    let tooltip = label.clone();
                                    div()
                                        .id("label-value")
                                        .w_full()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .hoverable_tooltip(move |window, cx| {
                                            full_label_tooltip(tooltip.clone(), window, cx)
                                        })
                                        .child(label)
                                })
                                .outline(),
                        )
                        .dropdown_menu(move |menu, window, _| {
                            let mut menu = bounded_menu(menu, window);
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
                                menu =
                                    menu.item(candidate_item(&id, label.clone(), window).on_click(
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
                    .debug_selector(move || format!("jump-row-{index}"))
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .child(format!("{}", index + 1)),
                    )
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
                            .flex_shrink_0()
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
                        .flex_shrink_0()
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
                .dropdown_menu(move |menu, window, _| {
                    let mut menu = bounded_menu(menu, window);
                    for (id, label) in &choices {
                        let id = id.clone();
                        let d = d.clone();
                        let s = s.clone();
                        let e = e.clone();
                        menu = menu.item(candidate_item(&id, label.clone(), window).on_click(
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
            .child(h_flex().flex_wrap().gap_2().child(div().debug_selector(|| "jump-add-control".into()).child(add)).child(clear))
            .into_any_element()
        },
    );
    SettingPage::new(crate::i18n::t("多级跳板", "SSH bastions")).group(
        SettingGroup::new().item(
            // A route is a full-width table. The horizontal settings layout leaves
            // its field intrinsically sized, so a long label can push controls out.
            SettingItem::new(crate::i18n::t("连接链路", "Connection route"), field)
                .layout(Axis::Vertical),
        ),
    )
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
        component::{setting::Settings, Root},
        gpui::{TestAppContext, VisualTestContext},
        point, px,
        test::TestWindowExt as _,
        Context, Entity, Render, Window,
    };

    struct Probe {
        draft: Rc<RefCell<SessionDraft>>,
        store: Rc<RefCell<ConfigStore>>,
        editor: Entity<SessionEditor>,
        width: Option<f32>,
    }
    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .debug_selector(|| "jump-editor-bounds".into())
                .size_full()
                .when_some(self.width, |this, width| this.w(px(width)).h(px(580.)))
                .child(Settings::new("jump-route-probe").page(page(
                    self.draft.clone(),
                    self.store.clone(),
                    self.editor.downgrade(),
                )))
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

    fn long_route_fixture(
        cx: &mut TestAppContext,
        count: usize,
        width: f32,
    ) -> (Entity<Probe>, &mut VisualTestContext) {
        let mut sessions: Vec<_> = (0..16)
            .map(|index| {
                let mut hop = session(&format!("hop-{index:02}"), "");
                hop.name = format!(
                    "跳板{index:02}｜合成测试-外层入口-Bastion-Gateway-{}",
                    "LongName".repeat(10)
                );
                hop.host = format!(
                    "hop-{index:02}.{}.documentation-only.invalid",
                    "long-region-name.".repeat(8)
                );
                hop.user = "qa_fixture".into();
                hop
            })
            .collect();
        let mut target = session("target", "");
        target.jump_session_ids = sessions
            .iter()
            .take(count)
            .map(|hop| hop.id.clone())
            .collect();
        sessions.push(target.clone());
        let draft = Rc::new(RefCell::new(SessionDraft::from_session(&target)));
        let store = Rc::new(RefCell::new(ConfigStore {
            path: std::env::temp_dir()
                .join(format!("xenterm-jump-layout-{}.db", uuid::Uuid::new_v4())),
            backup_dir: None,
            cache: ConfigFile {
                sessions,
                ..Default::default()
            },
            key: [0; 32],
            keyring_enabled: false,
            saved_state: std::sync::Mutex::new(SavedState::default()).into(),
        }));
        let mut probe = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let editor = cx.new(|_| SessionEditor::edit(store.clone(), target));
                Probe {
                    draft,
                    store,
                    editor,
                    width: Some(width),
                }
            });
            probe = Some(view.clone());
            Root::new(view, window, cx)
        });
        draw(cx);
        (probe.unwrap(), cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
    }

    #[gpui_kit::gpui::test]
    fn long_jump_routes_keep_all_controls_inside_the_editor(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        const CONTROLS: [[&str; 3]; 5] = [
            ["jump-up-0", "jump-down-0", "jump-remove-0"],
            ["jump-up-1", "jump-down-1", "jump-remove-1"],
            ["jump-up-2", "jump-down-2", "jump-remove-2"],
            ["jump-up-3", "jump-down-3", "jump-remove-3"],
            ["jump-up-4", "jump-down-4", "jump-remove-4"],
        ];
        for width in [900., 640.] {
            for count in [1, 2, 5] {
                let (_, cx) = long_route_fixture(cx, count, width);
                let editor = cx.debug_bounds("jump-editor-bounds").unwrap();
                for controls in CONTROLS.iter().take(count) {
                    for control in controls {
                        let bounds = cx.debug_bounds(control).unwrap();
                        assert!(bounds.size.width >= px(20.) && bounds.size.height >= px(20.));
                        assert!(bounds.left() >= editor.left() && bounds.right() <= editor.right(),
                            "{count} hops at {width}px: {control} is outside editor: {bounds:?} vs {editor:?}");
                        assert!(bounds.top() >= editor.top() && bounds.bottom() <= editor.bottom());
                    }
                }
            }
        }
    }

    #[gpui_kit::gpui::test]
    fn long_jump_menu_is_bounded_and_keyboard_can_select_the_last_candidate(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, cx) = long_route_fixture(cx, 0, 640.);
        let add = cx.debug_bounds("jump-add-control").unwrap();
        cx.simulate_click(
            point(add.right() - px(12.), add.center().y),
            gpui_kit::Modifiers::default(),
        );
        draw(cx);
        let menu = cx.update(|window, _| window.find("popup-menu").bounds());
        let viewport = cx.update(|window, _| window.viewport_size());
        assert!(
            menu.size.width <= px(562.) && menu.size.height <= px(322.),
            "{menu:?}"
        );
        assert!(menu.left() >= px(0.) && menu.right() <= viewport.width);
        assert!(menu.top() >= px(0.) && menu.bottom() <= viewport.height);
        let label = cx.debug_bounds("jump-menu-label-hop-00").unwrap();
        assert!(label.size.width <= px(528.) && label.right() <= menu.right());
        // The popup retains native keyboard navigation and scroll-to-selection.
        for _ in 0..16 {
            cx.simulate_keystrokes("down");
        }
        draw(cx);
        cx.simulate_keystrokes("enter");
        view.update(cx, |probe, cx| {
            assert_eq!(probe.draft.borrow().jump_session_ids, ["hop-15"]);
            assert!(probe
                .store
                .borrow()
                .get("target")
                .unwrap()
                .jump_session_ids
                .is_empty());
            cx.notify();
        });
        draw(cx);
        assert!(cx.update(|window, _| window.try_find("popup-menu").is_none()));
        let add = cx.debug_bounds("jump-add-control").unwrap();
        cx.simulate_click(
            point(add.right() - px(12.), add.center().y),
            gpui_kit::Modifiers::default(),
        );
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.update(|window, _| window.try_find("popup-menu").is_none()));
        view.read_with(cx, |probe, _| {
            assert_eq!(probe.draft.borrow().jump_session_ids, ["hop-15"])
        });
    }

    #[gpui_kit::gpui::test]
    fn long_jump_rows_keep_reorder_replace_and_remove_clicks_working(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = long_route_fixture(cx, 5, 640.);
        let down = cx.debug_bounds("jump-down-0").unwrap();
        cx.simulate_click(down.center(), gpui_kit::Modifiers::default());
        view.update(cx, |probe, cx| {
            assert_eq!(
                probe.draft.borrow().jump_session_ids,
                ["hop-01", "hop-00", "hop-02", "hop-03", "hop-04"]
            );
            cx.notify();
        });
        draw(cx);
        let trigger = cx.debug_bounds("jump-select-trigger-0").unwrap();
        cx.simulate_click(
            point(trigger.right() + px(16.), trigger.center().y),
            gpui_kit::Modifiers::default(),
        );
        draw(cx);
        let candidate = cx.debug_bounds("jump-menu-label-hop-05").unwrap();
        cx.simulate_click(candidate.center(), gpui_kit::Modifiers::default());
        view.update(cx, |probe, cx| {
            assert_eq!(probe.draft.borrow().jump_session_ids[0], "hop-05");
            cx.notify();
        });
        draw(cx);
        let remove = cx.debug_bounds("jump-remove-4").unwrap();
        cx.simulate_click(remove.center(), gpui_kit::Modifiers::default());
        view.update(cx, |probe, cx| {
            assert_eq!(
                probe.draft.borrow().jump_session_ids,
                ["hop-05", "hop-00", "hop-02", "hop-03"]
            );
            assert_eq!(
                probe.store.borrow().get("target").unwrap().jump_session_ids,
                ["hop-00", "hop-01", "hop-02", "hop-03", "hop-04"]
            );
            cx.notify();
        });
        draw(cx);
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
            saved_state: std::sync::Mutex::new(SavedState::default()).into(),
        }));
        let draft = Rc::new(RefCell::new(SessionDraft::from_session(&target)));
        let (view, cx) = cx.add_window_view(move |_, cx| {
            let editor = cx.new(|_| SessionEditor::edit(store.clone(), target));
            Probe {
                draft,
                store,
                editor,
                width: None,
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
