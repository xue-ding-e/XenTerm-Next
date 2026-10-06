//! The tab strip, as its own view.
//!
//! It used to be four hundred lines built inside [`TerminalPage::render`] — which
//! means it was rebuilt on every terminal frame, because a `cx.notify()` in this
//! gpui marks the whole ancestor chain dirty and the page is the terminal's
//! ancestor. As a child view it is cached: the page re-renders at the terminal's
//! pace, but the strip's element tree is rebuilt only when the strip itself is
//! notified, which happens when the page's state changes (a tab opened, closed,
//! renamed, activated) — never because output arrived.
//!
//! State flows one way: the page is the owner, the strip a projection. It reads
//! through [`TerminalPage`]'s accessors and writes through its methods or the two
//! action queues the two share — the same queues the strip's menus filled when
//! this tree lived on the page, drained by the same `drain_tab_actions`.
//!
//! What the page cannot tell the strip about is a session's state dot: the status
//! map is a plain `Arc<Mutex>`, invisible to dependency tracking, so the terminal
//! view that writes a transition also notifies the page — rare events, not per
//! frame.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    menu::{ContextMenuExt, PopupMenuItem},
    popover::Popover,
    v_flex,
    ActiveTheme as _,
    Disableable as _,
    Sizable as _,
};
use gpui_kit::{
    div, prelude::*, px, rgb, Animation, AnimationExt as _, Context, Entity, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, SharedString, Subscription, WeakEntity, Window,
};

use gpui_kit::assets::IconName;

use crate::ui::{TransferListView, TerminalView};
use super::pages::terminal_page::{TabAction, TerminalAction, TerminalPage};

/// The tab strip: one chip per session, the quick-connect door, and the strip's
/// right-end session actions.
pub(crate) struct TabStripView {
    /// The page whose tabs these are. Every render reads it; every handler
    /// updates it.
    page: WeakEntity<TerminalPage>,
    /// Shared with the page: what a chip's menu asked for, drained by the page's
    /// `drain_tab_actions`.
    tab_action: Rc<RefCell<Option<TabAction>>>,
    /// Shared with the page: what the strip asked on the shell's behalf
    /// (quick connect, the tunnels dialog, the active-tab follow).
    action: Rc<RefCell<Option<TerminalAction>>>,
    /// Shared with the page: each chip's on-screen band, recorded by its canvas
    /// at paint, which is what answers "which chip is the pointer over".
    chip_bounds: Rc<RefCell<Vec<(String, f32, f32)>>>,
    /// The drag, while it is one: the chip being dragged and where it started.
    tab_drag: Option<(String, f32)>,
    /// What keeps the strip redrawn when the page's state changes. Without it a
    /// cached strip would go stale the moment a tab opened.
    _observe_page: Subscription,
}

impl TabStripView {
    pub(crate) fn new(
        page: &Entity<TerminalPage>,
        action: Rc<RefCell<Option<TerminalAction>>>,
        tab_action: Rc<RefCell<Option<TabAction>>>,
        chip_bounds: Rc<RefCell<Vec<(String, f32, f32)>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(page, |_, _, cx| cx.notify());
        Self {
            page: page.downgrade(),
            tab_action,
            action,
            chip_bounds,
            tab_drag: None,
            _observe_page: observe,
        }
    }

    fn begin_tab_drag(&mut self, id: String, x: f32) {
        self.tab_drag = Some((id, x));
    }

    fn end_tab_drag(&mut self) {
        self.tab_drag = None;
    }

    /// Reorder live while the pointer carries a chip past a neighbour. A move
    /// under 4px is a click that has not happened yet, not a drag.
    fn drag_tab_to(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(page) = self.page.upgrade() else {
            return;
        };
        let Some((id, start_x)) = self.tab_drag.clone() else {
            return;
        };
        if (x - start_x).abs() < 4.0 {
            return;
        }
        let target = {
            let bounds = self.chip_bounds.borrow();
            bounds
                .iter()
                .find(|(_, bx, w)| x >= *bx && x <= bx + w)
                .map(|(bid, _, _)| bid.clone())
        };
        let Some(target) = target else {
            return;
        };
        if target == id {
            return;
        }
        page.update(cx, |page, cx| {
            if let (Some(from), Some(to)) = (
                page.tabs.iter().position(|tab| tab.id.as_str() == id),
                page.tabs.iter().position(|tab| tab.id.as_str() == target),
            ) {
                let tab = page.tabs.remove(from);
                page.tabs.insert(to, tab);
                cx.notify();
            }
        });
    }
}

impl Render for TabStripView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(page) = self.page.upgrade() else {
            return div().into_any_element();
        };
        let page_state = page.read(cx);
        let active = page_state.active_tab.clone();
        let border = cx.theme().border;
        let primary = cx.theme().primary;
        let muted = cx.theme().muted;
        let muted_fg = cx.theme().muted_foreground;
        let success = cx.theme().success;
        let warning = cx.theme().warning;
        // The dot is the tab's real session state, not a decoration: `TabStatus::state`
        // is what the session writes when it connects and when it ends, so a tab whose
        // shell has died stops looking alive. Reading it here rather than keeping a copy
        // is the one-owner rule this shell keeps everywhere: read the store, do not copy it.
        // The raw phase — 0 dialling, 1 live, 2 ended — rather than a live flag, because
        // a dialling tab and a dead one are different facts the strip has to tell apart:
        // the first pulses, the second greys out.
        let states: Vec<(String, String, u8)> = page_state
            .tabs
            .iter()
            .map(|tab| {
                let state = page_state.tab_state(tab.id.as_str());
                (
                    tab.id.as_str().to_string(),
                    tab.meta.title().to_string(),
                    state,
                )
            })
            .collect();

        // Stale entries for chips that closed last frame must not answer a
        // drag: cleared here, re-recorded by each chip's canvas at paint.
        self.chip_bounds.borrow_mut().clear();
        let chip_bounds = self.chip_bounds.clone();

        let renaming_pair = page_state.renaming.clone();
        let sftp_collapsed = page_state.sftp_collapsed;
        let sftp_available = page_state.sftp_available();
        let transfers = page_state.transfers().clone();

        h_flex()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .child(
                // The quick-connect door: one click — or Ctrl+K from anywhere —
                // puts a searchable list of every saved session and built-in
                // shell over the work, and a confirm connects in place. The
                // hot path for "open another session" should not run through
                // the connections page.
                Button::new("quick-connect")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .tooltip(crate::i18n::t("快速连接（Ctrl+K）", "Quick connect (Ctrl+K)"))
                    .accessibility_label(crate::i18n::t("快速连接", "Quick connect"))
                    .on_click({
                        let action = self.action.clone();
                        let strip = cx.entity();
                        move |_, _, cx| {
                            *action.borrow_mut() = Some(TerminalAction::OpenQuickConnect);
                            // The queue is drained by the page's render, which this
                            // repaint schedules: the strip is the page's child, so
                            // marking it dirty marks the page too.
                            let _ = strip.update(cx, |_, cx| cx.notify());
                        }
                    }),
            )
            .child(
                // h_flex, not a plain div: a bare div lays its children out in
                // a column, which stacked the tab chips on top of each other.
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .overflow_hidden()
                    // Drag to reorder: press a chip, move it past a neighbour,
                    // and the strip reorders live. The pointer handlers live on
                    // the strip rather than the chip so a drag keeps running
                    // after the pointer crosses onto another chip.
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        this.drag_tab_to(f32::from(event.position.x), cx);
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| {
                        this.end_tab_drag();
                    }))
                    .children(states.into_iter().map(|(id, title, state)| {
                        let selected = active.as_deref() == Some(id.as_str());
                        let click_id = id.clone();
                        let close_id = id.clone();
                        // The rename field belongs to this chip while a rename is in progress:
                        // the title becomes an input in the same place the title was.
                        let renaming = renaming_pair
                            .as_ref()
                            .filter(|(tab, _)| tab == &id)
                            .map(|(_, input)| input.clone());
                        let pending = self.tab_action.clone();
                        let for_rename = pending.clone();
                        let for_duplicate = pending.clone();
                        let for_close = pending.clone();
                        let for_close_others = pending.clone();
                        let for_move_left = pending.clone();
                        let for_move_right = pending.clone();
                        let for_split = pending.clone();
                        let rename_id = id.clone();
                        let duplicate_id = id.clone();
                        let menu_close_id = id.clone();
                        let others_id = id.clone();
                        let dot_id = id.clone();
                        // Duplicating is only meaningful for a tab that names a session: a second
                        // tab of a session is a second connection, and there is nothing to connect
                        // when the id is not one the store or the built-in shells know.
                        let duplicable = page_state.tab_duplicable(&id);
                        let id_for_drag = id.clone();
                        let id_for_bounds = id.clone();
                        let id_for_compare = id.clone();
                        let id_for_close = id.clone();
                        let dragging = self
                            .tab_drag
                            .as_ref()
                            .map(|(dragged, _)| dragged == &id)
                            .unwrap_or(false);
                        h_flex()
                            .id(SharedString::from(format!("tab-{id}")))
                            .relative()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                                    this.begin_tab_drag(
                                        id_for_drag.clone(),
                                        f32::from(event.position.x),
                                    );
                                }),
                            )
                            .when(selected, |this| this.bg(primary.opacity(0.15)))
                            .when(!selected, |this| this.hover(|this| this.bg(muted)))
                            .when(dragging, |this| {
                                this.border_1()
                                    .border_color(primary)
                                    .opacity(0.55)
                                    .bg(primary.opacity(0.08))
                            })
                            .cursor_pointer()
                            .on_click({
                                let page = self.page.clone();
                                move |_, _, cx| {
                                    if let Some(page) = page.upgrade() {
                                        let _ =
                                            page.update(cx, |page, cx| {
                                                page.set_active_tab(
                                                    Some(click_id.clone()),
                                                    cx,
                                                );
                                            });
                                    }
                                }
                            })
                            .context_menu(move |menu, _, _| {
                                let rename = for_rename.clone();
                                let duplicate = for_duplicate.clone();
                                let close = for_close.clone();
                                let others = for_close_others.clone();
                                let left = for_move_left.clone();
                                let right = for_move_right.clone();
                                let rename_id = rename_id.clone();
                                let duplicate_id = duplicate_id.clone();
                                let close_id = menu_close_id.clone();
                                let others_id = others_id.clone();
                                let left_id = menu_close_id.clone();
                                let right_id = others_id.clone();
                                let split = for_split.clone();
                                let split_id = right_id.clone();
                                let split_down = for_split.clone();
                                let split_id2 = right_id.clone();
                                let mut menu = menu.item(
                                    PopupMenuItem::new(crate::i18n::t("重命名标签", "Rename tab"))
                                        .on_click(move |_, _, _| {
                                            *rename.borrow_mut() =
                                                Some(TabAction::Rename(rename_id.clone()));
                                        }),
                                );
                                if duplicable {
                                    menu = menu.item(
                                        PopupMenuItem::new(crate::i18n::t(
                                            "复制标签",
                                            "Duplicate tab",
                                        ))
                                        .on_click(move |_, _, _| {
                                            *duplicate.borrow_mut() =
                                                Some(TabAction::Duplicate(duplicate_id.clone()));
                                        }),
                                    );
                                }
                                menu.separator()
                                    .item(
                                        // Moving a tab is a menu entry rather than a drag. A drag
                                        // between chips is a gesture this shell cannot make
                                        // reliable — the pointer leaves the chip it started on,
                                        // and GPUI delivers moves to whatever is under it — and
                                        // the outcome the user wants is a position, which two
                                        // entries give exactly.
                                        PopupMenuItem::new(crate::i18n::t("左移", "Move left"))
                                            .on_click(move |_, _, _| {
                                                *left.borrow_mut() =
                                                    Some(TabAction::MoveLeft(left_id.clone()));
                                            }),
                                    )
                                    .item(
                                        PopupMenuItem::new(crate::i18n::t("右移", "Move right"))
                                            .on_click(move |_, _, _| {
                                                *right.borrow_mut() =
                                                    Some(TabAction::MoveRight(right_id.clone()));
                                            }),
                                    )
                                    .separator()
                                    .item(
                                        // Splitting moves the *next* tab into a new pane beside this
                                        // one, which is what a split does with a tab id: a split is an
                                        // arrangement of tabs you already have, not a second connection
                                        // made for the occasion. With one tab open there is nothing to
                                        // move, so the entry is inert.
                                        PopupMenuItem::new(crate::i18n::t("向右分屏", "Split right"))
                                            .on_click(move |_, _, _| {
                                                *split.borrow_mut() =
                                                    Some(TabAction::Split(split_id.clone()));
                                            }),
                                    )
                                    .item(
                                        // The layout tree has supported a vertical
                                        // split from the start; this is the door to
                                        // it. Ctrl+Shift+O is the keyboard form.
                                        PopupMenuItem::new(crate::i18n::t(
                                            "向下分屏",
                                            "Split down",
                                        ))
                                        .on_click(move |_, _, _| {
                                            *split_down.borrow_mut() =
                                                Some(TabAction::SplitDown(split_id2.clone()));
                                        }),
                                    )
                                    .separator()
                                    .item(
                                        PopupMenuItem::new(crate::i18n::t(
                                            "关闭其他标签",
                                            "Close others",
                                        ))
                                        .on_click(move |_, _, _| {
                                            *others.borrow_mut() =
                                                Some(TabAction::CloseOthers(others_id.clone()));
                                        }),
                                    )
                                    .item(
                                        PopupMenuItem::new(crate::i18n::t(
                                            "关闭标签",
                                            "Close tab",
                                        ))
                                        .on_click(move |_, _, _| {
                                            *close.borrow_mut() =
                                                Some(TabAction::Close(close_id.clone()));
                                        }),
                                    )
                            })
                            // The chip's own bounds, recorded for the drag: which
                            // chip the pointer is over is answered from here, not
                            // from hit-testing a moving element.
                            .child(
                                gpui_kit::canvas(
                                    {
                                        let chip_bounds = chip_bounds.clone();
                                        move |bounds, _, _| {
                                            let mut bounds_map = chip_bounds.borrow_mut();
                                            let entry = (
                                                id_for_bounds.clone(),
                                                f32::from(bounds.origin.x),
                                                f32::from(bounds.size.width),
                                            );
                                            if let Some(slot) = bounds_map
                                                .iter_mut()
                                                .find(|(bid, _, _)| *bid == id)
                                            {
                                                *slot = entry;
                                            } else {
                                                bounds_map.push(entry);
                                            }
                                        }
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                            .child(
                                // Three phases, three looks: a live session uses the
                                // theme's success colour (the accent means "this tab is
                                // selected", a different fact already carried by the
                                // chip's background), a dialling one pulses so a slow
                                // connect reads as progress rather than a hang, and an
                                // ended one greys out instead of glowing green.
                                match state {
                                    1 => div()
                                        .size_2()
                                        .rounded_full()
                                        .bg(success)
                                        .into_any_element(),
                                    2 => div()
                                        .size_2()
                                        .rounded_full()
                                        // Full opacity: at 0.6 the grey dot on a
                                        // selected chip measured 2.7:1 in the
                                        // light theme — "this session is dead"
                                        // was faintest exactly when the user
                                        // was looking at the tab.
                                        .bg(muted_fg)
                                        .into_any_element(),
                                    _ => div()
                                        .size_2()
                                        .rounded_full()
                                        .bg(warning)
                                        .with_animation(
                                            SharedString::from(format!("tab-dot-{dot_id}")),
                                            Animation::new(Duration::from_millis(1200)).repeat(),
                                            |dot, delta| {
                                                // One sine cycle per lap: eases in and out of
                                                // bright, and never blinks hard off.
                                                let pulse =
                                                    0.5 - 0.5 * (std::f32::consts::TAU * delta).cos();
                                                dot.opacity(0.35 + 0.65 * pulse)
                                            },
                                        )
                                        .into_any_element(),
                                },
                            )
                            .when_some(renaming, |this, input| {
                                this.child(
                                    div()
                                        .w(px(160.))
                                        .child(Input::new(&input)),
                                )
                            })
                            .when(
                                renaming_pair
                                    .as_ref()
                                    .map(|(tab, _)| tab != &id_for_compare)
                                    .unwrap_or(true),
                                |this| this.child(div().text_sm().child(SharedString::from(title))),
                            )
                            .child(
                                // An icon, not the "×" character: a glyph's weight depends on
                                // whichever font wins the fallback chain, so it lands at the
                                // wrong size against a real icon set and cannot be themed.
                                Button::new(SharedString::from(format!("close-{id_for_close}")))
                                    .icon(IconName::X)
                                    .ghost()
                                    .tooltip(crate::i18n::t("关闭", "Close"))
                                    .accessibility_label(crate::i18n::t("关闭标签", "Close tab"))
                                    .on_click({
                                        let page = self.page.clone();
                                        move |_, _, cx| {
                                            if let Some(page) = page.upgrade() {
                                                let _ = page.update(cx, |page, cx| {
                                                    page.close_tab(&close_id, cx);
                                                });
                                            }
                                        }
                                    }),
                            )
                    })),
            )
            // The occasional actions, as one compact icon row at the strip's right end —
            // the original keeps them there too. They belong to the *session*, not to
            // the line being typed, and a command line that carried them would spend
            // the width the command needs.
            .child(
                Button::new("toggle-sftp-panel")
                    .debug_selector(|| "toggle-sftp-panel".to_string())
                    .icon(IconName::FolderOpen)
                    .ghost()
                    .tooltip(if !sftp_available {
                        // A session without an SFTP channel — a local shell, a serial
                        // line — has no directory to show, and a toggle that opened an
                        // empty frame would be a toggle that lied.
                        crate::i18n::t(
                            "当前会话没有远程文件",
                            "This session has no remote files",
                        )
                    } else if sftp_collapsed {
                        crate::i18n::t("显示文件面板", "Show file panel")
                    } else {
                        crate::i18n::t("隐藏文件面板", "Hide file panel")
                    })
                    .accessibility_label(if !sftp_available {
                        crate::i18n::t(
                            "当前会话没有远程文件",
                            "This session has no remote files",
                        )
                    } else if sftp_collapsed {
                        crate::i18n::t("显示文件面板", "Show file panel")
                    } else {
                        crate::i18n::t("隐藏文件面板", "Hide file panel")
                    })
                    .disabled(!sftp_available)
                    .on_click({
                        let page = self.page.clone();
                        move |_, _, cx| {
                            if let Some(page) = page.upgrade() {
                                let _ = page.update(cx, |page, cx| {
                                    page.set_sftp_collapsed(!page.sftp_collapsed, cx);
                                });
                            }
                        }
                    }),
            )
            .child(
                // The transfer records' door, and its indicator: a label while anything
                // is still running, because a transfer you cannot see is the one you
                // forget you started. The list opens as a bubble off this button, the
                // same way the command bar's quick commands and history do.
                Popover::new("transfer-records")
                    .trigger(
                        Button::new("toggle-transfers")
                            .icon(IconName::ArrowDownUp)
                            .ghost()
                            .tooltip(crate::i18n::t("传输记录", "Transfer records"))
                            .accessibility_label(crate::i18n::t("传输记录", "Transfer records"))
                            .when(transfers.read(cx).has_active(), |this| {
                                this.label(crate::i18n::t("传输中", "Transferring"))
                            }),
                    )
                    .content({
                        let transfers = transfers.clone();
                        move |_, _, _| div().w_full().child(transfers.clone())
                    }),
            )
            .child(
                // The tunnel dialog's door: the forwards this session is running. It
                // sits with the transfers rather than in the command bar, because it
                // is about the session's connections and not about the line being
                // typed.
                Button::new("open-tunnels")
                    .icon(IconName::Cable)
                    .ghost()
                    .tooltip(crate::i18n::t("端口转发", "Port forwarding"))
                    .accessibility_label(crate::i18n::t("端口转发", "Port forwarding"))
                    .on_click({
                        let action = self.action.clone();
                        let strip = cx.entity();
                        move |_, _, cx| {
                            *action.borrow_mut() = Some(TerminalAction::OpenTunnels);
                            let _ = strip.update(cx, |_, cx| cx.notify());
                        }
                    }),
            )
            .into_any_element()
    }
}

// The terminal view is named here only so the doc link above resolves; the
// strip never touches it directly — state transitions reach the strip through
// the page they notify.
#[allow(unused)]
fn _types() {
    fn _assert(_: std::option::Option<Entity<TerminalView>>) {}
    let _ = rgb(0x000000);
    let _ = v_flex();
}
