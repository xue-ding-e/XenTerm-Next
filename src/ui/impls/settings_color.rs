//! GPUI picker state, palette primitives and popup edit the same colour draft.
//! The continuous HSV surface is the separately attributed ColorArea adapter.
//! Only Apply/Enter writes the profile. A popup closing is not an apply action.

use super::super::color_area::{HsvAreaEvent, HsvAreaState};
use super::*;
use crate::config::color::{ColorFormat, ColorValue};
use gpui_kit::base::{ColorPicker as PickerRoot, ColorSwatch};
use gpui_kit::component::{
    button::DropdownButton,
    color_picker::{ColorPickerEvent, ColorPickerState},
    menu::PopupMenuItem,
    popover::Popover,
    tab::{Tab, TabBar},
    theme,
};
use gpui_kit::{App, Focusable as _, Hsla};

pub(super) struct CursorColorEditor {
    pub(super) picker: Entity<ColorPickerState>,
    pub(super) format: ColorFormat,
    pub(super) area: Entity<HsvAreaState>,
    pub(super) recent: Vec<ColorValue>,
    _subscription: Subscription,
    _area_subscription: Subscription,
}

fn picker_color(color: ColorValue) -> Hsla {
    let [r, g, b, a] = color.to_rgba8();
    super::super::terminal::rgba_to_hsla(crate::terminal::Rgba { r, g, b, a })
}

fn picked_value(color: Hsla) -> ColorValue {
    let value = csscolorparser::Color::from_hsla(color.h * 360., color.s, color.l, color.a);
    ColorValue::from_rgba8(value.to_rgba8())
}

fn format_label(format: ColorFormat) -> &'static str {
    match format {
        ColorFormat::Hex => "HEX",
        ColorFormat::Rgb => "RGB / RGBA",
        ColorFormat::Hsl => "HSL / HSLA",
        ColorFormat::Hsv => "HSV / HSVA",
        ColorFormat::Cmyk => crate::i18n::t("CMYK（近似）", "CMYK (approx.)"),
    }
}

fn cursor_preview(
    label: &'static str,
    style: usize,
    colour: Hsla,
    cx: &App,
) -> gpui_kit::AnyElement {
    let painted = if style == 0 {
        colour.opacity(0.7)
    } else {
        colour
    };
    v_flex()
        .gap_1()
        .items_center()
        .child(
            div()
                .relative()
                .w_10()
                .h_7()
                .border_1()
                .border_color(cx.theme().border)
                .bg(super::super::terminal::rgba_to_hsla(
                    crate::terminal::terminal_background(true),
                ))
                .text_color(gpui_kit::white())
                .child(div().absolute().left(px(13.)).top(px(3.)).child("A"))
                .child(
                    div()
                        .absolute()
                        .left(px(12.))
                        .top(if style == 2 { px(20.) } else { px(4.) })
                        .w(if style == 1 { px(2.) } else { px(12.) })
                        .h(if style == 2 { px(2.) } else { px(18.) })
                        .bg(painted),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .into_any_element()
}

// Presentation only: the palette, popover, tabs and swatches remain GPUI Kit
// primitives; HSV/alpha editing is delegated to the attributed ColorArea adapter.
fn preset_rows() -> Vec<Vec<Hsla>> {
    // Public Kit palette accessors keep the existing colour family and shades
    // without copying its private palette data or adding another palette crate.
    let rows: [fn(usize) -> Hsla; 9] = [
        theme::stone,
        theme::red,
        theme::orange,
        theme::yellow,
        theme::green,
        theme::cyan,
        theme::blue,
        theme::purple,
        theme::pink,
    ];
    let levels = [950, 900, 800, 700, 600, 500, 400, 300, 200, 100, 50];
    rows.into_iter()
        .map(|row| levels.into_iter().map(row).collect())
        .collect()
}

fn preset_swatch(
    id: String,
    colour: Hsla,
    picker: &Entity<ColorPickerState>,
    cx: &App,
) -> ColorSwatch {
    let current = picker.read(cx).value().map(picked_value);
    let selected = current == Some(picked_value(colour));
    let ring = cx.theme().ring;
    let choose = picker.clone();
    let preview = picker.clone();
    let selector = id.clone();
    ColorSwatch::new(SharedString::from(id), colour)
        .debug_selector(move || selector.clone())
        .selected(selected)
        .accessibility_label(picked_value(colour).canonical_hex())
        .size(px(20.))
        .bg(colour)
        .rounded(px(3.))
        .border(if selected { px(2.) } else { px(1.) })
        .border_color(if selected {
            cx.theme().ring
        } else {
            cx.theme().border
        })
        .hover(move |this| this.border_color(ring))
        .on_hover(move |color, entered, window, cx| {
            if entered {
                preview.update(cx, |state, cx| state.preview_color(color, window, cx));
            }
        })
        .on_click(move |color, _, window, cx| {
            choose.update(cx, |state, cx| state.select_color(color, window, cx));
        })
}

fn render_cursor_picker(
    picker: &Entity<ColorPickerState>,
    area: &Entity<HsvAreaState>,
    recent: &[ColorValue],
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::AnyElement {
    let state = picker.read(cx);
    let open = state.is_open();
    let tab = state.active_tab().min(1);
    let colour = state.value().unwrap_or(gpui_kit::white());
    let hex = state.hex_input().clone();
    let focus = picker.focus_handle(cx);
    let tab_picker = picker.clone();
    let tab_area = area.clone();
    let popup_picker = picker.clone();
    let popup_area = area.clone();
    let root_picker = picker.clone();
    let root_area = area.clone();
    let mut content = v_flex()
        .id("cursor-colour-popup-content")
        .debug_selector(|| "cursor-colour-popup-content".to_string())
        .w(px(292.))
        .max_h((window.viewport_size().height - px(72.)).max(px(180.)))
        .overflow_y_scroll()
        .gap_3()
        .p_2()
        .child(
            div()
                .debug_selector(|| "cursor-colour-mode-tabs".to_string())
                .w_full()
                .child(
                    TabBar::new("cursor-colour-tabs")
                        .w_full()
                        .segmented()
                        .selected_index(tab)
                        .child(
                            Tab::new()
                                .flex_1()
                                .debug_selector(|| "cursor-colour-tab-presets".to_string())
                                .label(crate::i18n::t("预设色板", "Presets")),
                        )
                        .child(
                            Tab::new()
                                .flex_1()
                                .debug_selector(|| "cursor-colour-tab-continuous".to_string())
                                .label(crate::i18n::t("连续调色", "Continuous")),
                        )
                        .on_click(move |index: &usize, window, cx| {
                            // Both entities survive tab changes. Move focus before hiding
                            // either body, so Escape still belongs to the visible popup.
                            if *index == 1 {
                                window.focus(&tab_area.focus_handle(cx), cx);
                            } else {
                                tab_area.update(cx, |state, cx| state.cancel_drag(cx));
                                window.focus(&tab_picker.focus_handle(cx), cx);
                            }
                            tab_picker.update(cx, |state, cx| state.set_active_tab(*index, cx));
                        }),
                ),
        );
    if tab == 0 {
        let mut palette = v_flex().gap_1();
        if !recent.is_empty() {
            palette = palette
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::i18n::t(
                            "最近应用（本窗口）",
                            "Recently applied (this window)",
                        )),
                )
                .child(h_flex().gap_1().children(recent.iter().enumerate().map(
                    |(index, color)| {
                        preset_swatch(
                            format!("cursor-recent-{index}"),
                            picker_color(*color),
                            picker,
                            cx,
                        )
                    },
                )))
                .child(div().h_1());
        }
        for (row, colours) in preset_rows().into_iter().enumerate() {
            palette = palette.child(h_flex().gap_1().children(
                colours.into_iter().enumerate().map(|(column, color)| {
                    preset_swatch(format!("cursor-preset-{row}-{column}"), color, picker, cx)
                }),
            ));
        }
        content = content.child(palette);
    } else {
        content = content.child(area.clone());
    }
    content = content
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(crate::i18n::t(
                    "选色仅更改草稿；在设置行应用或取消。",
                    "Picking changes the draft; apply or cancel in the settings row.",
                )),
        )
        .child(Input::new(&hex).small().w_full());
    PickerRoot::new(("cursor-picker", picker.entity_id()))
        .open(open)
        .track_focus(&focus)
        .accessibility_label(crate::i18n::t("光标调色板", "Cursor colour palette"))
        .on_open_change(move |open, _, cx| {
            if !open {
                root_area.update(cx, |state, cx| state.cancel_drag(cx));
            }
            root_picker.update(cx, |state, cx| state.set_open(open, cx))
        })
        .child(
            Popover::new("cursor-colour-popup")
                .open(open)
                .on_open_change(move |open, _, cx| {
                    if !*open {
                        popup_area.update(cx, |state, cx| state.cancel_drag(cx));
                    }
                    popup_picker.update(cx, |state, cx| state.set_open(*open, cx))
                })
                .trigger(
                    Button::new("cursor-colour-trigger")
                        .outline()
                        .label(crate::i18n::t("调色板", "Palette"))
                        .child(
                            div()
                                .size(px(16.))
                                .rounded(px(2.))
                                .border_1()
                                .border_color(cx.theme().input)
                                .bg(colour),
                        ),
                )
                .child(content),
        )
        .into_any_element()
}

impl SettingsView {
    pub(super) fn remember_applied_cursor_color(&mut self) {
        let Some(editor) = self.color_editor.as_mut() else {
            return;
        };
        let saved = self.store.borrow().terminal_cursor_color().to_string();
        let Ok(color) = ColorValue::parse(&saved) else {
            return;
        };
        editor.recent.retain(|previous| *previous != color);
        editor.recent.insert(0, color);
        editor.recent.truncate(8);
    }

    /// Reflect valid raw text in the picker without replacing partial text or
    /// emitting a picker event back into the field.
    pub(super) fn sync_cursor_picker(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.color_editor.as_ref() else {
            return;
        };
        let Some(draft) = self.text_drafts.get(&TextSetting::CursorColor) else {
            return;
        };
        let raw = draft.input.read(cx).value();
        if raw.trim().is_empty() {
            // Empty is an application default, whose resolved cursor is white.
            // ColorPicker::clear_value leaves stale (initially zero-alpha) sliders,
            // so show the resolved colour while the outer text keeps its sentinel.
            editor.picker.update(cx, |state, cx| {
                state.set_value(gpui_kit::white(), window, cx)
            });
            editor
                .area
                .update(cx, |state, cx| state.reset_default(window, cx));
        } else if let Ok(color) = ColorValue::parse(&raw) {
            editor.area.update(cx, |state, cx| {
                state.set_color(picker_color(color), window, cx)
            });
            editor.picker.update(cx, |state, cx| {
                state.set_value(picker_color(color), window, cx);
                // Toolkit 0.6.1 truncates its internal HEX formatter. Keep the
                // popup's editable HEX value on our nearest-byte canonical value,
                // so opening it and pressing Enter cannot subtract a byte.
                let input = state.hex_input().clone();
                input.update(cx, |input, cx| {
                    input.set_value(color.canonical_hex(), window, cx)
                });
            });
        }
    }

    fn replace_cursor_text(
        &mut self,
        raw: String,
        dirty: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = self
            .text_drafts
            .get_mut(&TextSetting::CursorColor)
            .expect("colour field initialized");
        draft.last_value = raw.clone();
        draft.dirty = dirty;
        draft.error = None;
        draft
            .input
            .update(cx, |input, cx| input.set_value(raw, window, cx));
        cx.notify();
    }

    fn set_cursor_draft(
        &mut self,
        raw: String,
        dirty: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_cursor_text(raw, dirty, window, cx);
        self.sync_cursor_picker(window, cx);
    }

    fn restore_cursor_color(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.store.borrow().terminal_cursor_color().to_string();
        let format = self.color_editor.as_ref().expect("colour editor").format;
        let raw = ColorValue::parse(&saved)
            .map(|c| c.format(format))
            .unwrap_or_default();
        self.set_cursor_draft(raw, false, window, cx);
        self.color_editor
            .as_ref()
            .unwrap()
            .picker
            .update(cx, |picker, cx| picker.set_open(false, cx));
    }

    pub(super) fn change_cursor_format(
        &mut self,
        format: ColorFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = self
            .text_drafts
            .get(&TextSetting::CursorColor)
            .expect("colour draft");
        let raw = draft.input.read(cx).value().to_string();
        let dirty = draft.dirty;
        let converted = if raw.trim().is_empty() {
            Some(String::new())
        } else {
            ColorValue::parse(&raw).ok().map(|c| c.format(format))
        };
        let Some(converted) = converted else {
            self.text_drafts
                .get_mut(&TextSetting::CursorColor)
                .unwrap()
                .error = Some(crate::i18n::t(
                "请先修正颜色，再转换格式。原输入已保留。",
                "Correct the colour before converting its format. Your input is kept.",
            ));
            cx.notify();
            return;
        };
        self.color_editor.as_mut().unwrap().format = format;
        self.set_cursor_draft(converted, dirty, window, cx);
    }

    pub(super) fn cursor_color_field(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SettingField<SharedString> {
        // Register the shared raw-input subscription and persistence error handling.
        let _ = self.text_field(TextSetting::CursorColor, window, cx);
        if self.color_editor.is_none() {
            let picker = cx.new(|cx| ColorPickerState::new(window, cx));
            let area = cx.new(|cx| HsvAreaState::new(window, cx));
            let area_subscription = cx.subscribe_in(
                &area,
                window,
                |view, _, event: &HsvAreaEvent, window, cx| {
                    if let HsvAreaEvent::Change { rgba, .. } = event {
                        // The event carries the bytes from this exact pointer step.
                        // Avoid an HSV -> HSL -> RGB echo that can quantize a boundary
                        // twice or replace the area's latent hue while dragging.
                        let value = ColorValue::from_rgba8(*rgba);
                        let format = view.color_editor.as_ref().expect("colour editor").format;
                        view.replace_cursor_text(value.format(format), true, window, cx);
                        let picker = view
                            .color_editor
                            .as_ref()
                            .expect("colour editor")
                            .picker
                            .clone();
                        picker.update(cx, |state, cx| {
                            state.set_value(picker_color(value), window, cx);
                            let hex = state.hex_input().clone();
                            hex.update(cx, |input, cx| {
                                input.set_value(value.canonical_hex(), window, cx)
                            });
                        });
                    }
                },
            );
            let subscription = cx.subscribe_in(
                &picker,
                window,
                |view, picker, event: &ColorPickerEvent, window, cx| {
                    let ColorPickerEvent::Change(color) = event;
                    let format = view.color_editor.as_ref().expect("colour editor").format;
                    let raw = color
                        .map(|value| picked_value(value).format(format))
                        .unwrap_or_default();
                    // Keep the component's continuous HSLA state. A round trip
                    // through RGB would erase hue at saturation=0 or lightness=0/1.
                    view.replace_cursor_text(raw, true, window, cx);
                    let hex = picker.read(cx).hex_input().clone();
                    let canonical = color
                        .map(|c| picked_value(c).canonical_hex())
                        .unwrap_or_default();
                    hex.update(cx, |input, cx| input.set_value(canonical, window, cx));
                    if let Some(color) = color {
                        let area = view.color_editor.as_ref().unwrap().area.clone();
                        area.update(cx, |state, cx| state.set_color(*color, window, cx));
                    }
                },
            );
            self.color_editor = Some(CursorColorEditor {
                picker,
                format: ColorFormat::Hex,
                area,
                recent: Vec::new(),
                _subscription: subscription,
                _area_subscription: area_subscription,
            });
            self.sync_cursor_picker(window, cx);
        }
        // A clean field follows external profile changes. A dirty field survives
        // Font/Cursor navigation, retaining invalid text and its feedback.
        if !self.text_drafts[&TextSetting::CursorColor].dirty {
            let saved = self.store.borrow().terminal_cursor_color().to_string();
            let format = self.color_editor.as_ref().unwrap().format;
            let display = ColorValue::parse(&saved)
                .map(|c| c.format(format))
                .unwrap_or_default();
            if self.text_drafts[&TextSetting::CursorColor]
                .input
                .read(cx)
                .value()
                .as_ref()
                != display
            {
                self.set_cursor_draft(display, false, window, cx);
            }
        }
        let draft = &self.text_drafts[&TextSetting::CursorColor];
        let input = draft.input.clone();
        let dirty = draft.dirty;
        let error = draft.error;
        let editor = self.color_editor.as_ref().unwrap();
        let picker = editor.picker.clone();
        let area = editor.area.clone();
        let recent = editor.recent.clone();
        let format = editor.format;
        let placeholder = match format {
            ColorFormat::Hex => crate::i18n::t(
                "例如 #12345680；留空使用默认颜色",
                "For example #12345680; empty uses the default",
            ),
            ColorFormat::Rgb => "rgba(18, 52, 86, 0.5)",
            ColorFormat::Hsl => "hsla(210, 65%, 20%, 0.5)",
            ColorFormat::Hsv => "hsva(210, 79%, 34%, 0.5)",
            ColorFormat::Cmyk => "cmyk(0% 100% 100% 0% / 50%)",
        };
        if input.read(cx).presentation().placeholder().as_ref() != placeholder {
            input.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx)
            });
        }
        let view = cx.entity().downgrade();
        SettingField::element(
            move |options: &gpui_kit::component::setting::RenderOptions,
                  window: &mut Window,
                  cx: &mut App| {
                let menu_view = view.clone();
                let apply_view = view.clone();
                let cancel_view = view.clone();
                let default_view = view.clone();
                let apply_input = input.clone();
                v_flex()
                .id("settings-cursor-color-editor")
                .debug_selector(|| "settings-cursor-color-editor".to_string())
                .w_full().max_w(px(470.)).min_w_0().gap_2()
                .child(h_flex().gap_2().items_center().flex_wrap()
                    .child(div().debug_selector(|| "cursor-color-picker".to_string())
                        .child(render_cursor_picker(&picker, &area, &recent, window, cx)))
                    .child(DropdownButton::new("cursor-color-format")
                        .button(Button::new("cursor-color-format-trigger").outline()
                            .label(format_label(format)).with_size(options.size())
                            .accessibility_label(crate::i18n::t("颜色显示格式", "Colour display format"))
                            .debug_selector(|| "cursor-color-format".to_string()))
                        .dropdown_menu(move |mut menu, _, _| {
                            for choice in ColorFormat::ALL {
                                let target = menu_view.clone();
                                menu = menu.item(PopupMenuItem::new(format_label(choice)).checked(choice == format)
                                    .on_click(move |_, window, cx| {
                                        let _ = target.update(cx, |view, cx| view.change_cursor_format(choice, window, cx));
                                    }));
                            }
                            menu
                        })))
                .child(div().debug_selector(|| "cursor-color-input".to_string()).w_full().min_w_0()
                    .child(Input::new(&input).w_full().with_size(options.size())))
                .child(h_flex().gap_2().flex_wrap()
                    .child(Button::new("cursor-color-apply").primary().label(crate::i18n::t("应用", "Apply"))
                        .debug_selector(|| "cursor-color-apply".to_string()).disabled(!dirty)
                        .on_click(move |_, window, cx| {
                            let _ = apply_view.update(cx, |view, cx| view.commit_text_draft(TextSetting::CursorColor, &apply_input, window, cx));
                        }))
                    .child(Button::new("cursor-color-cancel").outline().label(crate::i18n::t("取消", "Cancel"))
                        .debug_selector(|| "cursor-color-cancel".to_string()).disabled(!dirty)
                        .on_click(move |_, window, cx| {
                            let _ = cancel_view.update(cx, |view, cx| view.restore_cursor_color(window, cx));
                        }))
                    .child(Button::new("cursor-color-default").ghost().label(crate::i18n::t("默认颜色", "Default colour"))
                        .debug_selector(|| "cursor-color-default".to_string())
                        .on_click(move |_, window, cx| {
                            let _ = default_view.update(cx, |view, cx| view.set_cursor_draft(String::new(), true, window, cx));
                        })))
                .child(h_flex().gap_3().children([
                    cursor_preview(crate::i18n::t("方块 ×0.7", "Block ×0.7"), 0, picker.read(cx).value().unwrap_or(gpui_kit::white()), cx),
                    cursor_preview(crate::i18n::t("竖线 ×1", "Bar ×1"), 1, picker.read(cx).value().unwrap_or(gpui_kit::white()), cx),
                    cursor_preview(crate::i18n::t("下划线 ×1", "Underline ×1"), 2, picker.read(cx).value().unwrap_or(gpui_kit::white()), cx),
                ]))
                .child(div().text_xs().text_color(if error.is_some() { cx.theme().danger } else { cx.theme().muted_foreground })
                    .child(error.unwrap_or_else(|| crate::i18n::t(
                        "点击应用，或在下方输入框按 Enter 保存；取消放弃草稿。选色和关闭弹层不会保存。",
                        "Apply or Enter in this text field saves; Cancel discards the draft. Picking or closing the popup does not save.",
                    ))))
                .child(div().text_xs().text_color(cx.theme().muted_foreground)
                    .child(if format == ColorFormat::Cmyk { crate::i18n::t(
                        "CMYK 四通道使用百分比，仅为未校准通用近似，不代表印刷效果。保存为 8 位 RGBA。",
                        "CMYK channels use percentages. This is an uncalibrated approximation, not a print proof. Saved as 8-bit RGBA.",
                    ) } else { crate::i18n::t(
                        "输入格式自动识别，以 6/8 位 HEX 保存。方块透明度再乘 0.7，细光标保持原值；选区和搜索提示不受影响。",
                        "Input format is detected; saved as 6/8-digit HEX. Block alpha is multiplied by 0.7; bar/underline keep it. Selection/search stay visible.",
                    ) }))
                .into_any_element()
            },
        )
    }
}
