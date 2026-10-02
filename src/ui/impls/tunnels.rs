//! The tunnel panel: one session's forwards, and whether they are running.
//!
//! ## What it is
//!
//! The original's port-forwarding window, as a full-window overlay: the rules this
//! session is running, each with its state, and a form to start another. A rule's shape
//! and its validation are `crate::core::tunnel`'s — one set of sentences this panel and
//! the session editor share, so a rule is refused for the same reasons wherever it is
//! entered.
//!
//! ## Where the rows come from
//!
//! The SSH layer is the only thing that knows whether a tunnel is up, and it says so
//! through `SessionEvent::TunnelUpdate`, which arrives at the session's own terminal
//! view. The rows are therefore read from the map the view writes them into, and the
//! panel watches that map: a tunnel that dies while the window is open has to stop
//! being shown as running, and nothing tells the panel directly.
//!
//! ## What is absent
//!
//! Editing the session's *saved* rules is the session editor's business, not this
//! panel's: it starts a tunnel for as long as the session lives, which is what the
//! runtime `AddTunnel` command does, and saving a rule for next time is a different
//! decision made in a different place.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputState},
        v_flex, ActiveTheme as _, Icon, Sizable as _,
    },
    div,
    prelude::*,
    px, AnyElement, Context, Entity, FontWeight, IntoElement, Render, SharedString, Subscription,
    Window,
};

use gpui_kit::assets::IconName;

use crate::config::{ConfigStore, PortForward};
use crate::core::tunnel::{self, TunnelDraft};
use crate::session::protocol::RuntimeTunnelInfo;

use super::session_state::TabTunnels;

/// What the panel wants the shell to do.
///
/// Recorded rather than sent, because sending needs the session handle the shell owns —
/// the same arrangement the SFTP panel uses for its commands.
///
/// No `PartialEq`: `PortForward` has none, and nothing compares two actions — the shell
/// drains them one at a time.
#[derive(Clone, Debug)]
pub(crate) enum TunnelAction {
    /// Bring up a tunnel for this session; it lives until it is stopped or the session
    /// ends.
    Start(PortForward),
    /// Take down the tunnel with this runtime id.
    Stop(String),
}

/// The panel.
pub(crate) struct TunnelsView {
    /// The session whose tunnels these are, named in the heading.
    session: Option<String>,
    /// The form's rows, one per rule being written, plus a spare.
    drafts: Vec<TunnelDraft>,
    name: Entity<InputState>,
    bind_port: Entity<InputState>,
    host: Entity<InputState>,
    host_port: Entity<InputState>,
    /// What was wrong with the last attempt, in the words the validator used.
    error: Option<String>,
    /// What was drawn last, so the poll only repaints on a change.
    shown: Vec<RuntimeTunnelInfo>,
    pending: Option<TunnelAction>,
    _subscriptions: Vec<Subscription>,
    _ticker: gpui_kit::Task<()>,
}

impl TunnelsView {
    pub(crate) fn new(
        _store: Rc<RefCell<ConfigStore>>,
        tab: Option<String>,
        session: Option<String>,
        tunnels: TabTunnels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::t("名称", "Name")));
        let bind_port = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::t("监听端口", "Listen port"))
        });
        let host = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::t("目标主机", "Target host"))
        });
        let host_port = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::t("目标端口", "Target port"))
        });

        let mut subscriptions = Vec::new();
        for input in [&name, &bind_port, &host, &host_port] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |view: &mut Self, _, _: &gpui_kit::component::input::InputEvent, _, cx| {
                    cx.notify();
                    let _ = view;
                },
            ));
        }

        // The rows are read through a map nobody notifies this view about, so it looks
        // once a second and repaints only when the answer moved. Half the resource
        // panel's rate: a tunnel changes state when the SSH layer says so, which is not
        // a rate at all.
        let watched = tunnels.clone();
        let watched_tab = tab.clone();
        let ticker = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let rows = live_rows(&watched, watched_tab.as_deref());
            let moved = this
                .update_in(cx, |view, _, cx| {
                    if keys(&view.shown) != keys(&rows) {
                        view.shown = rows;
                        cx.notify();
                    }
                })
                .is_err();
            if moved {
                break;
            }
        });

        let shown = live_rows(&tunnels, tab.as_deref());
        Self {
            session,
            drafts: vec![tunnel::blank_draft()],
            name,
            bind_port,
            host,
            host_port,
            error: None,
            shown,
            pending: None,
            _subscriptions: subscriptions,
            _ticker: ticker,
        }
    }

    /// Take the next action the user asked for.
    pub(crate) fn take_action(&mut self) -> Option<TunnelAction> {
        self.pending.take()
    }

    /// Start the rule the form describes.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.draft(cx);
        let ok = !draft.bind_port.trim().is_empty()
            || !draft.host.trim().is_empty()
            || !draft.name.trim().is_empty();
        if !ok {
            self.error = Some(
                crate::i18n::t(
                    "填写监听端口和目标后即可启动。",
                    "Fill in a listen port and a target.",
                )
                .to_string(),
            );
            cx.notify();
            return;
        }
        match tunnel::validate(std::slice::from_ref(&draft)) {
            Ok(mut forwards) => {
                let rule = forwards.remove(0);
                self.pending = Some(TunnelAction::Start(rule));
                self.error = None;
                for input in [&self.name, &self.bind_port, &self.host, &self.host_port] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    /// The form's single row, as typed.
    fn draft(&self, cx: &Context<Self>) -> TunnelDraft {
        TunnelDraft {
            kind: self
                .drafts
                .first()
                .map(|draft| draft.kind.clone())
                .unwrap_or_else(|| "local".to_string()),
            name: self.name.read(cx).value().to_string(),
            bind_addr: "127.0.0.1".to_string(),
            bind_port: self.bind_port.read(cx).value().to_string(),
            host: self.host.read(cx).value().to_string(),
            host_port: self.host_port.read(cx).value().to_string(),
        }
    }

    /// The word for one runtime row's kind.
    fn kind_word(kind: &str) -> &'static str {
        match kind {
            "remote" => crate::i18n::t("远程", "Remote"),
            "dynamic" => crate::i18n::t("动态", "Dynamic"),
            _ => crate::i18n::t("本地", "Local"),
        }
    }

    /// The form: the kind, the listener, and what it forwards to.
    fn form(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // Copied out rather than held as `cx.theme()`: the kind buttons below need
        // `&mut Context`, and a live borrow of `cx` would conflict with them.
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let kind = self
            .drafts
            .first()
            .map(|draft| draft.kind.clone())
            .unwrap_or_else(|| "local".to_string());
        let dynamic = kind == "dynamic";

        let kind_button =
            |id: &'static str, label: &'static str, value: &'static str, cx: &mut Context<Self>| {
                Button::new(id)
                    .label(label)
                    .small()
                    .when(kind == value, |button| button.primary())
                    .when(kind != value, |button| button.ghost())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = this.drafts.first_mut() {
                            draft.kind = value.to_string();
                        }
                        cx.notify();
                    }))
            };

        v_flex()
            .w_full()
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(border)
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::i18n::t("新建隧道", "New tunnel")),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(kind_button(
                        "tunnel-kind-local",
                        crate::i18n::t("本地", "Local"),
                        "local",
                        cx,
                    ))
                    .child(kind_button(
                        "tunnel-kind-remote",
                        crate::i18n::t("远程", "Remote"),
                        "remote",
                        cx,
                    ))
                    .child(kind_button(
                        "tunnel-kind-dynamic",
                        crate::i18n::t("动态", "Dynamic"),
                        "dynamic",
                        cx,
                    ))
                    .child(div().flex_1()),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(div().w(px(180.)).child(Input::new(&self.name)))
                    .child(div().w(px(140.)).child(Input::new(&self.bind_port)))
                    .when(!dynamic, |this| {
                        this.child(div().w(px(200.)).child(Input::new(&self.host)))
                            .child(div().w(px(140.)).child(Input::new(&self.host_port)))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("tunnel-start")
                            .icon(Icon::new(IconName::Play))
                            .label(crate::i18n::t("启动", "Start"))
                            .primary()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                    ),
            )
            .child(div().text_xs().text_color(muted).child(crate::i18n::t(
                "监听地址固定为 127.0.0.1；动态隧道是本地 SOCKS5 代理，不需要目标。",
                "The listener is 127.0.0.1; a dynamic tunnel is a local SOCKS5 proxy \
                         and needs no target.",
            )))
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_xs().text_color(danger).child(error))
            })
            .into_any_element()
    }

    /// One running tunnel: what it is, where it goes, and how to stop it.
    fn row(&self, info: &RuntimeTunnelInfo, cx: &mut Context<Self>) -> AnyElement {
        // Copied out because the buttons below take &mut Context through cx.listener.
        let id = info.id.clone();
        let forward = PortForward {
            kind: info.kind.clone(),
            name: info.name.clone(),
            bind_addr: info.bind_addr.clone(),
            bind_port: info.bind_port,
            host: info.host.clone(),
            host_port: info.host_port,
        };
        let (bind, target) = tunnel::describe(&forward);
        // Copied out because the buttons below take `&mut Context` through `cx.listener`.
        let up = cx.theme().success;
        let muted_fg = cx.theme().muted_foreground;
        let name = if info.name.trim().is_empty() {
            crate::i18n::t("未命名", "Unnamed").to_string()
        } else {
            info.name.clone()
        };

        h_flex()
            .w_full()
            .gap_2()
            .px_2()
            .py_1()
            .items_center()
            .child(
                div()
                    .size_2()
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(if info.active { up } else { muted_fg }),
            )
            .child(div().w(px(160.)).truncate().child(SharedString::from(name)))
            .child(
                div()
                    .w(px(48.))
                    .text_xs()
                    .text_color(muted_fg)
                    .child(Self::kind_word(&info.kind)),
            )
            .child(div().text_xs().child(SharedString::from(bind)))
            .when(!target.is_empty(), |this| {
                this.child(
                    Icon::new(IconName::ArrowRight)
                        .size_3()
                        .text_color(muted_fg),
                )
                .child(div().text_xs().child(SharedString::from(target)))
            })
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(SharedString::from(info.status.clone())),
            )
            .child(
                Button::new(SharedString::from(format!("tunnel-stop-{}", info.id)))
                    .icon(Icon::new(IconName::Square))
                    .ghost()
                    .small()
                    .tooltip(crate::i18n::t("停止隧道", "Stop the tunnel"))
                    .accessibility_label(crate::i18n::t("停止隧道", "Stop the tunnel"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pending = Some(TunnelAction::Stop(id.clone()));
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

impl Render for TunnelsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let form = self.form(cx);
        // Copied out for the same reason as in `form`: the rows below need `&mut Context`.
        let muted = cx.theme().muted_foreground;
        let rows = self.shown.clone();
        let live = rows.iter().filter(|row| row.active).count();

        let heading = match self.session.as_deref() {
            Some(session) => SharedString::from(format!(
                "{session} · {} {live}",
                crate::i18n::t("运行中", "running")
            )),
            None => SharedString::from(crate::i18n::t(
                "没有会话：先在左边打开一个连接。",
                "No session: open a connection on the left first.",
            )),
        };

        let mut list: Vec<AnyElement> = Vec::new();
        if rows.is_empty() {
            list.push(
                div()
                    .py_4()
                    .text_xs()
                    .text_color(muted)
                    .child(crate::i18n::t(
                        "这个会话还没有隧道。",
                        "This session has no tunnels yet.",
                    ))
                    .into_any_element(),
            );
        }
        for info in &rows {
            list.push(self.row(info, cx));
        }

        v_flex()
            .id("tunnels")
            .size_full()
            .overflow_y_scroll()
            .gap_2()
            .p_3()
            .child(div().text_xs().text_color(muted).child(heading))
            .child(form)
            .children(list)
    }
}

/// The rows for one tab, as the terminal view last wrote them.
fn live_rows(tunnels: &TabTunnels, tab: Option<&str>) -> Vec<RuntimeTunnelInfo> {
    let Some(tab) = tab else {
        return Vec::new();
    };
    tunnels
        .lock()
        .ok()
        .and_then(|tunnels| tunnels.get(tab).cloned())
        .unwrap_or_default()
}

/// What a change to the rows looks like: the id, whether it is up, and its status.
///
/// The rows themselves are not compared — `RuntimeTunnelInfo` is the SSH layer's own
/// report and carries no equality — and these three are the whole of what this panel
/// draws from them.
fn keys(rows: &[RuntimeTunnelInfo]) -> Vec<(String, bool, String)> {
    rows.iter()
        .map(|row| (row.id.clone(), row.active, row.status.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::TestAppContext;

    /// Fills the form the way a user would, then presses Start.
    ///
    /// `start` is private and this is the same module, which is the point: the panel's whole
    /// contract with the session is what it puts in `pending` and what it says in `error`,
    /// and neither needs a button to be found on screen.
    fn describe(
        view: &mut TunnelsView,
        window: &mut Window,
        cx: &mut Context<TunnelsView>,
        name: &str,
        bind: &str,
        host: &str,
        host_port: &str,
    ) {
        for (field, value) in [
            (&view.name, name),
            (&view.bind_port, bind),
            (&view.host, host),
            (&view.host_port, host_port),
        ] {
            field.clone().update(cx, |state, cx| {
                state.set_value(gpui_kit::SharedString::from(value.to_string()), window, cx);
            });
        }
        view.start(window, cx);
    }

    /// The form refuses an empty rule and says so, rather than asking the session to bring up
    /// a tunnel to nowhere.
    #[gpui_kit::gpui::test]
    fn an_empty_form_reports_an_error_and_starts_nothing(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            TunnelsView::new(
                Rc::new(RefCell::new(
                    crate::config::ConfigStore::load().expect("a configuration"),
                )),
                Some("tab".into()),
                Some("user@host".into()),
                TabTunnels::default(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        cx.update(|window, cx| {
            view.update(cx, |panel, cx| {
                describe(panel, window, cx, "", "", "", "");
            });
        });

        let (error, pending) = view.read_with(cx, |panel, _| {
            (panel.error.clone(), panel.pending.is_some())
        });
        assert!(error.is_some(), "an empty form explains itself");
        assert!(!pending, "and does not start a tunnel to nowhere");
    }

    /// And a filled one becomes the action the shell turns into a real forward, carrying the
    /// ports the user typed.
    #[gpui_kit::gpui::test]
    fn a_filled_form_reports_the_forward_it_describes(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            TunnelsView::new(
                Rc::new(RefCell::new(
                    crate::config::ConfigStore::load().expect("a configuration"),
                )),
                Some("tab".into()),
                Some("user@host".into()),
                TabTunnels::default(),
                window,
                cx,
            )
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        cx.update(|window, cx| {
            view.update(cx, |panel, cx| {
                describe(panel, window, cx, "web", "8080", "10.0.0.5", "80");
            });
        });

        let action = view.update(cx, |panel, _| panel.take_action());
        match action {
            Some(TunnelAction::Start(forward)) => {
                assert_eq!(forward.bind_port, 8080, "the listen port the user typed");
                assert_eq!(forward.host, "10.0.0.5", "and the target it forwards to");
                assert_eq!(forward.host_port, 80);
            }
            other => panic!("expected a start, got {other:?}"),
        }
    }
}
