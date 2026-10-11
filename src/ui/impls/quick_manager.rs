//! The quick-command manager: where the dock's commands come from.
//!
//! ## What it is
//!
//! The original's manage dialog, as a full-window overlay rather than a modal: a form for
//! one entry — name, command, group, and whether a click runs it or only types it — above
//! the list those entries make. Selecting a row loads it into the form, which is the whole
//! interaction: there is one form, and it is either adding or editing.
//!
//! ## Why the form is always visible
//!
//! A dialog that opens to edit and closes to add makes the user keep track of which mode
//! they are in. With one form on screen, "add" and "edit" differ only in what the button
//! says and in whether the list has a row selected — and a half-typed command survives
//! clicking around the list, which is what a user does while deciding where an entry
//! belongs.
//!
//! ## Groups
//!
//! A group is a name on an entry, so adding one is typing it, and a group disappears when
//! its last entry is deleted or moved. That is the config's model, and this view does not
//! invent a second one: the group field offers the names already in use, and accepts one
//! that is not.

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

// The full Lucide catalog rather than the component library's curated subset: the icons
// this view needs for its own actions are not in that subset.
use gpui_kit::assets::IconName;

use crate::config::{ConfigStore, QuickCommand};
use crate::core::quick;

/// What the manager wants the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QuickManagerAction {
    /// The list changed, so anything else showing it — the dock — should redraw.
    Saved,
}

/// The manager.
pub(crate) struct QuickManagerView {
    store: Rc<RefCell<ConfigStore>>,
    /// The entry the form is editing, or `None` when it would add one.
    editing: Option<usize>,
    name: Entity<InputState>,
    command: Entity<InputState>,
    group: Entity<InputState>,
    /// The group the group form would rename, or `None` when it would add one.
    ///
    /// A group is a name shared by entries, so renaming one is a decision about all of
    /// them; the form says which group it is about rather than making the user remember
    /// what the field was filled from.
    renaming_group: Option<String>,
    group_name: Entity<InputState>,
    send_enter: bool,
    pending: Option<QuickManagerAction>,
    _subscriptions: Vec<Subscription>,
}

impl QuickManagerView {
    pub(crate) fn new(
        store: Rc<RefCell<ConfigStore>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::t("名称", "Name")));
        let command = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::t("要发送的命令", "The command to run"))
        });
        let group = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::i18n::t("分组(可留空)", "Group (optional)"))
        });
        let group_name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::t("分组名", "Group name"))
        });

        // The three fields are what a save reads, so their changes are what tells the form
        // it has something to save — which is the button's enabled state.
        let mut subscriptions = Vec::new();
        for input in [&name, &command, &group, &group_name] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |view: &mut Self, _, _: &gpui_kit::component::input::InputEvent, _, cx| {
                    cx.notify();
                    let _ = view;
                },
            ));
        }

        Self {
            store,
            editing: None,
            name,
            command,
            group,
            renaming_group: None,
            group_name,
            send_enter: true,
            pending: None,
            _subscriptions: subscriptions,
        }
    }

    /// Take the next action, if any.
    pub(crate) fn take_action(&mut self) -> Option<QuickManagerAction> {
        self.pending.take()
    }

    /// Load a row into the form, or clear the form when `index` is `None`.
    fn select(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let entry = index.and_then(|index| {
            self.store
                .borrow()
                .quick_commands()
                .get(index)
                .map(|entry| (index, entry.clone()))
        });
        match entry {
            Some((index, entry)) => {
                self.editing = Some(index);
                self.send_enter = entry.send_enter;
                self.name.update(cx, |input, cx| {
                    input.set_value(entry.name.clone(), window, cx)
                });
                self.command.update(cx, |input, cx| {
                    input.set_value(entry.command.clone(), window, cx)
                });
                self.group.update(cx, |input, cx| {
                    input.set_value(entry.group.clone(), window, cx)
                });
            }
            None => {
                self.editing = None;
                self.send_enter = true;
                for input in [&self.name, &self.command, &self.group] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
            }
        }
        cx.notify();
    }

    /// Write the form into the store: over the selected entry, or as a new one.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().to_string();
        let command = self.command.read(cx).value().to_string();
        // An entry with no name has no row to click in the dock, and one with no command
        // would send nothing: the form refuses rather than saving something unusable.
        if name.trim().is_empty() || command.trim().is_empty() {
            return;
        }
        let group = self.group.read(cx).value().trim().to_string();
        let entry = QuickCommand {
            name: name.trim().to_string(),
            command: command.clone(),
            group: group.clone(),
            send_enter: self.send_enter,
        };

        {
            let mut store = self.store.borrow_mut();
            match self.editing {
                Some(index) => store.update_quick_command(index, entry),
                None => {
                    let mut commands = store.quick_commands().to_vec();
                    commands.push(entry);
                    store.set_quick_commands(commands);
                    // A group the user typed is registered, so it keeps its header even
                    // when its entries are all moved away — the difference between a group
                    // and a spelling mistake.
                    if !group.is_empty() {
                        store.add_quick_group(group.clone());
                    }
                }
            }
            if let Err(error) = store.save() {
                tracing::warn!("could not save the quick commands: {error:#}");
            }
        }

        self.pending = Some(QuickManagerAction::Saved);
        self.select(None, window, cx);
    }

    /// Create a group, or rename the one the form is about.
    ///
    /// The name a group is known by is the name its entries carry, which is why this goes
    /// through the store's own `rename_quick_group` rather than only changing the header:
    /// a header that disagreed with its entries would split one group into two.
    fn save_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.group_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        {
            let mut store = self.store.borrow_mut();
            match self.renaming_group.take() {
                Some(old) => store.rename_quick_group(&old, name.clone()),
                None => store.add_quick_group(name.clone()),
            }
            if let Err(error) = store.save() {
                tracing::warn!("could not save the quick groups: {error:#}");
            }
        }
        self.group_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.pending = Some(QuickManagerAction::Saved);
        cx.notify();
    }

    /// Delete a group, and with it the grouping of the entries that were in it.
    ///
    /// The entries stay: deleting a folder is not deleting what is filed in it, and the
    /// store's own method moves them back to the ungrouped list.
    fn delete_group(&mut self, name: &str, cx: &mut Context<Self>) {
        {
            let mut store = self.store.borrow_mut();
            store.remove_quick_group(name);
            if let Err(error) = store.save() {
                tracing::warn!("could not save the quick groups: {error:#}");
            }
        }
        if self.renaming_group.as_deref() == Some(name) {
            self.renaming_group = None;
        }
        self.pending = Some(QuickManagerAction::Saved);
        cx.notify();
    }

    /// Delete the selected entry.
    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.editing else {
            return;
        };
        {
            let mut store = self.store.borrow_mut();
            let mut commands = store.quick_commands().to_vec();
            if index >= commands.len() {
                return;
            }
            commands.remove(index);
            store.set_quick_commands(commands);
            if let Err(error) = store.save() {
                tracing::warn!("could not save the quick commands: {error:#}");
            }
        }
        self.pending = Some(QuickManagerAction::Saved);
        self.select(None, window, cx);
    }

    /// Add a copy of the selected entry, right after it.
    fn duplicate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.editing else {
            return;
        };
        {
            let mut store = self.store.borrow_mut();
            let mut commands = store.quick_commands().to_vec();
            let Some(entry) = commands.get(index).cloned() else {
                return;
            };
            let copy = QuickCommand {
                name: format!("{} 2", entry.name),
                ..entry
            };
            commands.insert(index + 1, copy);
            store.set_quick_commands(commands);
            if let Err(error) = store.save() {
                tracing::warn!("could not save the quick commands: {error:#}");
            }
        }
        self.pending = Some(QuickManagerAction::Saved);
        self.select(Some(index + 1), window, cx);
    }

    /// Move the selected entry within its group.
    fn move_entry(&mut self, move_up: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.editing else {
            return;
        };
        let target = {
            let mut store = self.store.borrow_mut();
            let mut commands = store.quick_commands().to_vec();
            let Some(target) = quick::reorder(&mut commands, index, move_up) else {
                return;
            };
            store.set_quick_commands(commands);
            if let Err(error) = store.save() {
                tracing::warn!("could not save the quick commands: {error:#}");
            }
            target
        };
        self.pending = Some(QuickManagerAction::Saved);
        self.select(Some(target), window, cx);
    }

    /// The form: the three fields, the switch, and the buttons that act on them.
    fn form(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let editing = self.editing.is_some();
        let can_save = !self.name.read(cx).value().trim().is_empty()
            && !self.command.read(cx).value().trim().is_empty();

        v_flex()
            .w_full()
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if editing {
                        crate::i18n::t("编辑命令", "Edit a command")
                    } else {
                        crate::i18n::t("新建命令", "New command")
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(div().w(px(200.)).child(Input::new(&self.name)))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.command)))
                    .child(div().w(px(160.)).child(Input::new(&self.group))),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_center()
                    .child(
                        // A button rather than the library's switch: the switch is a form
                        // control with its own label conventions, and this one has one
                        // meaning to state in full.
                        Button::new("quick-send-enter")
                            .icon(Icon::new(if self.send_enter {
                                IconName::SquareCheck
                            } else {
                                IconName::Square
                            }))
                            .label(crate::i18n::t("点击后直接执行", "Run on click"))
                            .ghost()
                            .small()
                            .tooltip(crate::i18n::t(
                                "关闭则只把命令填入输入框,便于修改后再执行。",
                                "Off drops the command into the command bar instead, so it \
                                 can be edited before running.",
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send_enter = !this.send_enter;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("quick-save")
                            .icon(Icon::new(IconName::Check))
                            .label(if editing {
                                crate::i18n::t("保存", "Save")
                            } else {
                                crate::i18n::t("添加", "Add")
                            })
                            .primary()
                            .small()
                            .disabled(!can_save)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .when(editing, |this| {
                        this.child(
                            Button::new("quick-cancel")
                                .label(crate::i18n::t("取消", "Cancel"))
                                .ghost()
                                .small()
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.select(None, window, cx)
                                    }),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// One row of the list: what it is, and what can be done to it.
    fn row(&self, row: &quick::QuickRow, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let index = row.index;
        let selected = index.is_some() && index == self.editing;

        h_flex()
            // Scope the repeated action-button IDs to this command row.
            .id(SharedString::from(format!("quick-manager-row-{}", index.unwrap())))
            .debug_selector(|| format!("quick-manager-row-{}", index.unwrap()))
            .w_full()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .items_center()
            .when(selected, |this| this.bg(theme.muted))
            .hover(|this| this.bg(theme.muted))
            .child(
                div()
                    .w(px(180.))
                    .flex_shrink_0()
                    .truncate()
                    .text_sm()
                    .child(SharedString::from(row.name.clone())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(row.command.clone())),
            )
            .when(!row.send_enter, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(crate::i18n::t("仅填入", "Fills only")),
                )
            })
            .child(
                h_flex()
                    .gap_1()
                    .flex_shrink_0()
                    .child(
                        Button::new("quick-select")
                            .icon(Icon::new(IconName::Pencil))
                            .ghost()
                            .small()
                            .tooltip(crate::i18n::t("编辑", "Edit"))
                            .accessibility_label(crate::i18n::t("编辑", "Edit"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(index, window, cx)
                            })),
                    )
                    .child(
                        Button::new("quick-row-up")
                            .icon(Icon::new(IconName::ArrowUp))
                            .ghost()
                            .small()
                            .tooltip(crate::i18n::t("上移", "Move up"))
                            .accessibility_label(crate::i18n::t("上移", "Move up"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(index, window, cx);
                                this.move_entry(true, window, cx);
                            })),
                    )
                    .child(
                        Button::new("quick-row-down")
                            .icon(Icon::new(IconName::ArrowDown))
                            .ghost()
                            .small()
                            .tooltip(crate::i18n::t("下移", "Move down"))
                            .accessibility_label(crate::i18n::t("下移", "Move down"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(index, window, cx);
                                this.move_entry(false, window, cx);
                            })),
                    )
                    .child(
                        Button::new("quick-row-copy")
                            .icon(Icon::new(IconName::Copy))
                            .ghost()
                            .small()
                            .tooltip(crate::i18n::t("复制", "Duplicate"))
                            .accessibility_label(crate::i18n::t("复制", "Duplicate"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(index, window, cx);
                                this.duplicate(window, cx);
                            })),
                    )
                    .child(
                        Button::new("quick-row-delete")
                            .icon(Icon::new(IconName::Trash))
                            .ghost()
                            .small()
                            .tooltip(crate::i18n::t("删除", "Delete"))
                            .accessibility_label(crate::i18n::t("删除", "Delete"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(index, window, cx);
                                this.delete(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The group form: name a new group, or rename the one a heading's pencil picked.
    ///
    /// One field rather than a list of editable rows, because a group is a name and
    /// nothing else — the entries that carry it are the rows below, and a rename has to
    /// agree with all of them.
    fn group_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let renaming = self.renaming_group.clone();
        let can_save = !self.group_name.read(cx).value().trim().is_empty();
        let mut row = h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .child(
                Icon::new(IconName::FolderCog)
                    .size_4()
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(match &renaming {
                        Some(name) => SharedString::from(format!(
                            "{} {name}",
                            crate::i18n::t("重命名分组", "Rename group")
                        )),
                        None => SharedString::from(crate::i18n::t("新建分组", "New group")),
                    }),
            )
            .child(div().w(px(200.)).child(Input::new(&self.group_name)));

        if renaming.is_some() {
            row = row.child(
                Button::new("quick-group-cancel")
                    .icon(Icon::new(IconName::X))
                    .ghost()
                    .small()
                    .tooltip(crate::i18n::t("取消重命名", "Cancel the rename"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.renaming_group = None;
                        this.group_name
                            .update(cx, |input, cx| input.set_value("", window, cx));
                        cx.notify();
                    })),
            );
        }

        row.child(div().flex_1())
            .child(
                Button::new("quick-group-save")
                    .icon(Icon::new(IconName::Check))
                    .label(match renaming {
                        Some(_) => crate::i18n::t("重命名", "Rename"),
                        None => crate::i18n::t("创建", "Create"),
                    })
                    .primary()
                    .small()
                    .disabled(!can_save)
                    .on_click(cx.listener(|this, _, window, cx| this.save_group(window, cx))),
            )
            .into_any_element()
    }
}

impl Render for QuickManagerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Built before the tree: the form needs `&mut cx` for its input entities, and the
        // theme borrows `cx` immutably.
        let form = self.form(cx);
        let theme = cx.theme();
        let rows = {
            let store = self.store.borrow();
            quick::rows(store.quick_commands(), store.quick_groups())
        };

        let mut list: Vec<AnyElement> = Vec::new();
        if rows.is_empty() {
            list.push(
                div()
                    .w_full()
                    .py_4()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(crate::i18n::t(
                        "还没有快速命令。在上面的表单里添加一条。",
                        "No quick commands yet. Add one with the form above.",
                    ))
                    .into_any_element(),
            );
        }
        for row in &rows {
            // A group heading is not selectable: it stands for the rows under it, and the
            // buttons beside an entry act on that entry.
            if row.header {
                // The default group is reserved: the store refuses to create a second one
                // and the entries with no group belong to it, so it is shown without the
                // two buttons rather than with buttons that would fail.
                let reserved = row.group.eq_ignore_ascii_case("default");
                let for_rename = row.group.clone();
                let for_delete = row.group.clone();
                list.push(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .px_2()
                        .pt_2()
                        .items_center()
                        .child(
                            Icon::new(IconName::Folder)
                                .size_3()
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.muted_foreground)
                                .child(SharedString::from(row.group.clone())),
                        )
                        .child(div().flex_1())
                        .when(!reserved, |this| {
                            this.child(
                                Button::new(SharedString::from(format!(
                                    "quick-group-rename-{}",
                                    row.group
                                )))
                                .icon(Icon::new(IconName::Pencil))
                                .ghost()
                                .small()
                                .tooltip(crate::i18n::t("重命名分组", "Rename the group"))
                                .accessibility_label(crate::i18n::t(
                                    "重命名分组",
                                    "Rename the group",
                                ))
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.renaming_group = Some(for_rename.clone());
                                        this.group_name.update(cx, |input, cx| {
                                            input.set_value(for_rename.clone(), window, cx)
                                        });
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "quick-group-delete-{}",
                                    row.group
                                )))
                                .icon(Icon::new(IconName::Trash))
                                .ghost()
                                .small()
                                .tooltip(crate::i18n::t(
                                    "删除分组(命令会移到未分组)",
                                    "Delete the group (its commands move to the ungrouped list)",
                                ))
                                .accessibility_label(crate::i18n::t("删除分组", "Delete the group"))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.delete_group(&for_delete, cx);
                                    },
                                )),
                            )
                        })
                        .into_any_element(),
                );
            }
            if row.index.is_some() {
                list.push(self.row(row, cx));
            }
        }

        v_flex().size_full().bg(theme.background).child(
            v_flex()
                .id("quick-manager")
                .size_full()
                .overflow_y_scroll()
                .gap_2()
                .p_3()
                .child(form)
                .child(self.group_form(cx))
                .children(list),
        )
    }
}

#[cfg(test)]
#[path = "quick_manager_row_tests.rs"]
mod row_tests;
