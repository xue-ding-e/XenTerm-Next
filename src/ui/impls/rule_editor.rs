//! The rule editor: one custom output-highlighting rule, and why it cannot be saved.
//!
//! ## What it edits, and what it does not
//!
//! A rule is a pattern, three flags and a colour. There is no *editing* of an existing
//! rule — neither frontend has ever had it: a rule is added, enabled, disabled or
//! removed, and changing one means removing it and typing a better one. The manager lists
//! them with their switches for the same reason.
//!
//! ## Why the refusal is on screen
//!
//! `crate::core::highlight` decides whether a rule is valid, and its message is the
//! interface: an invalid regular expression would otherwise be saved as a rule that
//! silently never fires, which looks exactly like a pattern that does not match. The
//! dialog stays open with the reason under the field.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputState},
        switch::Switch,
        v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    },
    div,
    prelude::*,
    AnyElement, Context, Entity, FontWeight, IntoElement, Render, SharedString, Subscription,
    Window,
};

// The full Lucide catalog rather than the component library's curated subset.
use gpui_kit::assets::IconName;

use crate::config::{ConfigStore, OutputHighlightRule};

/// The colours a rule can paint with.
///
/// A fixed palette rather than a colour picker: the terminal's own colours are a set of
/// named ANSI slots, and a rule that asked for an arbitrary RGB would have to be mapped
/// back into one anyway.
const COLORS: [(&str, &str); 6] = [
    ("red", "红"),
    ("yellow", "黄"),
    ("green", "绿"),
    ("cyan", "青"),
    ("magenta", "紫"),
    ("gray", "灰"),
];

/// What the editor wants the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RuleEditorAction {
    /// The rule was saved; the settings page and the terminals pick it up from the store.
    Saved,
}

/// The rule editor.
pub(crate) struct RuleEditorView {
    store: Rc<RefCell<ConfigStore>>,
    pattern: Entity<InputState>,
    regex: bool,
    case_sensitive: bool,
    whole_line: bool,
    color: String,
    /// Why the last save was refused, shown under the field until it is fixed.
    error: Option<String>,
    pending: Option<RuleEditorAction>,
    _subscription: Subscription,
}

impl RuleEditorView {
    pub(crate) fn new(
        store: Rc<RefCell<ConfigStore>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pattern = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::t(
                "关键词或正则表达式",
                "Keyword or regular expression",
            ))
        });
        // The error is cleared as soon as the text changes: a message about the previous
        // pattern sitting under a new one is a message about nothing.
        let subscription = cx.subscribe_in(
            &pattern,
            window,
            |view: &mut Self, _, _: &gpui_kit::component::input::InputEvent, _, cx| {
                view.error = None;
                cx.notify();
            },
        );

        Self {
            store,
            pattern,
            regex: false,
            case_sensitive: false,
            whole_line: false,
            color: COLORS[0].0.to_string(),
            error: None,
            pending: None,
            _subscription: subscription,
        }
    }

    /// Take the next action, if any.
    pub(crate) fn take_action(&mut self) -> Option<RuleEditorAction> {
        self.pending.take()
    }

    /// Validate and save the rule.
    fn save(&mut self, cx: &mut Context<Self>) {
        let pattern = self.pattern.read(cx).value().trim().to_string();
        if let Err(message) =
            crate::core::highlight::validate(&pattern, self.regex, self.case_sensitive)
        {
            self.error = Some(message);
            cx.notify();
            return;
        }
        {
            let mut store = self.store.borrow_mut();
            if store.output_highlight_rules().len() >= crate::core::highlight::MAX_RULES {
                self.error = Some(
                    crate::i18n::t("自定义规则最多 128 条", "Custom rules are limited to 128")
                        .to_string(),
                );
                cx.notify();
                return;
            }
            store.add_output_highlight_rule(OutputHighlightRule {
                pattern,
                regex: self.regex,
                case_sensitive: self.case_sensitive,
                whole_line: self.whole_line,
                color: self.color.clone(),
                enabled: true,
            });
            if let Err(error) = store.save() {
                tracing::warn!("could not save the output highlighting rule: {error:#}");
            }
        }
        self.pending = Some(RuleEditorAction::Saved);
        cx.notify();
    }

    /// One switch with its label: the flags are what a pattern *means*, so they read as
    /// sentences rather than as three unlabelled toggles.
    fn flag(
        &self,
        id: &'static str,
        label: SharedString,
        value: bool,
        set: fn(&mut Self, bool),
        cx: &Context<Self>,
    ) -> AnyElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(Switch::new(id).checked(value).on_click(cx.listener(
                move |this, checked, _, cx| {
                    set(this, *checked);
                    cx.notify();
                },
            )))
            .child(div().text_sm().child(label))
            .into_any_element()
    }
}

impl Render for RuleEditorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let can_save = !self.pattern.read(cx).value().trim().is_empty();
        let color = self.color.clone();

        v_flex().size_full().bg(theme.background).child(
            v_flex()
                .id("rule-editor")
                .size_full()
                .overflow_y_scroll()
                .gap_3()
                .p_3()
                .child(
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
                                .child(crate::i18n::t("新增高亮规则", "New highlight rule")),
                        )
                        .child(Input::new(&self.pattern))
                        .when_some(self.error.clone(), |this, error| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.danger)
                                    .child(SharedString::from(error)),
                            )
                        })
                        .child(h_flex().w_full().gap_4().children(vec![
                            self.flag(
                                "rule-regex",
                                crate::i18n::t("正则表达式", "Regular expression").into(),
                                self.regex,
                                |view, value| view.regex = value,
                                cx,
                            ),
                            self.flag(
                                "rule-case",
                                crate::i18n::t("区分大小写", "Case sensitive").into(),
                                self.case_sensitive,
                                |view, value| view.case_sensitive = value,
                                cx,
                            ),
                            self.flag(
                                "rule-whole-line",
                                crate::i18n::t("整行匹配", "Whole line").into(),
                                self.whole_line,
                                |view, value| view.whole_line = value,
                                cx,
                            ),
                        ]))
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .items_center()
                                .child(div().text_sm().child(crate::i18n::t("颜色", "Colour")))
                                .children(COLORS.iter().map(|(id, label)| {
                                    let chosen = *id == color;
                                    let target = (*id).to_string();
                                    Button::new(SharedString::from(format!("rule-color-{id}")))
                                        .label(SharedString::from(crate::i18n::t(label, id)))
                                        .small()
                                        .when(chosen, |this| this.primary())
                                        .when(!chosen, |this| this.ghost())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.color = target.clone();
                                            cx.notify();
                                        }))
                                })),
                        )
                        .child(
                            h_flex().w_full().justify_end().gap_2().child(
                                Button::new("rule-save")
                                    .icon(Icon::new(IconName::Check))
                                    .label(crate::i18n::t("添加", "Add"))
                                    .primary()
                                    .small()
                                    .disabled(!can_save)
                                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                            ),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(crate::i18n::t(
                            "规则只影响显示,终端输出的原始文本不会改变。",
                            "Rules affect what is drawn; the terminal's own text is unchanged.",
                        )),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::TestAppContext;

    /// A pattern the validator refuses adds nothing — not a rule, not a rule-and-a-half.
    ///
    /// `save` writes the configuration on the path where the pattern is *accepted*, so this
    /// test drives the path where it is refused: an unclosed character class with the regex
    /// flag on, which `core::highlight::validate` rejects. The assertion that matters is the
    /// store's rule count, unchanged — a validator that refuses but leaves a rule behind is
    /// worse than one that accepts, because the error it shows says the opposite.
    ///
    /// The store is loaded and read, never written: the refused path returns before `save`.
    #[gpui_kit::gpui::test]
    fn a_refused_pattern_adds_no_rule(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let store = Rc::new(RefCell::new(
            crate::config::ConfigStore::load().expect("the configuration this machine has"),
        ));
        let before = store.borrow().output_highlight_rules().len();

        let (view, cx) = cx.add_window_view({
            let store = store.clone();
            move |window, cx| RuleEditorView::new(store, window, cx)
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        cx.update(|window, cx| {
            view.update(cx, |editor, cx| {
                editor.regex = true;
                editor.pattern.clone().update(cx, |state, cx| {
                    state.set_value(gpui_kit::SharedString::from("["), window, cx);
                });
                editor.save(cx);
            });
        });

        let (error, pending) = view.read_with(cx, |editor, _| {
            (editor.error.clone(), editor.pending.is_some())
        });
        assert!(
            error.is_some(),
            "an unclosed character class is refused, and says why"
        );
        assert!(!pending, "and nothing is reported as saved");
        assert_eq!(
            store.borrow().output_highlight_rules().len(),
            before,
            "and no rule was added on the way out"
        );
    }
}
