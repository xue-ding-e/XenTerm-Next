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

/// Synthetic GPUI bounds, not native font or Windows rendering validation.
#[gpui_kit::gpui::test]
fn storage_numeric_anchors_do_not_move_with_mounts_or_monitor_state(cx: &mut TestAppContext) {
    use crate::session::protocol::ResourceMonitorState as State;
    let (view, statuses, cx) = fixture(cx);
    let initial_total = cx
        .debug_bounds("storage-total")
        .expect("reserved total anchor");
    let initial_available = cx
        .debug_bounds("storage-available")
        .expect("reserved available anchor");
    let initial_root_total = cx
        .debug_bounds("storage-root-total")
        .expect("reserved root total");
    let initial_root_available = cx
        .debug_bounds("storage-root-available")
        .expect("reserved root available");
    let initial_summary = cx.debug_bounds("storage-summary").unwrap();
    assert_eq!(initial_total.size.width, px(100.));
    assert_eq!(initial_available.origin.y - initial_total.origin.y, px(24.));
    for (state, mounts) in [
        (
            State::Available,
            vec![("/data".into(), 50, 100), ("/".into(), 25, 100)],
        ),
        (
            State::Available,
            vec![
                ("/".into(), 0, 100),
                ("/a/long/synthetic/mount/path".into(), 1000, 2000),
                ("/tmp".into(), 1, 2),
            ],
        ),
        (State::Available, vec![]),
        (State::Unavailable, vec![]),
        (State::Stale, vec![]),
        (State::Waiting, vec![]),
    ] {
        {
            let mut map = statuses.lock().unwrap();
            let status = map.get_mut("remote").unwrap();
            status.monitor_state = state;
            status.sampled_at = Some(std::time::Instant::now());
            status.mem_total_kib = 4096;
            status.local_storage = (!mounts.is_empty()).then_some((50, 200));
            status.disks = mounts;
        }
        view.update(cx, |view, cx| view.refresh(cx));
        draw(cx);
        view.read_with(cx, |view, _| {
            if state != State::Available {
                assert!(view.shown.local_storage.is_none());
            }
        });
        assert_eq!(cx.debug_bounds("storage-total").unwrap(), initial_total);
        assert_eq!(
            cx.debug_bounds("storage-available").unwrap(),
            initial_available
        );
        assert_eq!(
            cx.debug_bounds("storage-root-total").unwrap(),
            initial_root_total
        );
        assert_eq!(
            cx.debug_bounds("storage-root-available").unwrap(),
            initial_root_available
        );
        assert_eq!(cx.debug_bounds("storage-summary").unwrap(), initial_summary);
    }
}

#[gpui_kit::gpui::test]
fn storage_numeric_columns_keep_their_width_and_right_alignment_on_resize(cx: &mut TestAppContext) {
    let (_, _, cx) = fixture(cx);
    let mut top = None;
    for width in [200., 260., 360., 480., 200.] {
        cx.simulate_resize(gpui_kit::size(px(width), px(900.)));
        draw(cx);
        let total = cx.debug_bounds("storage-total").unwrap();
        let available = cx.debug_bounds("storage-available").unwrap();
        for id in ["storage-root-total", "storage-root-available"] {
            let root = cx.debug_bounds(id).unwrap();
            assert_eq!(root.size.width, total.size.width);
            assert_eq!(root.origin.x, total.origin.x);
        }
        let summary = cx.debug_bounds("storage-summary").unwrap();
        assert_eq!(total.size.width, px(100.));
        assert_eq!(available.size.width, total.size.width);
        assert_eq!(total.origin.x, available.origin.x);
        assert_eq!(
            total.origin.x + total.size.width,
            summary.origin.x + summary.size.width
        );
        assert_eq!(*top.get_or_insert(total.origin.y), total.origin.y);
    }
}

#[gpui_kit::gpui::test]
fn storage_projection_keeps_root_scope_separate_from_aggregate_and_stale_samples(
    cx: &mut TestAppContext,
) {
    use crate::session::protocol::ResourceMonitorState as State;
    let (view, statuses, cx) = fixture(cx);
    for (mounts, aggregate, expected_root) in [
        (
            vec![("/data".into(), 25, 200), ("/".into(), 50, 100)],
            Some((75, 300)),
            Some((50, 100)),
        ),
        (vec![("/data".into(), 25, 200)], Some((25, 200)), None),
        (vec![("/".into(), 0, 100)], None, Some((0, 100))),
        (
            vec![("/".into(), 50, 100), ("/".into(), 25, 100)],
            None,
            None,
        ),
        (vec![("/".into(), 0, 0)], None, None),
    ] {
        {
            let mut map = statuses.lock().unwrap();
            let status = map.get_mut("remote").unwrap();
            status.monitor_state = State::Available;
            status.sampled_at = Some(std::time::Instant::now());
            status.mem_total_kib = 4096;
            status.local_storage = aggregate;
            status.disks = mounts;
        }
        view.update(cx, |view, cx| view.refresh(cx));
        draw(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.shown.local_storage, aggregate);
            assert_eq!(
                crate::resource::root_disk(&view.shown.disks).and_then(|disk| disk.capacity),
                expected_root
            );
            if expected_root.is_some() {
                assert_eq!(view.shown.disks[0].path, "/");
            }
        });
    }
    // Retained store data must not leak through after the streaming sample expires.
    {
        let mut map = statuses.lock().unwrap();
        let status = map.get_mut("remote").unwrap();
        status.local_storage = Some((75, 300));
        status.disks = vec![("/".into(), 50, 100)];
        status.sampled_at = Some(std::time::Instant::now() - Duration::from_secs(11));
    }
    view.update(cx, |view, cx| view.refresh(cx));
    draw(cx);
    view.read_with(cx, |view, _| {
        assert!(view.shown.local_storage.is_none());
        assert!(crate::resource::root_disk(&view.shown.disks).is_none());
    });
}
