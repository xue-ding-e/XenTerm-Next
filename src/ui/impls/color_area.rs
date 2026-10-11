//! Native continuous HSV editor; changes belong to the caller's draft.
//!
//! Adapted from Porabuild/HeroGPUI, Apache-2.0:
//! crates/herogpui-components/src/color_picker/area.rs
//! commit 875d67797968e5304f351490e66a866c3688622c
//! https://github.com/Porabuild/HeroGPUI/blob/875d67797968e5304f351490e66a866c3688622c/crates/herogpui-components/src/color_picker/area.rs
//! See the bundled HeroGPUI license and NOTICE for attribution.
//!
//! This port keeps the HSV gradient stack and capture-phase pointer pattern,
//! removes channel snapping and the Hero theme/form/animation framework, and
//! uses GPUI Kit 0.6.1 plus csscolorparser for conversions. Hue and alpha use
//! the same capture pattern. Latent HSV survives achromatic colours and quiet
//! canonical-RGBA synchronisation; no application persistence happens here.

use std::{cell::Cell, rc::Rc};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    black, canvas, checkerboard, div, hsla, linear_color_stop, linear_gradient, prelude::*, px,
    relative, rgb, white, AnyElement, App, Bounds, Context, CursorStyle, DispatchPhase,
    EventEmitter, FocusHandle, Focusable, Hsla, IntoElement, KeyDownEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Role, Subscription,
    TestSupportExt as _, Window,
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum HsvAreaEvent {
    /// Both representations describe this event, even if a later pointer move
    /// changes the editor before the owner processes its deferred callback.
    Change {
        color: Hsla,
        rgba: [u8; 4],
    },
    End(Hsla),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channel {
    Sv = 0,
    Hue = 1,
    Alpha = 2,
}

/// Three native focus targets: the saturation/value plane, hue, and alpha.
pub(crate) struct HsvAreaState {
    hue: f32,
    saturation: f32,
    brightness: f32,
    alpha: f32,
    focus: [FocusHandle; 3],
    bounds: [Rc<Cell<Bounds<Pixels>>>; 3],
    dragging: Option<Channel>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<HsvAreaEvent> for HsvAreaState {}

impl HsvAreaState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // With track_focus, GPUI reads tab-stop metadata from the handle;
        // a Div's tab_index alone does not update an externally owned handle.
        let focus = std::array::from_fn(|_| cx.focus_handle().tab_stop(true));
        let mut subscriptions = Vec::new();
        // Losing focus, including removal from the element tree, cancels a drag.
        // These subscriptions and the paint callbacks hold only weak entities.
        for (index, handle) in focus.iter().enumerate() {
            subscriptions.push(cx.on_blur(handle, window, move |this, _, cx| {
                if this
                    .dragging
                    .is_some_and(|channel| channel as usize == index)
                {
                    this.cancel_drag(cx);
                }
            }));
        }
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.cancel_drag(cx);
            }
        }));
        Self {
            hue: 0.,
            saturation: 0.,
            brightness: 1.,
            alpha: 1.,
            focus,
            bounds: std::array::from_fn(|_| Rc::new(Cell::new(Bounds::default()))),
            dragging: None,
            _subscriptions: subscriptions,
        }
    }

    /// Hue is in degrees; saturation and value are in 0..=1.
    pub(crate) fn hsv(&self) -> [f32; 3] {
        [self.hue * 360., self.saturation, self.brightness]
    }

    pub(crate) fn alpha(&self) -> f32 {
        self.alpha
    }

    /// Canonical bytes come directly from HSV, avoiding an HSL round trip at
    /// half-byte boundaries when the caller formats an area-originated change.
    pub(crate) fn rgba8(&self) -> [u8; 4] {
        self.color().to_rgba8()
    }

    pub(crate) fn value(&self) -> Hsla {
        let [_, saturation, lightness, alpha] = self.color().to_hsla();
        // HSL cannot represent black's hidden HSV saturation, but its hue can
        // still carry the user's selection. The full HSV remains in this state.
        hsla(self.hue, saturation, lightness, alpha)
    }

    fn color(&self) -> csscolorparser::Color {
        csscolorparser::Color::from_hsva(
            self.hue * 360.,
            self.saturation,
            self.brightness,
            self.alpha,
        )
    }

    /// Quietly reflect a text/preset change. Never quantize the live drag state.
    pub(crate) fn set_color(&mut self, color: Hsla, _: &mut Window, cx: &mut Context<Self>) {
        if ![color.h, color.s, color.l, color.a]
            .into_iter()
            .all(f32::is_finite)
        {
            return;
        }
        // An exact echo of our event is already current. Converting it back
        // through HSL can move a half-byte RGB boundary and interrupt a drag.
        if color == self.value() {
            return;
        }
        let color = csscolorparser::Color::from_hsla(
            color.h.rem_euclid(1.) * 360.,
            color.s.clamp(0., 1.),
            color.l.clamp(0., 1.),
            color.a.clamp(0., 1.),
        );
        let old = self.color().to_rgba8();
        let new = color.to_rgba8();
        // A formatted-text echo must not destroy hue at gray/black, endpoint
        // hue 360, or sub-byte pointer precision. Alpha-only edits retain RGB.
        if old == new {
            return;
        }
        if old[..3] != new[..3] {
            let [hue, saturation, brightness, _] = color.to_hsva();
            if saturation > f32::EPSILON && brightness > f32::EPSILON {
                self.hue = hue / 360.;
            }
            if brightness > f32::EPSILON {
                self.saturation = saturation;
            }
            self.brightness = brightness;
        }
        self.alpha = color.a;
        self.dragging = None;
        cx.notify();
    }

    /// An empty application colour means opaque white, with no latent hue.
    pub(crate) fn reset_default(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.hue = 0.;
        self.saturation = 0.;
        self.brightness = 1.;
        self.alpha = 1.;
        self.dragging = None;
        cx.notify();
    }

    pub(crate) fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        if self.dragging.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.dragging.is_some()
    }

    fn coordinates(&self, channel: Channel, position: Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.bounds[channel as usize].get();
        let [left, top, width, height, x, y] = [
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
            position.x,
            position.y,
        ]
        .map(f32::from);
        if width <= 0.
            || height <= 0.
            || ![left, top, width, height, x, y]
                .into_iter()
                .all(f32::is_finite)
        {
            return None;
        }
        Some((
            ((x - left) / width).clamp(0., 1.),
            ((y - top) / height).clamp(0., 1.),
        ))
    }

    fn apply_pointer(&mut self, channel: Channel, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((x, y)) = self.coordinates(channel, position) else {
            return;
        };
        let previous = self.channels();
        match channel {
            Channel::Sv => {
                self.saturation = x;
                self.brightness = 1. - y;
            }
            Channel::Hue => self.hue = x,
            Channel::Alpha => self.alpha = x,
        }
        self.emit_change(previous, cx);
    }

    fn channels(&self) -> [f32; 4] {
        [self.hue, self.saturation, self.brightness, self.alpha]
    }

    fn emit_change(&self, previous: [f32; 4], cx: &mut Context<Self>) -> bool {
        if self.channels() == previous {
            return false;
        }
        cx.emit(HsvAreaEvent::Change {
            color: self.value(),
            rgba: self.rgba8(),
        });
        cx.notify();
        true
    }

    fn pointer_down(
        &mut self,
        channel: Channel,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left
            || blocked_modifiers(event.modifiers)
            || self.coordinates(channel, event.position).is_none()
        {
            return;
        }
        window.focus(&self.focus[channel as usize], cx);
        self.dragging = Some(channel);
        self.apply_pointer(channel, event.position, cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn key_down(
        &mut self,
        channel: Channel,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if blocked_modifiers(event.keystroke.modifiers) {
            return;
        }
        let key = event.keystroke.key.as_str();
        // Each target has a real FocusHandle. Explicit traversal works without
        // a parent application keymap and lets Tab leave the component normally.
        if key == "tab" {
            if event.keystroke.modifiers.shift {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            cx.stop_propagation();
            return;
        }
        let previous = self.channels();
        let fine = if event.keystroke.modifiers.shift {
            0.1
        } else {
            1.
        };
        let step = if channel == Channel::Hue {
            fine / 360.
        } else {
            fine * 0.01
        };
        match channel {
            Channel::Sv => match key {
                "left" => self.saturation = (self.saturation - step).max(0.),
                "right" => self.saturation = (self.saturation + step).min(1.),
                "down" => self.brightness = (self.brightness - step).max(0.),
                "up" => self.brightness = (self.brightness + step).min(1.),
                "home" => {
                    self.saturation = 0.;
                    self.brightness = 0.;
                }
                "end" => {
                    self.saturation = 1.;
                    self.brightness = 1.;
                }
                _ => return,
            },
            Channel::Hue | Channel::Alpha => {
                let value = if channel == Channel::Hue {
                    &mut self.hue
                } else {
                    &mut self.alpha
                };
                *value = match key {
                    "left" | "down" => (*value - step).max(0.),
                    "right" | "up" => (*value + step).min(1.),
                    "home" => 0.,
                    "end" => 1.,
                    _ => return,
                };
            }
        }
        self.dragging = None;
        if self.emit_change(previous, cx) {
            cx.emit(HsvAreaEvent::End(self.value()));
        }
        cx.stop_propagation();
    }

    fn control(&self, channel: Channel, window: &Window, cx: &Context<Self>) -> AnyElement {
        let focus = &self.focus[channel as usize];
        let (id, label, x, y, height) = match channel {
            Channel::Sv => (
                "hsv-sv-plane",
                crate::i18n::t("饱和度和明度", "Saturation and value"),
                self.saturation,
                1. - self.brightness,
                152.,
            ),
            Channel::Hue => (
                "hsv-hue-track",
                crate::i18n::t("色相", "Hue"),
                self.hue,
                0.5,
                18.,
            ),
            Channel::Alpha => (
                "hsv-alpha-track",
                crate::i18n::t("不透明度", "Opacity"),
                self.alpha,
                0.5,
                18.,
            ),
        };
        let mut layers = div().absolute().inset_0().rounded(px(4.)).overflow_hidden();
        match channel {
            Channel::Sv => {
                layers = layers
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(white(), 0.),
                        linear_color_stop(hsla(self.hue, 1., 0.5, 1.), 1.),
                    ))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(px(4.))
                            .bg(linear_gradient(
                                180.,
                                linear_color_stop(black().opacity(0.), 0.),
                                linear_color_stop(black(), 1.),
                            )),
                    );
            }
            Channel::Hue => {
                for index in 0..6 {
                    layers = layers.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(index as f32 / 6.))
                            .w(relative(1. / 6.))
                            .when(index == 0, |el| el.rounded_tl(px(4.)).rounded_bl(px(4.)))
                            .when(index == 5, |el| el.rounded_tr(px(4.)).rounded_br(px(4.)))
                            .bg(linear_gradient(
                                90.,
                                linear_color_stop(hsla(index as f32 / 6., 1., 0.5, 1.), 0.),
                                linear_color_stop(hsla((index + 1) as f32 / 6., 1., 0.5, 1.), 1.),
                            )),
                    );
                }
            }
            Channel::Alpha => {
                let color = self.value();
                layers = layers
                    .bg(rgb(0xf5f5f5))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(px(4.))
                            .bg(checkerboard(rgb(0xa0a0a0), 6.)),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(px(4.))
                            .bg(linear_gradient(
                                90.,
                                linear_color_stop(Hsla { a: 0., ..color }, 0.),
                                linear_color_stop(Hsla { a: 1., ..color }, 1.),
                            )),
                    );
            }
        }
        let bounds = self.bounds[channel as usize].clone();
        let thumb_color = if channel == Channel::Hue {
            hsla(self.hue, 1., 0.5, 1.)
        } else {
            Hsla {
                a: 1.,
                ..self.value()
            }
        };
        let description = match channel {
            Channel::Sv => format!(
                "S {:.1}%, V {:.1}%. {}",
                self.saturation * 100.,
                self.brightness * 100.,
                crate::i18n::t(
                    "方向键 1%，Shift 0.1%；Home 最小，End 最大",
                    "Arrows 1%, Shift 0.1%; Home minimum, End maximum"
                )
            ),
            Channel::Hue => format!(
                "{:.1}°. {}",
                self.hue * 360.,
                crate::i18n::t(
                    "方向键 1°，Shift 0.1°；Home 0°，End 360°",
                    "Arrows 1°, Shift 0.1°; Home 0°, End 360°"
                )
            ),
            Channel::Alpha => format!(
                "{:.1}%. {}",
                self.alpha * 100.,
                crate::i18n::t(
                    "方向键 1%，Shift 0.1%；Home 0%，End 100%",
                    "Arrows 1%, Shift 0.1%; Home 0%, End 100%"
                )
            ),
        };
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .relative()
            .w_full()
            .h(px(height))
            .flex_shrink_0()
            .track_focus(focus)
            .tab_index(0)
            .key_context("HsvArea")
            .role(if channel == Channel::Sv {
                Role::Group
            } else {
                Role::Slider
            })
            .aria_label(label)
            .aria_description(description)
            .when(channel != Channel::Sv, |el| {
                el.aria_min_numeric_value(0.)
                    .aria_max_numeric_value(if channel == Channel::Hue { 360. } else { 100. })
                    .aria_numeric_value(if channel == Channel::Hue {
                        f64::from(self.hue * 360.)
                    } else {
                        f64::from(self.alpha * 100.)
                    })
                    .aria_numeric_value_step(1.)
            })
            .cursor(CursorStyle::Crosshair)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    this.pointer_down(channel, event, window, cx)
                }),
            )
            .on_key_down(
                cx.listener(move |this, event, window, cx| {
                    this.key_down(channel, event, window, cx)
                }),
            )
            .child(
                canvas(move |rect, _, _| bounds.set(rect), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .child(layers)
            // Keep the frame below the thumb. Two fixed contrast rings remain
            // visible over white, black, every hue, and the alpha checkerboard.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(if focus.is_focused(window) {
                        cx.theme().primary
                    } else {
                        rgb(0x777777).into()
                    }),
            )
            .child(
                div()
                    .absolute()
                    .left(relative(x))
                    .top(relative(y))
                    .ml(px(-7.))
                    .mt(px(-7.))
                    .size(px(14.))
                    .rounded_full()
                    .border_1()
                    .border_color(black())
                    .bg(white())
                    .p(px(2.))
                    .child(div().size_full().rounded_full().bg(thumb_color)),
            )
            .test_support()
            .into_any_element()
    }
}

fn blocked_modifiers(modifiers: Modifiers) -> bool {
    modifiers.control || modifiers.alt || modifiers.platform || modifiers.function
}

impl Focusable for HsvAreaState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus[Channel::Sv as usize].clone()
    }
}

impl Render for HsvAreaState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        div()
            .relative()
            .flex()
            .flex_col()
            .w(px(260.))
            .p_2()
            .gap_3()
            .child(self.control(Channel::Sv, window, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::i18n::t("色相", "Hue")),
            )
            .child(self.control(Channel::Hue, window, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::i18n::t("不透明度", "Opacity")),
            )
            .child(self.control(Channel::Alpha, window, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::i18n::t(
                        "方向键微调 · Shift 精调",
                        "Arrow keys adjust · Shift for finer steps",
                    )),
            )
            // Register after painting, once per frame, so capture continues
            // outside the control while mounted and disappears on unmount.
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let move_weak = weak.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            let _ = move_weak.update(cx, |this, cx| {
                                let Some(channel) = this.dragging else {
                                    return;
                                };
                                if event.pressed_button != Some(MouseButton::Left) {
                                    this.cancel_drag(cx);
                                } else if !blocked_modifiers(event.modifiers) {
                                    this.apply_pointer(channel, event.position, cx);
                                }
                            });
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture || event.button != MouseButton::Left
                            {
                                return;
                            }
                            let _ = weak.update(cx, |this, cx| {
                                let Some(channel) = this.dragging.take() else {
                                    return;
                                };
                                if !blocked_modifiers(event.modifiers) {
                                    this.apply_pointer(channel, event.position, cx);
                                }
                                cx.emit(HsvAreaEvent::End(this.value()));
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
    }
}

#[cfg(test)]
#[path = "color_area_tests.rs"]
mod tests;
