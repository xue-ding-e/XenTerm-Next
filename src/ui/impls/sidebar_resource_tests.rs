use super::*;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use std::collections::HashMap;

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::Entity<SidebarView>,
    TabStatuses,
    &mut VisualTestContext,
) {
    cx.update(gpui_kit::init);
    let store = ConfigStore {
        path: std::env::temp_dir().join(format!(
            "xenterm-resource-state-{}.db",
            uuid::Uuid::new_v4()
        )),
        backup_dir: None,
        cache: crate::config::ConfigFile::default(),
        key: [7; 32],
        keyring_enabled: false,
        saved_state: Mutex::new(crate::config::SavedState::default()).into(),
    };
    let statuses = Arc::new(Mutex::new(HashMap::from([(
        "remote".into(),
        TabStatus {
            host: "192.0.2.10".into(),
            state: 1,
            ..Default::default()
        },
    )])));
    let shared = statuses.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        let mut view = SidebarView::new(shared, Rc::new(RefCell::new(store)), cx);
        view.set_active(Some("remote".into()), cx);
        view
    });
    draw(cx);
    (view, statuses, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
}

#[gpui_kit::gpui::test]
fn connected_without_a_sample_does_not_present_fabricated_zero_resources(cx: &mut TestAppContext) {
    let (view, _, cx) = fixture(cx);
    view.read_with(cx, |view, _| {
        assert!(
            view.shown.top_waiting,
            "a connected socket is not a resource sample"
        );
        assert_eq!(view.shown.mem_detail.as_ref(), "--");
        assert_eq!(view.shown.swap_detail.as_ref(), "--");
        assert!(
            view.shown.proc_available && view.shown.system_info_available,
            "independent probes must remain accessible"
        );
    });
}

#[gpui_kit::gpui::test]
fn resource_monitor_states_hide_unknown_values_but_keep_independent_probes(
    cx: &mut TestAppContext,
) {
    use crate::session::protocol::ResourceMonitorState as State;
    let (view, statuses, cx) = fixture(cx);
    for state in [
        State::Waiting,
        State::Unavailable,
        State::Paused,
        State::Unsupported,
        State::Stale,
    ] {
        statuses
            .lock()
            .unwrap()
            .get_mut("remote")
            .unwrap()
            .monitor_state = state;
        view.update(cx, |view, cx| view.refresh(cx));
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(!view.shown.resources_available && !view.shown.cpu_available);
            assert!(view.shown.top_waiting);
            assert_eq!(view.shown.mem_detail.as_ref(), "--");
            assert_eq!(view.shown.swap_detail.as_ref(), "--");
            assert!(view.shown.proc_available && view.shown.system_info_available);
            assert!(!view.shown.monitor_text.is_empty());
        });
    }
}

#[gpui_kit::gpui::test]
fn resource_monitor_measured_zero_is_visible_until_sample_expires(cx: &mut TestAppContext) {
    use crate::session::protocol::ResourceMonitorState as State;
    let (view, statuses, cx) = fixture(cx);
    {
        let mut map = statuses.lock().unwrap();
        let status = map.get_mut("remote").unwrap();
        status.monitor_state = State::Available;
        status.sampled_at = Some(std::time::Instant::now());
        status.mem_total_kib = 4096;
        status.cpu_sampled = true;
        status.net = vec![("eth0".into(), 0, 0)];
    }
    view.update(cx, |view, cx| view.refresh(cx));
    draw(cx);
    view.read_with(cx, |view, _| {
        assert!(view.shown.resources_available && view.shown.cpu_available);
        assert_eq!(view.shown.cpu, 0.0);
        assert!(
            !view.shown.top_waiting,
            "measured zero network throughput is still a sample"
        );
        assert_ne!(view.shown.mem_detail.as_ref(), "--");
    });
    // Expiry uses the stored monotonic timestamp, never a test-only forced notification.
    statuses
        .lock()
        .unwrap()
        .get_mut("remote")
        .unwrap()
        .sampled_at = Some(std::time::Instant::now() - Duration::from_secs(11));
    view.update(cx, |view, cx| view.refresh(cx));
    draw(cx);
    view.read_with(cx, |view, _| {
        assert!(!view.shown.resources_available);
        assert_eq!(
            view.shown.monitor_text.as_ref(),
            crate::i18n::t("数据已过期", "Sample is stale")
        );
    });
}

#[test]
fn resource_monitor_initial_silence_and_sample_age_have_distinct_states() {
    use crate::session::protocol::ResourceMonitorState as State;
    let now = std::time::Instant::now();
    let mut status = TabStatus {
        monitor_started_at: Some(now),
        ..Default::default()
    };
    assert_eq!(
        status.resource_state_at(now + Duration::from_secs(9)),
        State::Waiting
    );
    assert_eq!(
        status.resource_state_at(now + Duration::from_secs(10)),
        State::Unavailable
    );
    status.monitor_state = State::Available;
    status.sampled_at = Some(now);
    assert_eq!(
        status.resource_state_at(now + Duration::from_secs(9)),
        State::Available
    );
    assert_eq!(
        status.resource_state_at(now + Duration::from_secs(10)),
        State::Stale
    );
}
