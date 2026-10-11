//! Dependency regressions using the same Input and ContextMenu components as the app.
//! No profile, terminal backend, network, or credentials are involved. Keep these
//! fixtures independent of application fixes so they can also run on Kit 0.6.1.
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use gpui_kit::{
    component::{
        input::{Input, InputState},
        menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
    },
    div,
    gpui::{
        DismissEvent, EmptyView, Focusable as _, Modifiers, MouseButton, TestAppContext,
        VisualContext as _, VisualTestContext, WeakEntity,
    },
    point,
    prelude::*,
    px, Context, Entity, IntoElement, SharedString, Window,
};

fn settle_frames(cx: &mut VisualTestContext) {
    // Both buffered frames may hold the old menu. Retire them before checking
    // release, without confusing a dismissal event with destruction.
    for _ in 0..6 {
        cx.update(|window, cx| {
            window.refresh();
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
    }
}

struct MenuHost {
    selected: Rc<Cell<usize>>,
}

impl Render for MenuHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        div().size_full().child(
            div()
                .id("toolkit-menu-host")
                .debug_selector(|| "toolkit-menu-host".into())
                .w(px(240.))
                .h(px(60.))
                .child("Synthetic context-menu target")
                .context_menu(move |menu, _, _| {
                    let selected = selected.clone();
                    menu.item(
                        PopupMenuItem::new("Select fixture").on_click(move |_, _, _| {
                            selected.set(selected.get() + 1);
                        }),
                    )
                }),
        )
    }
}

#[derive(Clone, Copy)]
enum Dismissal {
    Escape,
    OutsideClick,
    SelectItem,
}

fn check_menu_release(cx: &mut TestAppContext, dismissal: Dismissal) {
    cx.update(gpui_kit::init);
    let survivor = cx.add_empty_window().window_handle();
    let menus = Rc::new(RefCell::new(Vec::<WeakEntity<PopupMenu>>::new()));
    let observed = menus.clone();
    let observer = cx.update(|cx| {
        cx.observe_new::<PopupMenu>(move |_, _, cx| observed.borrow_mut().push(cx.weak_entity()))
    });
    let selected = Rc::new(Cell::new(0));
    let for_host = selected.clone();
    let (view, cx) = cx.add_window_view(move |_, _| MenuHost { selected: for_host });
    settle_frames(cx);
    assert!(menus.borrow().is_empty());
    let position = cx
        .debug_bounds("toolkit-menu-host")
        .expect("visible menu target")
        .center();
    cx.simulate_mouse_move(position, None, Modifiers::default());
    cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    settle_frames(cx);
    assert_eq!(menus.borrow().len(), 1);
    let menu = menus.borrow()[0].clone();
    let dismissed = Rc::new(Cell::new(0));
    let count = dismissed.clone();
    let subscription = cx.update(|_, cx| {
        cx.subscribe(
            &menu.upgrade().expect("live menu"),
            move |_, _: &DismissEvent, _| {
                count.set(count.get() + 1);
            },
        )
    });
    match dismissal {
        Dismissal::Escape => cx.simulate_keystrokes("escape"),
        Dismissal::SelectItem => cx.simulate_keystrokes("down enter"),
        Dismissal::OutsideClick => {
            let outside = cx.update(|window, _| {
                let size = window.bounds().size;
                point(size.width - px(2.), size.height - px(2.))
            });
            cx.simulate_mouse_move(outside, None, Modifiers::default());
            cx.simulate_mouse_down(outside, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        }
    }
    settle_frames(cx);
    assert_eq!(dismissed.get(), 1);
    assert_eq!(
        selected.get(),
        usize::from(matches!(dismissal, Dismissal::SelectItem))
    );
    drop(subscription);
    drop(observer);
    let host = view.downgrade();
    cx.update(|window, cx| {
        window.replace_root(cx, |_, _| EmptyView);
    });
    drop(view);
    settle_frames(cx);
    assert!(cx.debug_bounds("toolkit-menu-host").is_none());
    assert!(cx.update(|_, cx| cx.windows().contains(&survivor)));
    assert_eq!(cx.update(|_, cx| cx.windows().len()), 2);
    host.assert_released();
    menu.assert_released();
}

#[gpui_kit::gpui::test]
fn popup_menu_escape_releases_after_host_removal(cx: &mut TestAppContext) {
    check_menu_release(cx, Dismissal::Escape);
}

#[gpui_kit::gpui::test]
fn popup_menu_outside_click_releases_after_host_removal(cx: &mut TestAppContext) {
    check_menu_release(cx, Dismissal::OutsideClick);
}

#[gpui_kit::gpui::test]
fn popup_menu_selection_releases_after_host_removal(cx: &mut TestAppContext) {
    check_menu_release(cx, Dismissal::SelectItem);
}

struct InputHost {
    input: Entity<InputState>,
    model: SharedString,
}

impl Render for InputHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.input.read(cx).value() != self.model {
            let value = self.model.clone();
            self.input
                .update(cx, |input, cx| input.set_value(value, window, cx));
        }
        div().size_full().child(Input::new(&self.input))
    }
}

#[gpui_kit::gpui::test]
fn unfocused_input_reconciliation_does_not_start_a_blink_loop(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, cx) = cx.add_window_view(|window, cx| InputHost {
        input: cx.new(|cx| InputState::new(window, cx)),
        model: "".into(),
    });
    let input = view.read_with(cx, |view, _| view.input.clone());
    cx.update(|window, cx| window.blur(cx));
    settle_frames(cx);
    let notifications = Rc::new(Cell::new(0));
    let count = notifications.clone();
    let _subscription =
        cx.update(|_, cx| cx.observe(&input, move |_, _| count.set(count.get() + 1)));
    view.update(cx, |view, cx| {
        view.model = "#123456".into();
        cx.notify();
    });
    settle_frames(cx);
    let settled = notifications.get();
    for _ in 0..6 {
        cx.executor().advance_clock(Duration::from_millis(500));
        settle_frames(cx);
    }
    cx.update(|window, cx| {
        assert!(!input.read(cx).focus_handle(cx).is_focused(window));
        assert_eq!(input.read(cx).value().as_ref(), "#123456");
    });
    assert_eq!(
        notifications.get(),
        settled,
        "an unfocused field must stay quiet after reconciliation"
    );
}
