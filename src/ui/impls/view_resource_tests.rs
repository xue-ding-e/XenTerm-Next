use super::*;
use crate::core::EventSink as _;
use crate::session::protocol::ResourceMonitorState as State;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};

fn fixture(
    cx: &mut TestAppContext,
) -> (
    crate::resource::TabStatuses,
    super::super::event_sink::GpuiEventSink,
    &mut VisualTestContext,
) {
    cx.update(gpui_kit::init);
    let statuses = Arc::new(Mutex::new(HashMap::from([(
        "resource-tab".into(),
        crate::resource::TabStatus {
            state: 1,
            monitor_generation: 7,
            ..Default::default()
        },
    )])));
    let shared = statuses.clone();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let sink =
        super::super::event_sink::GpuiEventSink::new(sender, Arc::new(Mutex::new(HashMap::new())));
    let (_, cx) = cx.add_window_view(move |_, cx| {
        TerminalView::new(
            "resource-tab".into(),
            TerminalSettings {
                family: "test".into(),
                font_size: 13,
                bold: false,
                padding: false,
                line_spacing: 1.0,
                cursor_style: CursorStyle::Block,
                cursor_color: None,
                highlight: crate::terminal::OutputHighlightPreset::Off,
                rules: vec![],
                review_multiline_paste: true,
                paste_shortcuts: true,
            },
            Rc::new(std::cell::RefCell::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
            shared,
            Arc::new(Mutex::new(crate::core::TransferStore::new())),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(None)),
            receiver,
            None,
            cx,
        )
    });
    cx.run_until_parked();
    (statuses, sink, cx)
}

fn sample(cpu_sampled: bool) -> SessionEvent {
    SessionEvent::ResourceStats {
        cpu_sampled,
        cpu_percent: 0.0,
        mem_used_kib: 1024,
        mem_total_kib: 4096,
        swap_used_kib: 0,
        swap_total_kib: 0,
        net: vec![("eth0".into(), 0, 0)],
        disks: vec![],
        current_user: String::new(),
        procs: vec![],
        sys: None,
    }
}

fn status(state: State) -> SessionEvent {
    SessionEvent::ResourceMonitorStatus { state }
}

#[gpui_kit::gpui::test]
fn resource_events_distinguish_initial_baseline_zero_failure_and_pause(cx: &mut TestAppContext) {
    let (statuses, sink, cx) = fixture(cx);
    sink.deliver_for_generation("resource-tab", 7, vec![sample(false)]);
    cx.run_until_parked();
    {
        let map = statuses.lock().unwrap();
        assert_eq!(map["resource-tab"].monitor_state, State::Available);
        assert!(!map["resource-tab"].cpu_sampled);
        assert!(map["resource-tab"].sampled_at.is_some());
    }
    sink.deliver_for_generation("resource-tab", 7, vec![sample(true)]);
    cx.run_until_parked();
    assert!(statuses.lock().unwrap()["resource-tab"].cpu_sampled);
    assert_eq!(statuses.lock().unwrap()["resource-tab"].cpu, 0.0);
    sink.deliver_for_generation("resource-tab", 7, vec![status(State::Unavailable)]);
    cx.run_until_parked();
    assert_eq!(
        statuses.lock().unwrap()["resource-tab"].monitor_state,
        State::Unavailable
    );
    sink.deliver_for_generation("resource-tab", 7, vec![status(State::Paused), sample(true)]);
    cx.run_until_parked();
    assert_eq!(
        statuses.lock().unwrap()["resource-tab"].monitor_state,
        State::Paused
    );
    sink.deliver_for_generation(
        "resource-tab",
        7,
        vec![status(State::Waiting), sample(false)],
    );
    cx.run_until_parked();
    assert_eq!(
        statuses.lock().unwrap()["resource-tab"].monitor_state,
        State::Available
    );
    assert!(!statuses.lock().unwrap()["resource-tab"].cpu_sampled);
}

#[gpui_kit::gpui::test]
fn resource_events_reject_old_connection_and_do_not_reclassify_system_details(
    cx: &mut TestAppContext,
) {
    let (statuses, sink, cx) = fixture(cx);
    sink.deliver_for_generation(
        "resource-tab",
        6,
        vec![sample(true), status(State::Unavailable)],
    );
    cx.run_until_parked();
    assert_eq!(
        statuses.lock().unwrap()["resource-tab"].monitor_state,
        State::Waiting
    );
    assert_eq!(statuses.lock().unwrap()["resource-tab"].mem_total_kib, 0);
    sink.deliver_for_generation("resource-tab", 7, vec![status(State::Unavailable)]);
    let mut details = sample(false);
    if let SessionEvent::ResourceStats { sys, .. } = &mut details {
        *sys = Some(Default::default());
    }
    sink.deliver_for_generation("resource-tab", 7, vec![details]);
    cx.run_until_parked();
    assert_eq!(
        statuses.lock().unwrap()["resource-tab"].monitor_state,
        State::Unavailable
    );
    assert!(statuses.lock().unwrap()["resource-tab"]
        .sampled_at
        .is_none());
    sink.deliver_for_generation(
        "resource-tab",
        7,
        vec![
            SessionEvent::Closed("synthetic disconnect".into()),
            sample(true),
        ],
    );
    cx.run_until_parked();
    assert_eq!(statuses.lock().unwrap()["resource-tab"].state, 2);
    assert!(statuses.lock().unwrap()["resource-tab"]
        .sampled_at
        .is_none());
    // Seed the replacement connection just as SessionState::connect does. The
    // same terminal queue may still contain old monitor callbacks.
    statuses.lock().unwrap().insert(
        "resource-tab".into(),
        crate::resource::TabStatus {
            monitor_generation: 8,
            state: 1,
            ..Default::default()
        },
    );
    sink.deliver_for_generation(
        "resource-tab",
        8,
        vec![SessionEvent::Connected, sample(true)],
    );
    // This late close is the old worker's, even when already queued after the
    // new Connected. Consumption must reject it along with its resource data.
    sink.deliver_for_generation(
        "resource-tab",
        7,
        vec![
            SessionEvent::Closed("old connection finished".into()),
            sample(true),
            status(State::Unavailable),
        ],
    );
    cx.run_until_parked();
    assert_eq!(
        statuses.lock().unwrap()["resource-tab"].monitor_state,
        State::Available
    );
    assert_eq!(statuses.lock().unwrap()["resource-tab"].mem_total_kib, 4096);
    assert_eq!(statuses.lock().unwrap()["resource-tab"].state, 1);
}
