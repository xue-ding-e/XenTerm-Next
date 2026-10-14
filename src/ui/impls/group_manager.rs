//! The group manager: the folders the session list files sessions under.
//!
//! ## Why a dialog rather than the heading's menu
//!
//! The original renames and deletes a group from a right-click on its heading, and this
//! shell tried that first. It did not work: the heading is a row of the list's own, and a
//! right-click on it resolved to the session *below* it, so the menu that opened was about
//! the wrong thing. Rather than leave two actions that cannot be reached, they live here —
//! one surface, reachable by construction, listing every group with what it holds.
//!
//! ## One form, adding or editing
//!
//! The same shape as the quick-command manager, and for the same reason: a dialog that
//! opens to edit and closes to add makes the user track which mode they are in. With one
//! form on screen, the difference is what the button says and whether a row is selected.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputState},
        v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    },
    div,
    prelude::*,
    px, AnyElement, Context, Entity, FontWeight, IntoElement, Render, SharedString, Subscription,
    Window,
};

// The full Lucide catalog rather than the component library's curated subset.
use gpui_kit::assets::IconName;
use gpui_kit::gpui::Focusable as _;

use crate::config::ConfigStore;

/// What the manager wants the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GroupManagerAction {
    /// The groups changed, so the list that shows them should be rebuilt.
    Saved,
}

/// The group manager.
pub(crate) struct GroupManagerView {
    store: Rc<RefCell<ConfigStore>>,
    /// The group being renamed inline, with its own transient input. The
    /// edit deliberately does NOT touch the creation box: a half-typed new
    /// name there survives a rename elsewhere, and the old name never leaks
    /// into the "add" field.
    editing: Option<(String, Entity<InputState>, Subscription)>,
    /// The always-present creation box at the top of the list.
    create: Entity<InputState>,
    pending: Option<GroupManagerAction>,
    /// Why the rename in progress was refused, shown under its row: an empty
    /// name or one another group already has. The refusal keeps the draft —
    /// the user fixes a typo, not retypes the name (#98).
    rename_error: Option<&'static str>,
    /// The group whose delete button is armed (first press). Deleting a group
    /// keeps its sessions but takes their grouping with it, and regrouping a
    /// dozen sessions by hand is tedious enough that a stray click should not
    /// start it. Every other destructive row in this window asks first too.
    confirming_delete: Option<String>,
    _create_subscription: Subscription,
}

impl GroupManagerView {
    pub(crate) fn new(
        store: Rc<RefCell<ConfigStore>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let create = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::t(
                "新分组名称",
                "New group name",
            ))
        });
        // The add button enables and disables with the box, so typing has to
        // repaint this view.
        let _create_subscription = cx.subscribe_in(
            &create,
            window,
            |_: &mut Self, _, _: &gpui_kit::component::input::InputEvent, _, cx| cx.notify(),
        );

        Self {
            store,
            editing: None,
            create,
            pending: None,
            rename_error: None,
            confirming_delete: None,
            _create_subscription,
        }
    }

    /// Take the next action, if any.
    pub(crate) fn take_action(&mut self) -> Option<GroupManagerAction> {
        self.pending.take()
    }

    /// The groups this dialog manages, with how many sessions each holds.
    ///
    /// Reserved names are excluded because they are not the user's folders: `system` is
    /// where the built-in local shells live and `default` is the heading the ungrouped
    /// rows are shown under. Renaming one of those would be renaming a part of the
    /// interface.
    fn groups(&self) -> Vec<(String, usize)> {
        let store = self.store.borrow();
        let names = crate::config::named_display_groups(store.groups(), store.sessions());
        names
            .into_iter()
            .map(|name| {
                let count = store
                    .sessions()
                    .iter()
                    .filter(|session| session.group == name)
                    .count();
                (name, count)
            })
            .collect()
    }

    /// Stand down an armed delete, if any: acting elsewhere in the card is a
    /// decision, and leaving a live 确认按钮 behind one would be a trap.
    fn stand_down_delete(&mut self) {
        self.confirming_delete = None;
    }

    /// Begin an inline rename: a fresh input seeded with the current name,
    /// living only for as long as the edit. The creation box is untouched.
    fn start_edit(&mut self, group: String, window: &mut Window, cx: &mut Context<Self>) {
        self.stand_down_delete();
        self.rename_error = None;
        let input = cx.new(|cx| InputState::new(window, cx).default_value(group.clone()));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |_: &mut Self, _, _: &gpui_kit::component::input::InputEvent, _, cx| cx.notify(),
        );
        // Switching rows also retires the previous input. Carry its focus to
        // the replacement without taking focus from another control.
        if self
            .editing
            .as_ref()
            .is_some_and(|(_, previous, _)| previous.read(cx).focus_handle(cx).is_focused(window))
        {
            window.focus(&input.read(cx).focus_handle(cx), cx);
        }
        self.editing = Some((group, input, subscription));
        cx.notify();
    }

    /// End the inline rename without changing the creation draft. If the
    /// disappearing input owns focus, hand it to the creation box first: a
    /// dialog's close button dispatches Cancel through the current focus path.
    fn finish_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let restore_focus = self
            .editing
            .as_ref()
            .is_some_and(|(_, input, _)| input.read(cx).focus_handle(cx).is_focused(window));
        if restore_focus {
            let focus = self.create.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        self.editing = None;
        cx.notify();
    }

    /// Add the group the creation box names, then clear the box.
    fn add_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stand_down_delete();
        self.rename_error = None;
        let name = self.create.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        {
            let mut store = self.store.borrow_mut();
            store.add_group(name);
            if let Err(error) = store.save() {
                tracing::warn!("could not save the session groups: {error:#}");
            }
        }
        self.create
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.pending = Some(GroupManagerAction::Saved);
        cx.notify();
    }

    /// The caret's first stop when the card opens: the creation box, which
    /// is the one field this card always shows.
    pub(crate) fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create.update(cx, |field, cx| field.focus(window, cx));
    }

    /// What Enter means in this card: commit whatever is being typed, and close
    /// only when nothing is.
    ///
    /// The order matters. An inline rename that holds focus owns the key first —
    /// closing the card under an unfinished edit would lose it, while committing
    /// it is what the key says. Then a non-empty creation box adds the group and
    /// keeps the card open (adding several in a row is the normal flow). Only an
    /// empty box closes, and that loses nothing: every other change here applied
    /// the moment it was made.
    pub(crate) fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rename_focused = self
            .editing
            .as_ref()
            .is_some_and(|(_, input, _)| input.read(cx).focus_handle(cx).is_focused(window));
        if rename_focused {
            self.stand_down_delete();
            self.save_rename(window, cx);
            return false;
        }
        if self.create.read(cx).value().trim().is_empty() {
            return true;
        }
        self.add_group(window, cx);
        false
    }

    /// Write an inline rename through: the edited row's input names the new
    /// value; the row goes back to showing a plain name.
    fn save_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stand_down_delete();
        let Some((old, input)) = self
            .editing
            .as_ref()
            .map(|(group, input, _)| (group.clone(), input.clone()))
        else {
            return;
        };
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() {
            // Kept open with the reason under it: a silent discard of a
            // half-typed name reads as an Enter that did nothing (#98).
            self.rename_error = Some(crate::i18n::t(
                "名称不能为空。",
                "The name cannot be empty.",
            ));
            cx.notify();
            return;
        }
        if name == old {
            self.rename_error = None;
            self.finish_edit(window, cx);
            return;
        }
        if self.groups().iter().any(|(group, _)| group == &name) {
            self.rename_error = Some(crate::i18n::t(
                "已有同名分组。",
                "Another group already has that name.",
            ));
            cx.notify();
            return;
        }
        {
            let mut store = self.store.borrow_mut();
            store.rename_group(&old, name);
            if let Err(error) = store.save() {
                tracing::warn!("could not save the session groups: {error:#}");
            }
        }
        self.rename_error = None;
        self.pending = Some(GroupManagerAction::Saved);
        self.finish_edit(window, cx);
    }

    /// Delete a group, leaving its sessions ungrouped.
    fn delete(&mut self, group: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Two clicks: the first arms the row's confirm pair, the second (on the
        // same group) removes it. A different group re-aims there instead.
        if self.confirming_delete.as_deref() != Some(group) {
            self.confirming_delete = Some(group.to_string());
            cx.notify();
            return;
        }
        self.stand_down_delete();
        {
            let mut store = self.store.borrow_mut();
            store.remove_group(group);
            if let Err(error) = store.save() {
                tracing::warn!("could not save the session groups: {error:#}");
            }
        }
        self.pending = Some(GroupManagerAction::Saved);
        // An inline rename of the group that just went away is over.
        if self
            .editing
            .as_ref()
            .map(|(g, _, _)| g == group)
            .unwrap_or(false)
        {
            self.finish_edit(window, cx);
        }
        cx.notify();
    }

    /// The creation row: always at the top of the list, always ready. A new
    /// group is one keystroke away, not behind a mode switch — the old layout
    /// made the form the only way in, so adding a group meant reading a
    /// titled card first.
    fn create_row(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let can_add = !self.create.read(cx).value().trim().is_empty();

        h_flex()
            .w_full()
            .gap_2()
            .child(div().flex_1().min_w_0().child(Input::new(&self.create)))
            .child(
                Button::new("group-add")
                    .icon(Icon::new(IconName::Plus))
                    .label(crate::i18n::t("添加", "Add"))
                    .primary()
                    .small()
                    .disabled(!can_add)
                    .on_click(cx.listener(|this, _, window, cx| this.add_group(window, cx))),
            )
            .into_any_element()
    }

    /// One group: what it is called, what it holds, and what can be done to
    /// it. The row being renamed swaps its name for the input in place — the
    /// same inline-rename doctrine the tab strip follows, so the edit happens
    /// where the name lives instead of in a form somewhere else.
    fn row(&mut self, name: &str, count: usize, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let selected = self
            .editing
            .as_ref()
            .map(|(g, _, _)| g == name)
            .unwrap_or(false);
        let for_rename = name.to_string();
        let for_delete = name.to_string();
        let sessions = if count == 1 {
            crate::i18n::t("1 个会话", "1 session").to_string()
        } else {
            match crate::i18n::t("个会话", "sessions") {
                "个会话" => format!("{count} 个会话"),
                other => format!("{count} {other}"),
            }
        };

        h_flex()
            .w_full()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .items_center()
            .when(selected, |this| this.bg(theme.muted))
            .hover(|this| this.bg(theme.muted))
            .child(
                Icon::new(IconName::Folder)
                    .size_4()
                    .text_color(theme.muted_foreground),
            )
            .child(if selected {
                // The edit, in place: its own transient input where the name
                // was — nothing of it flows into the creation box above.
                let Some((_, input, _)) = self.editing.as_ref() else {
                    return div().into_any_element();
                };
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_1()
                            .child(div().flex_1().min_w_0().child(Input::new(input)))
                            .child(
                                Button::new(SharedString::from(format!(
                                    "group-save-{name}"
                                )))
                                .icon(Icon::new(IconName::Check))
                                .label(crate::i18n::t("保存", "Save"))
                                .primary()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_rename(window, cx)
                                })),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "group-cancel-{name}"
                                )))
                                .label(crate::i18n::t("取消", "Cancel"))
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.finish_edit(window, cx)
                                })),
                            ),
                    )
                    .when_some(
                        self.rename_error.filter(|_| self.editing.as_ref().is_some_and(|(g, _, _)| g == name)),
                        |this, error| {
                            this.child(
                                div()
                                    .w_full()
                                    .text_xs()
                                    .text_color(theme.danger)
                                    .child(error),
                            )
                        },
                    )
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .child(SharedString::from(name.to_string()))
                    .into_any_element()
            })
            .when(!selected, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(sessions)),
                )
                .child(if self.confirming_delete.as_deref() == Some(name) {
                    // Armed: the row asks for its own confirmation, in place,
                    // where the name it is about to ungroup is still readable.
                    h_flex()
                        .gap_1()
                        .flex_shrink_0()
                        .child(
                            Button::new(SharedString::from(format!(
                                "group-delete-confirm-{name}"
                            )))
                            .label(crate::i18n::t("确认删除", "Confirm delete"))
                            .danger()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.delete(&for_delete, window, cx)
                            })),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "group-delete-cancel-{name}"
                            )))
                            .label(crate::i18n::t("取消", "Cancel"))
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stand_down_delete();
                                cx.notify();
                            })),
                        )
                        .into_any_element()
                } else {
                    h_flex()
                        .gap_1()
                        .flex_shrink_0()
                        .child(
                            Button::new(SharedString::from(format!("group-edit-{name}")))
                                .icon(Icon::new(IconName::Pencil))
                                .ghost()
                                .small()
                                .tooltip(crate::i18n::t("重命名", "Rename"))
                                .accessibility_label(crate::i18n::t("重命名", "Rename"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_edit(for_rename.clone(), window, cx)
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("group-delete-{name}")))
                                .icon(Icon::new(IconName::Trash))
                                .ghost()
                                .small()
                                .tooltip(crate::i18n::t("删除分组", "Delete group"))
                                .accessibility_label(crate::i18n::t("删除分组", "Delete group"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.delete(&for_delete, window, cx)
                                })),
                        )
                        .into_any_element()
                })
            })
            .into_any_element()
    }
}

impl Render for GroupManagerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let groups = self.groups();

        let mut list: Vec<AnyElement> = Vec::new();
        if groups.is_empty() {
            list.push(
                div()
                    .w_full()
                    .py_4()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(crate::i18n::t(
                        "还没有分组。在上面输入名字添加一个，再从会话的右键菜单移入。",
                        "No groups yet. Type a name above to add one, then move sessions \
                         into it from their row menu.",
                    ))
                    .into_any_element(),
            );
        }
        for (name, count) in &groups {
            list.push(self.row(name, *count, cx));
        }

        // Creation is a row of the list, not a titled card above it: the
        // manager's job is a list of names, and "add one" is the first thing
        // you do with a list of names. Renaming keeps the inline swap — the
        // edited row's name becomes the input, where the change is visible
        // exactly where it lands.
        v_flex().size_full().bg(theme.background).child(
            v_flex()
                .id("group-manager")
                .size_full()
                .overflow_y_scroll()
                .gap_1()
                .p_3()
                .child(self.create_row(cx))
                .children(list),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::TestAppContext;

    /// Every group is listed once, with the number of sessions in it.
    ///
    /// The original builds this list by walking the *sessions*, taking each group header the
    /// first time it appears — which is why a group holding three sessions shows up once with
    /// a count of three rather than three times with a count of one. That is exactly the shape
    /// a rewrite gets wrong quietly, and the count is the number the manager exists to show.
    ///
    /// The store is the machine's own, read and never written.
    #[gpui_kit::gpui::test]
    fn each_group_is_listed_once_with_its_session_count(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let store = Rc::new(RefCell::new(
            crate::config::ConfigStore::load().expect("the configuration this machine has"),
        ));
        let (view, cx) = cx.add_window_view({
            let store = store.clone();
            move |window, cx| GroupManagerView::new(store, window, cx)
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        let listed = view.read_with(cx, |manager, _| manager.groups());
        let names: Vec<&String> = listed.iter().map(|(name, _)| name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            names.len(),
            sorted.len(),
            "a group appears once, however many sessions it holds: {names:?}"
        );

        let store = store.borrow();
        for (name, count) in &listed {
            let actual = store
                .sessions()
                .iter()
                .filter(|session| &session.group == name)
                .count();
            assert_eq!(
                *count, actual,
                "the count beside {name:?} is the number of sessions in it"
            );
        }

        // A group with nothing in it is still a group: the original lists empty folders so
        // they can be renamed or deleted.
        for name in store.groups() {
            assert!(
                names.iter().any(|listed| *listed == name),
                "the empty group {name:?} is missing from the manager"
            );
        }
    }
}
