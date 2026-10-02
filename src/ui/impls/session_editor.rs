//! The session editor: create a session, or change one.
//!
//! ## What this is responsible for
//!
//! The form itself, and nothing else. What a filled-in form *means* is
//! [`crate::core::SessionDraft::to_session`]'s job — the defaults per kind, the rule
//! that a blank password keeps the saved one, the dropping of half-filled rows. Editing
//! that logic here would give the two frontends two answers to "what did the user
//! write", which is exactly the split `SessionDraft` was extracted to prevent.
//!
//! ## Why the fields are the library's, and the state is ours
//!
//! `SettingField`'s closures take `&mut App`, so they cannot borrow the view. The draft
//! therefore lives in an `Rc<RefCell<SessionDraft>>` that every field captures, and the
//! view re-reads it when it saves. That is the same shape `SettingsView` uses for the
//! config store, and it is why a field can be declared with two closures instead of a
//! message enum per control.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputState},
        setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
        v_flex, ActiveTheme, AxisExt as _, Icon, Sizable as _,
    },
    div,
    prelude::*,
    px, relative, AnyElement, Context, Entity, Hsla, IntoElement, Render, SharedString, Window,
};

use gpui_kit::assets::IconName;

use crate::config::{ConfigStore, Session};
use crate::core::SessionDraft;

/// What the editor asks the shell to do when it closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorOutcome {
    /// The user cancelled; nothing changed.
    Cancelled,
    /// The user saved; the session is already written and persisted.
    Saved,
}

/// The session editor.
pub(crate) struct SessionEditor {
    /// The form's working state, shared with every field's closures.
    draft: Rc<RefCell<SessionDraft>>,
    /// The session being edited, for the two secrets a form must not echo back.
    original: Option<Session>,
    /// The store to write into.
    store: Rc<RefCell<ConfigStore>>,
    /// Set when the dialog is finished, so the shell can take the outcome.
    outcome: Option<EditorOutcome>,
    /// The port-forwarding rows. Their text fields are entities and so cannot be rebuilt
    /// every frame the way the settings fields are; the rows are kept here instead, and
    /// copied into the draft when the form is saved — the only moment they are read.
    forwards: Rc<RefCell<Vec<ForwardRow>>>,
    /// What the next forwarding row is called. A counter rather than a row index,
    /// because a row removed from the middle would otherwise hand its identity — and its
    /// element ids — to the row that took its place.
    next_forward_id: u64,
    /// The expect/response rows, on the same terms as the forwarding ones: their fields
    /// are entities, so the rows live here and reach the draft when the form is saved.
    triggers: Rc<RefCell<Vec<TriggerRow>>>,
    /// What the next trigger row is called, for the same reason as `next_forward_id`.
    next_trigger_id: u64,
    /// The password box, masked (audit N-低1): its text lives in the entity and
    /// reaches the draft when the form is saved, like the table rows.
    password: Option<Entity<InputState>>,
}

/// One row of the trigger table: what to watch for, and what to answer.
struct TriggerRow {
    id: u64,
    expect: Entity<InputState>,
    response: Entity<InputState>,
    /// Send the answer followed by Enter, which is what a prompt wants.
    append_enter: bool,
    /// Keep answering every time the pattern appears, rather than once per session.
    repeat: bool,
}

impl TriggerRow {
    fn new(
        id: u64,
        draft: &crate::core::TriggerDraft,
        window: &mut Window,
        cx: &mut Context<SessionEditor>,
    ) -> Self {
        let mut field =
            |placeholder: &'static str, value: &str, cx: &mut Context<SessionEditor>| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value.to_string())
                })
            };
        Self {
            id,
            expect: field(
                crate::i18n::t("等待出现", "Wait for"),
                draft.expect.as_str(),
                cx,
            ),
            response: field(
                crate::i18n::t("回答", "Answer"),
                draft.response.as_str(),
                cx,
            ),
            append_enter: draft.append_enter,
            repeat: draft.repeat,
        }
    }

    /// What this row holds, as the draft the config takes.
    fn draft(&self, cx: &Context<SessionEditor>) -> crate::core::TriggerDraft {
        crate::core::TriggerDraft {
            expect: self.expect.read(cx).value().to_string(),
            response: self.response.read(cx).value().to_string(),
            append_enter: self.append_enter,
            repeat: self.repeat,
        }
    }
}

/// One row of the port-forwarding table.
///
/// The kind is a plain string because it is chosen from a cycling chip rather than
/// typed; the other four are inputs.
struct ForwardRow {
    id: u64,
    kind: String,
    name: Entity<InputState>,
    bind_port: Entity<InputState>,
    host: Entity<InputState>,
    host_port: Entity<InputState>,
}

impl ForwardRow {
    fn new(
        id: u64,
        draft: &crate::core::PortForwardDraft,
        window: &mut Window,
        cx: &mut Context<SessionEditor>,
    ) -> Self {
        let mut field =
            |placeholder: &'static str, value: &str, cx: &mut Context<SessionEditor>| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value.to_string())
                })
            };
        Self {
            id,
            kind: if draft.kind.trim().is_empty() {
                "local".to_string()
            } else {
                draft.kind.clone()
            },
            name: field(crate::i18n::t("名称", "Name"), draft.name.as_str(), cx),
            bind_port: field(
                crate::i18n::t("监听端口", "Listen port"),
                draft.bind_port.as_str(),
                cx,
            ),
            host: field(
                crate::i18n::t("目标主机", "Target host"),
                draft.host.as_str(),
                cx,
            ),
            host_port: field(
                crate::i18n::t("目标端口", "Target port"),
                draft.host_port.as_str(),
                cx,
            ),
        }
    }

    /// What this row holds, as the draft the config takes.
    fn draft(&self, cx: &Context<SessionEditor>) -> crate::core::PortForwardDraft {
        crate::core::PortForwardDraft {
            kind: self.kind.clone(),
            name: self.name.read(cx).value().to_string(),
            bind_addr: "127.0.0.1".to_string(),
            bind_port: self.bind_port.read(cx).value().to_string(),
            host: self.host.read(cx).value().to_string(),
            host_port: self.host_port.read(cx).value().to_string(),
        }
    }
}

impl SessionEditor {
    /// An editor for a brand-new session.
    pub(crate) fn new_session(store: Rc<RefCell<ConfigStore>>, group: String) -> Self {
        let mut draft = SessionDraft::new_ssh();
        draft.group = group;
        Self {
            draft: Rc::new(RefCell::new(draft)),
            original: None,
            store,
            outcome: None,
            forwards: Rc::new(RefCell::new(Vec::new())),
            next_forward_id: 1,
            triggers: Rc::new(RefCell::new(Vec::new())),
            next_trigger_id: 1,
            password: None,
        }
    }

    /// An editor for an existing session.
    pub(crate) fn edit(store: Rc<RefCell<ConfigStore>>, session: Session) -> Self {
        Self {
            draft: Rc::new(RefCell::new(SessionDraft::from_session(&session))),
            original: Some(session),
            store,
            outcome: None,
            forwards: Rc::new(RefCell::new(Vec::new())),
            next_forward_id: 1,
            triggers: Rc::new(RefCell::new(Vec::new())),
            next_trigger_id: 1,
            password: None,
        }
    }

    /// Take the outcome, if the editor has finished.
    pub(crate) fn take_outcome(&mut self) -> Option<EditorOutcome> {
        self.outcome.take()
    }

    /// The port-forwarding rows, created on the first frame.
    ///
    /// They are built from the draft here rather than in the constructors because an
    /// input needs a window, and neither constructor has one. `next_forward_id` is what
    /// keeps this from re-seeding after the user has removed every row: only the initial
    /// id means "nothing has been created yet".
    fn seed_forwards(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.next_forward_id != 1 || !self.forwards.borrow().is_empty() {
            return;
        }
        let seeded: Vec<_> = self.draft.borrow().forwards.clone();
        for draft in seeded {
            let id = self.next_forward_id;
            self.next_forward_id += 1;
            self.forwards
                .borrow_mut()
                .push(ForwardRow::new(id, &draft, window, cx));
        }
    }

    /// Copy what the forwarding rows hold into the draft, which is what the save reads.
    ///
    /// The guard is the id counter and not "are there any rows": a user who deletes every
    /// rule and saves means to have none, and writing the empty list only when a row was
    /// ever created is what tells that apart from a form whose table was never seeded.
    /// Skipping the empty case outright — which this did until a test was written for it —
    /// silently kept the rules the user had just removed.
    fn sync_forwards(&self, cx: &Context<Self>) {
        if self.next_forward_id == 1 && self.forwards.borrow().is_empty() {
            return;
        }
        let rows: Vec<_> = self
            .forwards
            .borrow()
            .iter()
            .map(|row| row.draft(cx))
            .collect();
        self.draft.borrow_mut().forwards = rows;
    }

    /// The trigger rows, created on the first frame, for the reason `seed_forwards`
    /// records.
    fn seed_triggers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.next_trigger_id != 1 || !self.triggers.borrow().is_empty() {
            return;
        }
        let seeded: Vec<_> = self.draft.borrow().triggers.clone();
        for draft in seeded {
            let id = self.next_trigger_id;
            self.next_trigger_id += 1;
            self.triggers
                .borrow_mut()
                .push(TriggerRow::new(id, &draft, window, cx));
        }
    }

    /// Copy the trigger rows into the draft, on the same terms as the forwarding ones.
    fn sync_triggers(&self, cx: &Context<Self>) {
        if self.next_trigger_id == 1 && self.triggers.borrow().is_empty() {
            return;
        }
        let rows: Vec<_> = self
            .triggers
            .borrow()
            .iter()
            .map(|row| row.draft(cx))
            .collect();
        self.draft.borrow_mut().triggers = rows;
    }

    /// Write the draft and persist it.
    ///
    /// Saving is one step rather than two, because a session that exists only in memory
    /// is a session the list shows and the next window does not: a save the user was
    /// not told about having failed is worse than no save at all.
    pub(crate) fn heading(&self) -> &'static str {
        if self.original.is_some() {
            crate::i18n::t("编辑会话", "Edit session")
        } else {
            crate::i18n::t("新建会话", "New session")
        }
    }

    fn save(&mut self, cx: &Context<Self>) {
        // The forwarding rows hold their own text, so it is copied into the draft first:
        // what the save reads is the draft, and a row typed into but never copied across
        // would be a rule the form showed and the file did not have.
        self.sync_forwards(cx);
        self.sync_triggers(cx);
        self.sync_password(cx);
        let session = {
            let draft = self.draft.borrow();
            draft.to_session(self.original.as_ref())
        };
        let mut store = self.store.borrow_mut();
        // A new session has no id until one is minted here; the store cannot know which
        // drafts are new.
        let mut session = session;
        if session.id.is_empty() {
            session.id = uuid::Uuid::new_v4().to_string();
        }
        store.upsert(session);
        if let Err(error) = store.save() {
            tracing::warn!("could not save the session: {error:#}");
        }
        self.outcome = Some(EditorOutcome::Saved);
    }

    /// Copy what the password box holds into the draft. Empty keeps the
    /// original password — `to_session` decides that, unchanged.
    fn sync_password(&self, cx: &Context<Self>) {
        if let Some(input) = self.password.as_ref() {
            self.draft.borrow_mut().password = input.read(cx).value().to_string();
        }
    }

    /// Create the masked password box on the first frame that draws it (an
    /// input needs a window; the constructors have none).
    fn seed_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.password.is_some() {
            return;
        }
        let initial = self.draft.borrow().password.clone();
        self.password = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(crate::i18n::t(
                    "留空保留已保存的密码",
                    "Leave empty to keep the saved password",
                ))
                .default_value(initial)
        }));
    }

    /// One text field bound to a draft field.
    fn text(
        draft: &Rc<RefCell<SessionDraft>>,
        get: impl Fn(&SessionDraft) -> String + 'static,
        set: impl Fn(&mut SessionDraft, String) + 'static,
    ) -> SettingField<SharedString> {
        let read = draft.clone();
        let write = draft.clone();
        SettingField::input(
            move |_| SharedString::from(get(&read.borrow())),
            move |value, _| set(&mut write.borrow_mut(), value.to_string()),
        )
    }

    /// One dropdown bound to a draft field.
    ///
    /// `editor` is what makes the dependent fields follow: the kind dropdown
    /// decides whether the connection rows read as SSH or as serial, the auth
    /// dropdown decides which credential rows exist — and none of that is
    /// visible unless the editor itself is told to re-render. The setter only
    /// receives an `App`, so the handle travels with it.
    fn choice(
        draft: &Rc<RefCell<SessionDraft>>,
        editor: gpui_kit::WeakEntity<Self>,
        options: Vec<(SharedString, SharedString)>,
        get: impl Fn(&SessionDraft) -> String + 'static,
        set: impl Fn(&mut SessionDraft, String) + 'static,
    ) -> SettingField<SharedString> {
        let read = draft.clone();
        let write = draft.clone();
        SettingField::dropdown(
            options,
            move |_| SharedString::from(get(&read.borrow())),
            move |value, cx| {
                set(&mut write.borrow_mut(), value.to_string());
                if let Some(editor) = editor.upgrade() {
                    editor.update(cx, |_, cx| cx.notify());
                }
            },
        )
    }
}

/// The port-forwarding table, as a settings field draws it.
///
/// A free function rather than a method because a field's closure is handed an `App` and
/// not this view's context: the rows are reached through the handle it shares with the
/// editor, and anything that changes them goes through the editor's entity, which is
/// safe in a click handler and would not be during a draw.
fn forwards_element(
    rows: Rc<RefCell<Vec<ForwardRow>>>,
    editor: gpui_kit::WeakEntity<SessionEditor>,
    border: Hsla,
    muted: Hsla,
) -> AnyElement {
    let mut list: Vec<AnyElement> = Vec::new();
    for row in rows.borrow().iter() {
        let id = row.id;
        let kind = row.kind.clone();
        let for_kind = editor.clone();
        let for_remove = editor.clone();
        list.push(
            h_flex()
                .w_full()
                .min_w_0()
                .flex_wrap()
                .gap_2()
                .items_center()
                .child(
                    // One chip per row that cycles the three kinds: they are a word each,
                    // and a click that says the next one is cheaper than three choices
                    // standing open in every row.
                    Button::new(SharedString::from(format!("forward-kind-{id}")))
                        .label(kind_word(&kind))
                        .small()
                        .outline()
                        .flex_shrink_0()
                        .tooltip(crate::i18n::t(
                            "点击切换：本地 / 远程 / 动态",
                            "Click to cycle: local / remote / dynamic",
                        ))
                        .on_click(move |_, _, cx| {
                            if let Some(editor) = for_kind.upgrade() {
                                let _ = editor.update(cx, |editor, cx| {
                                    if let Some(row) =
                                        editor.forwards.borrow_mut().iter_mut().find(|r| r.id == id)
                                    {
                                        row.kind = match row.kind.as_str() {
                                            "local" => "remote".to_string(),
                                            "remote" => "dynamic".to_string(),
                                            _ => "local".to_string(),
                                        };
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                )
                // The row wraps in *units*, not per widget: name+listen
                // port is one, arrow+host+target port is another. Each unit
                // carries a relative basis (something for the wrap to
                // measure) and a floor, so a wide pane lays everything on one
                // line, and a narrow one moves whole units to a second line —
                // never a bare input crushed to a sliver, never a column
                // pouring out of the card. Placeholders live on the input
                // states, set when the row was built.
                .child(
                    h_flex()
                        .w(relative(0.52))
                        .min_w(px(230.))
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().min_w(px(90.)).child(Input::new(&row.name)))
                        .child(
                            div()
                                .w(px(90.))
                                .flex_shrink_0()
                                .child(Input::new(&row.bind_port)),
                        ),
                )
                .child(
                    h_flex()
                        .w(relative(0.44))
                        .min_w(px(250.))
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::ArrowRight).size_3().text_color(muted))
                        .child(div().flex_1().min_w(px(110.)).child(Input::new(&row.host)))
                        .child(
                            div()
                                .w(px(90.))
                                .flex_shrink_0()
                                .child(Input::new(&row.host_port)),
                        ),
                )
                .child(
                    Button::new(SharedString::from(format!("forward-remove-{id}")))
                        .icon(IconName::Trash)
                        .ghost()
                        .small()
                        .tooltip(crate::i18n::t("删除这条规则", "Remove this rule"))
                        .accessibility_label(crate::i18n::t("删除这条规则", "Remove this rule"))
                        .on_click(move |_, _, cx| {
                            if let Some(editor) = for_remove.upgrade() {
                                let _ = editor.update(cx, |editor, cx| {
                                    editor.forwards.borrow_mut().retain(|row| row.id != id);
                                    cx.notify();
                                });
                            }
                        }),
                )
                .into_any_element(),
        );
    }

    let empty = list.is_empty();
    let for_add = editor;
    v_flex()
        .w_full()
        .min_w_0()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(border)
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(
                    Button::new("forward-add")
                        .icon(IconName::Plus)
                        .label(crate::i18n::t("添加规则", "Add a rule"))
                        .small()
                        .outline()
                        .flex_shrink_0()
                        .on_click(move |_, window, cx| {
                            if let Some(editor) = for_add.upgrade() {
                                let _ = editor.update(cx, |editor, cx| {
                                    let id = editor.next_forward_id;
                                    editor.next_forward_id += 1;
                                    let blank = crate::core::PortForwardDraft {
                                        kind: "local".to_string(),
                                        bind_addr: "127.0.0.1".to_string(),
                                        ..Default::default()
                                    };
                                    let row = ForwardRow::new(id, &blank, window, cx);
                                    editor.forwards.borrow_mut().push(row);
                                    cx.notify();
                                });
                            }
                        }),
                ),
        )
        // The note is its own line rather than the button row's tail: a note
        // long enough to meet the card edge reads as content leaking out of
        // the card, however honestly the truncation trims it.
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(crate::i18n::t(
                    "监听固定为 127.0.0.1，动态规则不需要目标。",
                    "The listener is 127.0.0.1; a dynamic rule needs no target.",
                )),
        )
        .when(empty, |this| {
            this.child(div().text_xs().text_color(muted).child(crate::i18n::t(
                "还没有转发规则。会话连上后它们会一起启动。",
                "No forwarding rules yet. They start with the session.",
            )))
        })
        .children(list)
        .into_any_element()
}

/// The word for a rule's kind.
fn kind_word(kind: &str) -> &'static str {
    match kind {
        "remote" => crate::i18n::t("远程", "Remote"),
        "dynamic" => crate::i18n::t("动态", "Dynamic"),
        _ => crate::i18n::t("本地", "Local"),
    }
}

/// The trigger table, as a settings field draws it: what to watch for, and what to answer.
///
/// A free function for the reason `forwards_element` is one — a field's closure is handed
/// an `App`, not the editor's context — and built the same way: the rows come in through
/// the handle they share with the editor, and every change goes through the editor's
/// entity, which is safe in a handler and not during a draw.
fn triggers_element(
    rows: Rc<RefCell<Vec<TriggerRow>>>,
    editor: gpui_kit::WeakEntity<SessionEditor>,
    border: Hsla,
    muted: Hsla,
) -> AnyElement {
    let mut list: Vec<AnyElement> = Vec::new();
    for row in rows.borrow().iter() {
        let id = row.id;
        let append_enter = row.append_enter;
        let repeat = row.repeat;
        let for_enter = editor.clone();
        let for_repeat = editor.clone();
        let for_remove = editor.clone();
        list.push(
            h_flex()
                .w_full()
                .min_w_0()
                .flex_wrap()
                .gap_2()
                .items_center()
                // The watch field and the answer field are one unit each —
                // relative basis, floor — and the flags travel with the
                // answer: a wide pane lays the row out flat, a narrow one
                // moves the answer and its flags to a second line whole.
                .child(
                    h_flex()
                        .w(relative(0.5))
                        .min_w(px(200.))
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().min_w(px(110.)).child(Input::new(&row.expect)))
                        .child(Icon::new(IconName::ArrowRight).size_3().text_color(muted)),
                )
                .child(
                    h_flex()
                        .w(relative(0.5))
                        .min_w(px(240.))
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().min_w(px(110.)).child(Input::new(&row.response))),
                )
                .child(
                    // Two flags rather than a menu: each is a sentence, and a switch that
                    // says what it does is one click from either state.
                    Button::new(SharedString::from(format!("trigger-enter-{id}")))
                        .icon(Icon::new(if append_enter {
                            IconName::SquareCheck
                        } else {
                            IconName::Square
                        }))
                        .label(crate::i18n::t("回车", "Enter"))
                        .small()
                        .ghost()
                        .flex_shrink_0()
                        .tooltip(crate::i18n::t(
                            "回答之后补一个回车，提示符等待的就是它。",
                            "Send Enter after the answer, which is what a prompt waits for.",
                        ))
                        .on_click(move |_, _, cx| {
                            if let Some(editor) = for_enter.upgrade() {
                                let _ = editor.update(cx, |editor, cx| {
                                    if let Some(row) = editor
                                        .triggers
                                        .borrow_mut()
                                        .iter_mut()
                                        .find(|row| row.id == id)
                                    {
                                        row.append_enter = !row.append_enter;
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    Button::new(SharedString::from(format!("trigger-repeat-{id}")))
                        .icon(Icon::new(if repeat {
                            IconName::SquareCheck
                        } else {
                            IconName::Square
                        }))
                        .label(crate::i18n::t("重复", "Repeat"))
                        .small()
                        .ghost()
                        .flex_shrink_0()
                        .tooltip(crate::i18n::t(
                            "每次出现都回答；关闭则整个会话只回答一次。",
                            "Answer every time it appears; off answers once per session.",
                        ))
                        .on_click(move |_, _, cx| {
                            if let Some(editor) = for_repeat.upgrade() {
                                let _ = editor.update(cx, |editor, cx| {
                                    if let Some(row) = editor
                                        .triggers
                                        .borrow_mut()
                                        .iter_mut()
                                        .find(|row| row.id == id)
                                    {
                                        row.repeat = !row.repeat;
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    Button::new(SharedString::from(format!("trigger-remove-{id}")))
                        .icon(IconName::Trash)
                        .ghost()
                        .small()
                        .tooltip(crate::i18n::t("删除这条规则", "Remove this rule"))
                        .accessibility_label(crate::i18n::t("删除这条规则", "Remove this rule"))
                        .on_click(move |_, _, cx| {
                            if let Some(editor) = for_remove.upgrade() {
                                let _ = editor.update(cx, |editor, cx| {
                                    editor.triggers.borrow_mut().retain(|row| row.id != id);
                                    cx.notify();
                                });
                            }
                        }),
                )
                .into_any_element(),
        );
    }

    let empty = list.is_empty();
    let for_add = editor;
    v_flex()
        .w_full()
        .min_w_0()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(border)
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(
                    Button::new("trigger-add")
                        .icon(IconName::Plus)
                        .label(crate::i18n::t("添加规则", "Add a rule"))
                        .small()
                        .outline()
                        .flex_shrink_0()
                        .on_click(move |_, window, cx| {
                            if let Some(editor) = for_add.upgrade() {
                                let _ = editor.update(cx, |editor, cx| {
                                    let id = editor.next_trigger_id;
                                    editor.next_trigger_id += 1;
                                    let blank = crate::core::TriggerDraft {
                                        append_enter: true,
                                        ..Default::default()
                                    };
                                    let row = TriggerRow::new(id, &blank, window, cx);
                                    editor.triggers.borrow_mut().push(row);
                                    cx.notify();
                                });
                            }
                        }),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(crate::i18n::t(
                    "输出里出现左侧文字时，自动回答右侧内容。",
                    "When the left text appears in the output, the right is sent.",
                )),
        )
        .when(empty, |this| {
            this.child(div().text_xs().text_color(muted).child(crate::i18n::t(
                "还没有自动应答规则。",
                "No automatic answers yet.",
            )))
        })
        .children(list)
        .into_any_element()
}

impl Render for SessionEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.seed_forwards(window, cx);
        self.seed_triggers(window, cx);
        self.seed_password(window, cx);
        let draft = self.draft.clone();
        let editor = cx.entity().downgrade();
        let theme = cx.theme();
        // Which fields apply is read here, once per frame, because the form is rebuilt
        // every frame and the kind can change while it is open. `SettingField::visible`
        // takes a plain bool set at build time, so a field that showed or hid itself
        // would be stuck on whatever the kind was when the page was first built.
        let kind_value = draft.borrow().kind.clone();
        let is_serial = kind_value == "serial";
        let is_ssh = kind_value == "ssh";

        let store_for_name = self.store.clone();
        let name = Self::text(&draft, |d| d.name.clone(), |d, v| d.name = v);
        let _ = store_for_name;

        let kind = Self::choice(
            &draft,
            editor.clone(),
            vec![
                (SharedString::from("ssh"), SharedString::from("SSH")),
                (SharedString::from("telnet"), SharedString::from("Telnet")),
                (SharedString::from("serial"), SharedString::from("Serial")),
            ],
            |d| d.kind.clone(),
            |d, v| d.kind = v,
        );

        let host = Self::text(&draft, |d| d.host.clone(), |d, v| d.host = v);
        let user = Self::text(&draft, |d| d.user.clone(), |d, v| d.user = v);
        let port = {
            let read = draft.clone();
            let write = draft.clone();
            // A port is a whole number, which is why this is a number field and not an
            // input: the only thing it can hold is a port.
            SettingField::number_input(
                Default::default(),
                move |_| f64::from(read.borrow().port),
                move |value, _| write.borrow_mut().port = value as i32,
            )
        };

        let auth = Self::choice(
            &draft,
            editor.clone(),
            vec![
                (
                    SharedString::from("password"),
                    crate::i18n::t("密码", "Password").into(),
                ),
                (
                    SharedString::from("key"),
                    crate::i18n::t("私钥", "Private key").into(),
                ),
                (
                    SharedString::from("keyboard-interactive"),
                    crate::i18n::t("键盘交互", "Keyboard interactive").into(),
                ),
            ],
            |d| d.auth.clone(),
            |d, v| d.auth = v,
        );

        // Masked: a password being typed is shoulder-surfable in plain form
        // (audit N-低1). The entity keeps the text; `sync_password` moves it
        // into the draft at save time.
        // An `element` field is rendered without any of the styling the framework
        // gives its own fields, so a bare `Input` here collapsed to its intrinsic
        // width and the row drew a stamp-sized box. Mirror what `gpui-component`'s
        // `StringField::render` does — same size, same width rule — so this row
        // lines up with every other one in the form.
        let password = {
            let input = self
                .password
                .clone()
                .expect("seed_password ran above while rendering");
            SettingField::element(
                move |options: &gpui_kit::component::setting::RenderOptions,
                      _: &mut Window,
                      _: &mut gpui_kit::App| {
                    Input::new(&input)
                        .disabled(options.is_disabled())
                        .with_size(options.size())
                        .map(|this| {
                            if options.layout().is_horizontal() {
                                this.w_64()
                            } else {
                                this.w_full()
                            }
                        })
                        .into_any_element()
                },
            )
        };
        let key_path = Self::text(
            &draft,
            |d| d.private_key_path.clone(),
            |d, v| d.private_key_path = v,
        );
        let proxy = Self::text(&draft, |d| d.proxy.clone(), |d, v| d.proxy = v);
        let note = Self::text(&draft, |d| d.note.clone(), |d, v| d.note = v);

        // Groups are a free-text field, not a dropdown, because the list of groups is
        // derived from the sessions in them: a dropdown would only offer the groups that
        // already exist, and a new session is often the first of its group.
        let group = Self::text(&draft, |d| d.group.clone(), |d, v| d.group = v);

        // ---- Serial-only fields, built whatever the kind is so a kind change mid-form
        // has values to show rather than empty ones. They are only *added* when the kind
        // is serial.
        let device = Self::text(&draft, |d| d.serial_port.clone(), |d, v| d.serial_port = v);
        let baud = {
            let read = draft.clone();
            let write = draft.clone();
            SettingField::number_input(
                Default::default(),
                move |_| f64::from(read.borrow().baud_rate),
                move |value, _| write.borrow_mut().baud_rate = value as i32,
            )
        };
        let data_bits = {
            let read = draft.clone();
            let write = draft.clone();
            SettingField::number_input(
                Default::default(),
                move |_| f64::from(read.borrow().data_bits),
                move |value, _| write.borrow_mut().data_bits = value as i32,
            )
        };
        let stop_bits = {
            let read = draft.clone();
            let write = draft.clone();
            SettingField::number_input(
                Default::default(),
                move |_| f64::from(read.borrow().stop_bits),
                move |value, _| write.borrow_mut().stop_bits = value as i32,
            )
        };
        let parity = Self::choice(
            &draft,
            editor.clone(),
            vec![
                (SharedString::from("none"), SharedString::from("None")),
                (SharedString::from("odd"), SharedString::from("Odd")),
                (SharedString::from("even"), SharedString::from("Even")),
            ],
            |d| d.parity.clone(),
            |d, v| d.parity = v,
        );
        let flow_control = Self::choice(
            &draft,
            editor.clone(),
            vec![
                (SharedString::from("none"), SharedString::from("None")),
                (
                    SharedString::from("hardware"),
                    SharedString::from("Hardware (RTS/CTS)"),
                ),
                (
                    SharedString::from("software"),
                    SharedString::from("Software (XON/XOFF)"),
                ),
            ],
            |d| d.flow_control.clone(),
            |d, v| d.flow_control = v,
        );

        let saving = {
            let read = draft.clone();
            let switch = draft.clone();
            SettingField::switch(
                move |_| read.borrow().disable_shell_integration,
                move |value, _| switch.borrow_mut().disable_shell_integration = value,
            )
        };
        let vt100 = {
            let read = draft.clone();
            let switch = draft.clone();
            SettingField::switch(
                move |_| read.borrow().vt100_drawing,
                move |value, _| switch.borrow_mut().vt100_drawing = value,
            )
        };

        let mut connection = SettingGroup::new()
            .title(crate::i18n::t("连接", "Connection"))
            .item(
                SettingItem::new(crate::i18n::t("名称", "Name"), name).description(crate::i18n::t(
                    "留空则按主机自动命名。",
                    "Left blank, named after the host.",
                )),
            )
            .item(SettingItem::new(crate::i18n::t("类型", "Type"), kind));

        // A serial session has no host, port or username — it has a device and a
        // framing — so showing those fields for one is a form asking for values it will
        // discard. The two sets are built per kind and only the matching one is added.
        if is_serial {
            connection = connection
                .item(SettingItem::new(
                    crate::i18n::t("串口设备", "Serial device"),
                    device,
                ))
                .item(SettingItem::new(
                    crate::i18n::t("波特率", "Baud rate"),
                    baud,
                ))
                .item(SettingItem::new(
                    crate::i18n::t("数据位", "Data bits"),
                    data_bits,
                ))
                .item(SettingItem::new(
                    crate::i18n::t("停止位", "Stop bits"),
                    stop_bits,
                ))
                .item(SettingItem::new(crate::i18n::t("校验", "Parity"), parity))
                .item(SettingItem::new(
                    crate::i18n::t("流控", "Flow control"),
                    flow_control,
                ));
        } else {
            connection = connection
                .item(SettingItem::new(crate::i18n::t("主机", "Host"), host))
                .item(SettingItem::new(crate::i18n::t("端口", "Port"), port))
                .item(SettingItem::new(crate::i18n::t("用户名", "Username"), user));
        }
        connection = connection.item(SettingItem::new(crate::i18n::t("分组", "Group"), group));

        // Authentication is an SSH idea: telnet and serial have neither a password
        // prompt nor a key, and offering one would be a field that writes a value
        // nothing reads.
        let mut credentials = SettingGroup::new().title(crate::i18n::t("认证", "Authentication"));
        if is_ssh {
            credentials = credentials
                .item(SettingItem::new(crate::i18n::t("方式", "Method"), auth))
                .item(
                    SettingItem::new(crate::i18n::t("密码", "Password"), password).description(
                        crate::i18n::t(
                            "留空表示保留已保存的密码，密码不会被读回界面。",
                            "Left blank, the saved password is kept. A password is never read \
                             back into a form.",
                        ),
                    ),
                )
                .item(
                    SettingItem::new(crate::i18n::t("私钥路径", "Private key path"), key_path)
                        .description(crate::i18n::t(
                            "使用私钥认证时填写。",
                            "Used when authenticating with a key.",
                        )),
                );
        } else {
            // Telnet: a password can be needed, a key cannot.
            credentials = credentials.item(
                SettingItem::new(crate::i18n::t("密码", "Password"), password).description(
                    crate::i18n::t(
                        "留空表示保留已保存的密码。",
                        "Left blank, the saved password is kept.",
                    ),
                ),
            );
        }

        let advanced = SettingGroup::new()
            .title(crate::i18n::t("高级", "Advanced"))
            .item(
                SettingItem::new(crate::i18n::t("代理", "Proxy"), proxy).description(
                    crate::i18n::t(
                        "例如 socks5://127.0.0.1:1080，留空则直连。",
                        "For example socks5://127.0.0.1:1080. Blank connects directly.",
                    ),
                ),
            )
            .item(SettingItem::new(crate::i18n::t("备注", "Note"), note))
            .item(
                SettingItem::new(
                    crate::i18n::t("跳过 Shell 集成", "Skip shell integration"),
                    saving,
                )
                .description(crate::i18n::t(
                    "Windows / pwsh 服务器上会破坏 shell，故跳过。",
                    "Skips it on Windows / pwsh servers, where it breaks the shell.",
                )),
            )
            .item(
                SettingItem::new(crate::i18n::t("VT100 制表符", "VT100 line drawing"), vt100)
                    .description(crate::i18n::t(
                        "UTF-8 下也启用 DEC 制表符。",
                        "Honours DEC Special Graphics even in UTF-8 mode.",
                    )),
            );

        // Port forwarding is an SSH idea: telnet has no channel to forward over and a
        // serial line has no remote end. The table is the settings framework's field for
        // this group, which is why it is built from a shared handle rather than from
        // `self`: a field's closure receives an `App`, not this view's context, and
        // reaching back into the view while it is drawing would be a render inside a
        // render.
        let forwards_field = {
            let rows = self.forwards.clone();
            let editor = cx.entity().downgrade();
            SettingField::element(
                move |_: &gpui_kit::component::setting::RenderOptions,
                      _: &mut Window,
                      cx: &mut gpui_kit::App| {
                    let theme = cx.theme();
                    forwards_element(
                        rows.clone(),
                        editor.clone(),
                        theme.border,
                        theme.muted_foreground,
                    )
                },
            )
        };

        // Its own page, rather than a fourth group on the session page: the table is as
        // wide as five fields and grows a row at a time, and a group that pushes the
        // connection fields off the bottom is a form that hides what it is for.
        let forwards_page = SettingPage::new(crate::i18n::t("端口转发", "Port forwarding")).group(
            SettingGroup::new().item(
                SettingItem::new(
                    crate::i18n::t("转发规则", "Forwarding rules"),
                    forwards_field,
                )
                .description(crate::i18n::t(
                    "会话建立后这些规则会一起启动。",
                    "These rules start with the session.",
                )),
            ),
        );

        // The triggers, on their own page for the same reason and in the same shape: what
        // to watch for and what to answer is a table, and a table belongs on a page with
        // room for it.
        let triggers_field = {
            let rows = self.triggers.clone();
            let editor = cx.entity().downgrade();
            SettingField::element(
                move |_: &gpui_kit::component::setting::RenderOptions,
                      _: &mut Window,
                      cx: &mut gpui_kit::App| {
                    let theme = cx.theme();
                    triggers_element(
                        rows.clone(),
                        editor.clone(),
                        theme.border,
                        theme.muted_foreground,
                    )
                },
            )
        };
        let triggers_page = SettingPage::new(crate::i18n::t("自动应答", "Automatic answers"))
            .group(
                SettingGroup::new().item(
                    SettingItem::new(crate::i18n::t("应答规则", "Answer rules"), triggers_field)
                        .description(crate::i18n::t(
                            "用于回答每次都要输入的提示，例如登录后的二次确认。",
                            "For prompts that always ask the same thing, such as a confirmation \
                         after login.",
                        )),
                ),
            );

        let cancel = Button::new("editor-cancel")
            .icon(IconName::X)
            .label(crate::i18n::t("取消", "Cancel"))
            .ghost()
            .on_click(cx.listener(|this, _, _, cx| {
                this.outcome = Some(EditorOutcome::Cancelled);
                cx.notify();
            }));

        let save = Button::new("editor-save")
            .icon(IconName::Check)
            .label(crate::i18n::t("保存", "Save"))
            .primary()
            .on_click(cx.listener(|this, _, _, cx| {
                this.save(cx);
                cx.notify();
            }));

        v_flex()
            .size_full()
            .bg(theme.background)
            // No header of its own. The overlay that holds this dialog already draws one,
            // and it reads this editor's `heading()`, so a second title inside the form was
            // the same sentence twice — the footer is the only part of the frame this dialog
            // needs to draw itself.
            .child(
                // A settings page rather than a bare form: it already has the label /
                // description / control layout, the section headings and the scrolling,
                // and a hand-rolled column would be a second version of all three.
                div().flex_1().min_h_0().overflow_hidden().child(
                    Settings::new("session-editor")
                        .page(
                            SettingPage::new(crate::i18n::t("会话", "Session"))
                                .group(connection)
                                .group(credentials)
                                .group(advanced),
                        )
                        .when(is_ssh, |settings| {
                            settings.page(forwards_page).page(triggers_page)
                        })
                        .into_any_element(),
                ),
            )
            .child(
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .justify_end()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(cancel)
                    .child(save),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::PortForwardDraft;
    use gpui_kit::gpui::TestAppContext;

    fn session_with_one_forward() -> Session {
        let mut draft = SessionDraft::new_ssh();
        draft.host = "example.com".into();
        draft.user = "root".into();
        draft.forwards = vec![PortForwardDraft {
            kind: "local".into(),
            name: "web".into(),
            bind_addr: "127.0.0.1".into(),
            bind_port: "8080".into(),
            host: "10.0.0.5".into(),
            host_port: "80".into(),
        }];
        draft.to_session(None)
    }

    /// The rows a session's rules arrive as, and the rules that leave when the rows do.
    ///
    /// This is round 31's bug at the level the user meets it. That round fixed the *data*
    /// path — a forwarding rule survives a trip through a form, and clearing the rows clears
    /// the rules — and both have unit tests in `core`. What was never tested is the view's
    /// half: that opening an editor on a session with a rule gives the form a row for it, and
    /// that removing every row leaves the draft with no rules rather than the ones it started
    /// with. The second is exactly the failure the round-31 fix was written for, and it is one
    /// guard in `sync_forwards` away from happening again.
    #[gpui_kit::gpui::test]
    fn the_forward_rows_follow_the_session_and_the_draft(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let store = Rc::new(RefCell::new(
            crate::config::ConfigStore::load().expect("the configuration this machine has"),
        ));
        let session = session_with_one_forward();
        let (view, cx) = cx.add_window_view(move |_, _| SessionEditor::edit(store, session));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        let rows = view.read_with(cx, |editor, _| editor.forwards.borrow().len());
        assert_eq!(rows, 1, "the session's rule arrives as one row");

        // Remove it the way the row's own button does, then let the form sync.
        view.update(cx, |editor, cx| {
            editor.forwards.borrow_mut().clear();
            editor.sync_forwards(cx);
        });
        let left = view.read_with(cx, |editor, _| editor.draft.borrow().forwards.len());
        assert_eq!(
            left, 0,
            "clearing the rows clears the rules, which is the bug round 31 fixed"
        );
    }
}
