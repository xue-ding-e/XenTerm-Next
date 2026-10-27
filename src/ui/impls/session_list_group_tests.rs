//! Real group interactions, isolated from the user's profile and credentials.

use super::tests::{draw, index, list, open, session, store};
use super::*;
use gpui_kit::component::list::ListEvent;
use gpui_kit::gpui::{
    Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext, VisualTestContext,
};
use std::cell::RefCell;

struct Fixture {
    directory: tempfile::TempDir,
    store: Rc<RefCell<ConfigStore>>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = store(vec![
            session("alpha", "Alpha", "ops"),
            session("beta", "Beta", "ops"),
            session("gamma", "Gamma", "z"),
        ]);
        {
            let mut store = store.borrow_mut();
            store.path = directory.path().join("sessions.db");
            store.cache.groups.push("empty".into());
            store.saved_state = std::sync::Mutex::new(crate::config::SavedState::default()).into();
            store.save().unwrap();
        }
        Self { directory, store }
    }

    fn saved_groups(&self) -> Option<Vec<String>> {
        let connection =
            rusqlite::Connection::open(self.directory.path().join("sessions.db")).unwrap();
        let settings: String = connection
            .query_row("SELECT value FROM meta WHERE key = 'settings'", [], |row| {
                row.get(0)
            })
            .unwrap();
        serde_json::from_str::<serde_json::Value>(&settings).unwrap()["collapsed_session_groups"]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .map(|group| group.as_str().unwrap().to_string())
                    .collect()
            })
    }
}

fn open_fixture<'a>(
    cx: &'a mut TestAppContext,
    fixture: &Fixture,
    compact: bool,
) -> (Entity<SessionListView>, &'a mut VisualTestContext) {
    let (view, cx) = open(cx, fixture.store.clone(), compact);
    cx.update(|window, _| {
        window.resize(gpui_kit::size(px(480.), px(1500.)));
        window.activate_window();
    });
    draw(cx);
    (view, cx)
}

fn click_header(group: &str, cx: &mut VisualTestContext) {
    let selector = match group {
        "system" => "group-hdr-system",
        "ops" => "group-hdr-ops",
        "empty" => "group-hdr-empty",
        _ => panic!("unknown fixture group: {group}"),
    };
    let bounds = cx.debug_bounds(selector).expect("visible group header");
    cx.simulate_click(bounds.center(), Modifiers::default());
    draw(cx);
}

fn observe_confirms(
    list: &Entity<ListState<SessionListDelegate>>,
    cx: &mut VisualTestContext,
) -> (Rc<RefCell<Vec<Option<String>>>>, gpui_kit::Subscription) {
    let confirms = Rc::new(RefCell::new(Vec::new()));
    let recorded = confirms.clone();
    let subscription = cx.update(|_, cx| {
        cx.subscribe(list, move |list, event: &ListEvent, cx| {
            if let ListEvent::Confirm(ix) = event {
                recorded
                    .borrow_mut()
                    .push(list.read(cx).delegate().connectable_at(*ix));
            }
        })
    });
    (confirms, subscription)
}

#[gpui_kit::gpui::test]
fn full_group_headers_and_session_rows_keep_equal_heights_after_each_fold(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (_, cx) = open_fixture(cx, &fixture, false);
    let system = cx.debug_bounds("group-hdr-system").unwrap();
    let expanded = cx.debug_bounds("group-hdr-ops").unwrap();
    let row = cx.debug_bounds("row-menu-alpha").unwrap();
    let next_row = cx.debug_bounds("row-menu-beta").unwrap();
    assert_eq!(system.size.height, px(FULL_ROW_HEIGHT));
    assert_eq!(expanded.size.height, system.size.height);
    assert_eq!(row.size.height, system.size.height);
    assert_eq!(
        row.top(),
        expanded.bottom(),
        "no reserved section-header gap"
    );
    assert_eq!(next_row.top(), row.bottom(), "session slots remain uniform");
    for _ in 0..2 {
        click_header("ops", cx);
        let folded = cx.debug_bounds("group-hdr-ops").unwrap();
        let following = cx.debug_bounds("group-hdr-z").unwrap();
        assert_eq!(
            folded, expanded,
            "the group header must not change shape or position"
        );
        assert_eq!(
            following.top(),
            folded.bottom(),
            "no invisible folded-row or header spacer"
        );
        assert!(cx.debug_bounds("row-menu-alpha").is_none());
        click_header("ops", cx);
        assert_eq!(cx.debug_bounds("group-hdr-ops").unwrap(), expanded);
        assert_eq!(
            cx.debug_bounds("row-menu-alpha").unwrap().top(),
            expanded.bottom()
        );
    }
}

#[gpui_kit::gpui::test]
fn full_system_header_click_double_click_and_enter_never_connect(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (view, cx) = open_fixture(cx, &fixture, false);
    let list = list(&view, cx);
    let (confirms, _subscription) = observe_confirms(&list, cx);
    let position = cx.debug_bounds("group-hdr-system").unwrap().center();
    click_header("system", cx);
    assert_eq!(fixture.saved_groups(), Some(vec!["system".into()]));
    cx.simulate_event(MouseDownEvent {
        position,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        position,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
    });
    draw(cx);
    assert_eq!(fixture.saved_groups(), Some(vec![]));
    assert!(
        confirms.borrow().is_empty(),
        "mouse headers consume the outer list Confirm"
    );
    let header = cx.update(|window, cx| {
        list.update(cx, |state, cx| {
            let section = state
                .delegate()
                .sections
                .iter()
                .position(|span| span.group == "system")
                .unwrap();
            let header = IndexPath::new(0).section(section);
            state.set_selected_index(Some(header), window, cx);
            state.focus(window, cx);
            header
        })
    });
    for expected in [vec!["system".to_string()], vec![]] {
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(fixture.saved_groups(), Some(expected));
        cx.update(|_, cx| {
            assert_eq!(list.read(cx).selected_index(), Some(header));
            assert!(list.read(cx).delegate().connectable_at(header).is_none());
        });
    }
    assert_eq!(
        *confirms.borrow(),
        vec![None, None],
        "keyboard Confirm must never resolve to a session"
    );
    let alpha = cx.debug_bounds("row-menu-alpha").unwrap();
    cx.simulate_click(alpha.center(), Modifiers::default());
    draw(cx);
    assert_eq!(
        confirms.borrow().last(),
        Some(&Some("alpha".into())),
        "real session rows still connect"
    );
}

#[gpui_kit::gpui::test]
fn full_search_temporarily_opens_groups_and_clearing_restores_saved_folds(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    assert!(persist_group_fold(&fixture.store, "ops", true));
    let (view, cx) = open_fixture(cx, &fixture, false);
    let list = list(&view, cx);
    let (confirms, _subscription) = observe_confirms(&list, cx);
    let original = fixture.saved_groups();
    cx.update(|window, cx| view.update(cx, |view, cx| view.focus_search(window, cx)));
    cx.simulate_input("Alpha");
    draw(cx);
    assert!(cx.debug_bounds("row-menu-alpha").is_some());
    assert!(cx.debug_bounds("row-menu-beta").is_none());
    assert_eq!(
        cx.update(|_, cx| list.read(cx).delegate().query.clone()),
        "Alpha"
    );
    click_header("ops", cx);
    assert_eq!(fixture.saved_groups(), original);
    assert!(confirms.borrow().is_empty());
    assert_eq!(
        cx.update(|_, cx| list.read(cx).selected_index()),
        Some(index(&list, "alpha", cx))
    );
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert_eq!(
        *confirms.borrow(),
        vec![Some("alpha".into())],
        "Enter after typing selects a real search match, not its header"
    );
    // Confirm may move input focus; clearing is an explicit edit of the
    // search box, as when the user returns from an opened session.
    cx.update(|window, cx| view.update(cx, |view, cx| view.focus_search(window, cx)));
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a backspace"
    } else {
        "ctrl-a backspace"
    });
    draw(cx);
    assert_eq!(
        cx.update(|_, cx| list.read(cx).delegate().query.clone()),
        ""
    );
    assert!(cx.debug_bounds("row-menu-alpha").is_none());
    assert!(cx.debug_bounds("group-hdr-ops").is_some());
    assert_eq!(fixture.saved_groups(), original);
    assert!(cx
        .update(|_, cx| list.read(cx).delegate().index_for_session("alpha"))
        .is_none());
}

#[gpui_kit::gpui::test]
fn full_empty_group_is_one_real_header_and_never_a_connectable_placeholder(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let (view, cx) = open_fixture(cx, &fixture, false);
    let list = list(&view, cx);
    let (confirms, _subscription) = observe_confirms(&list, cx);
    for _ in 0..3 {
        cx.update(|_, cx| {
            let delegate = list.read(cx).delegate();
            let section = delegate
                .sections
                .iter()
                .position(|span| span.group == "empty")
                .unwrap();
            assert_eq!(delegate.items_count(section, cx), 1);
            for row in 0..3 {
                let ix = IndexPath::new(row).section(section);
                assert!(delegate.row_at(ix).is_none());
                assert!(delegate.connectable_at(ix).is_none());
            }
        });
        assert_eq!(
            cx.debug_bounds("group-hdr-empty").unwrap().size.height,
            px(FULL_ROW_HEIGHT)
        );
        click_header("empty", cx);
    }
    assert!(confirms.borrow().is_empty());
    assert_eq!(index(&list, "alpha", cx).row, 1);
}

#[gpui_kit::gpui::test]
fn full_failed_fold_save_restores_view_and_reports_persistence_error(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (view, cx) = open_fixture(cx, &fixture, false);
    let list = list(&view, cx);
    let before = fixture
        .store
        .borrow()
        .cache
        .collapsed_session_groups
        .clone();
    let blocker = fixture.directory.path().join("not-a-directory");
    std::fs::write(&blocker, b"fixture").unwrap();
    fixture.store.borrow_mut().path = blocker.join("sessions.db");
    click_header("ops", cx);
    assert_eq!(
        fixture.store.borrow().cache.collapsed_session_groups,
        before
    );
    assert_eq!(fixture.saved_groups(), before);
    assert!(fixture.store.borrow().persistence_error().is_some());
    assert!(cx.debug_bounds("row-menu-alpha").is_some());
    assert_eq!(index(&list, "alpha", cx).row, 1);
}

#[gpui_kit::gpui::test]
fn compact_palette_keeps_all_group_headers_and_candidates_when_sidebar_groups_are_folded(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    for group in ["system", "ops", "empty", "z"] {
        assert!(persist_group_fold(&fixture.store, group, true));
    }
    let saved = fixture.saved_groups();
    let (view, cx) = open_fixture(cx, &fixture, true);
    let list = list(&view, cx);
    let (confirms, _subscription) = observe_confirms(&list, cx);
    let first = cx.debug_bounds("group-hdr-system").unwrap();
    assert!(first.size.height > px(0.));
    for selector in ["group-hdr-ops", "group-hdr-empty", "group-hdr-z"] {
        assert_eq!(
            cx.debug_bounds(selector).unwrap().size.height,
            first.size.height
        );
    }
    let ops = cx.debug_bounds("group-hdr-ops").unwrap();
    assert_eq!(
        cx.debug_bounds("row-menu-alpha").unwrap().top(),
        ops.bottom()
    );
    assert!(cx.debug_bounds("row-menu-beta").is_some());
    assert!(cx.debug_bounds("row-menu-gamma").is_some());
    for group in ["system", "ops", "empty"] {
        click_header(group, cx);
    }
    assert_eq!(
        fixture.saved_groups(),
        saved,
        "palette dividers never mutate sidebar folds"
    );
    assert!(confirms.borrow().is_empty());
    assert_eq!(
        index(&list, "alpha", cx).row,
        0,
        "compact palette keeps its existing index space"
    );
}

fn drag_header(source: &'static str, target: &'static str, cx: &mut VisualTestContext) {
    let source = cx.debug_bounds(source).expect("source header").center();
    let target = cx.debug_bounds(target).expect("target header").center();
    cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(source + gpui_kit::point(px(12.), px(0.)), MouseButton::Left, Modifiers::default());
    draw(cx);
    cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    draw(cx);
    cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    draw(cx);
}

#[gpui_kit::gpui::test]
fn group_order_drag_reorders_headers_without_folding_or_connecting(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (view, cx) = open_fixture(cx, &fixture, false);
    let list = list(&view, cx);
    let (confirms, _subscription) = observe_confirms(&list, cx);
    let folds = fixture.saved_groups();
    drag_header("group-hdr-z", "group-hdr-system", cx);
    assert_eq!(fixture.store.borrow().ordered_session_groups(), vec!["z", "system", "empty", "ops"]);
    assert!(cx.debug_bounds("group-hdr-z").unwrap().top() < cx.debug_bounds("group-hdr-system").unwrap().top());
    assert_eq!(fixture.saved_groups(), folds);
    assert!(confirms.borrow().is_empty(), "dragging a header never connects a session");
    assert_eq!(fixture.store.borrow().sessions()[0].group, "ops");
    drag_header("group-hdr-z", "group-hdr-ops", cx);
    assert_eq!(fixture.store.borrow().ordered_session_groups(), vec!["system", "empty", "ops", "z"]);
}

#[gpui_kit::gpui::test]
fn group_order_drag_outside_a_target_cancels_without_saving(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (_, cx) = open_fixture(cx, &fixture, false);
    let before = fixture.store.borrow().session_group_order().map(<[_]>::to_vec);
    let folds = fixture.saved_groups();
    let source = cx.debug_bounds("group-hdr-ops").unwrap().center();
    cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(source + gpui_kit::point(px(12.), px(0.)), MouseButton::Left, Modifiers::default());
    draw(cx);
    let outside = gpui_kit::point(px(470.), px(1490.));
    cx.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    draw(cx);
    cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    draw(cx);
    assert_eq!(fixture.store.borrow().session_group_order(), before.as_deref());
    assert_eq!(fixture.saved_groups(), folds);
}

#[gpui_kit::gpui::test]
fn group_order_drag_rejects_a_stale_source(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (_, cx) = open_fixture(cx, &fixture, false);
    let source = cx.debug_bounds("group-hdr-empty").unwrap().center();
    let target = cx.debug_bounds("group-hdr-system").unwrap().center();
    cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(source + gpui_kit::point(px(12.), px(0.)), MouseButton::Left, Modifiers::default());
    draw(cx);
    fixture.store.borrow_mut().remove_group("empty");
    cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    draw(cx);
    cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    draw(cx);
    assert!(fixture.store.borrow().session_group_order().is_none());
    assert!(!fixture.store.borrow().groups().iter().any(|group| group == "empty"));
}

#[gpui_kit::gpui::test]
fn group_order_drag_save_failure_keeps_visible_and_cached_order(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (_, cx) = open_fixture(cx, &fixture, false);
    let before = fixture.store.borrow().ordered_session_groups();
    let top = cx.debug_bounds("group-hdr-system").unwrap().top();
    // A directory cannot become the settings database; no user path is touched.
    fixture.store.borrow_mut().path = fixture.directory.path().to_path_buf();
    drag_header("group-hdr-z", "group-hdr-system", cx);
    assert_eq!(fixture.store.borrow().ordered_session_groups(), before);
    assert!(fixture.store.borrow().session_group_order().is_none());
    assert_eq!(cx.debug_bounds("group-hdr-system").unwrap().top(), top);
}
