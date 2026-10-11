//! Read the rendered badge after clicking real group headers.
use super::*;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::Entity;

fn fixture(
    cx: &mut TestAppContext,
) -> (
    Entity<QuickCommandsView>,
    Rc<RefCell<ConfigStore>>,
    &mut VisualTestContext,
) {
    cx.update(gpui_kit::init);
    let command = |name: &str, group: &str| crate::config::QuickCommand {
        name: name.into(),
        command: format!("echo synthetic-{name}"),
        group: group.into(),
        send_enter: true,
    };
    let cache = crate::config::ConfigFile {
        // Storage deliberately interleaves groups; rendering sorts the headers.
        quick_commands: vec![
            command("a", "many"),
            command("solo", "one"),
            command("b", "many"),
            command("elsewhere", "other"),
            command("c", "many"),
        ],
        quick_groups: vec!["empty".into(), "many".into(), "one".into(), "other".into()],
        ..Default::default()
    };
    let store = Rc::new(RefCell::new(ConfigStore {
        path: Default::default(),
        backup_dir: None,
        key: [0; 32],
        keyring_enabled: false,
        saved_state: std::sync::Mutex::new(crate::config::SavedState::of_cache(&cache)).into(),
        cache,
    }));
    let (view, cx) =
        cx.add_window_view(|_, _| QuickCommandsView::new(store.clone(), DockEdge::Right, false));
    draw(cx);
    (view, store, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn toggle(selector: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx.debug_bounds(selector).expect("visible group header");
    cx.simulate_click(bounds.center(), Default::default());
    draw(cx);
}

fn badge(group: &str, cx: &mut VisualTestContext) -> Option<String> {
    let id = SharedString::from(format!("quick-group-count-{group}"));
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|badge| badge.label().map(str::to_string))
    })
}

#[gpui_kit::gpui::test]
fn a_folded_single_command_displays_one(cx: &mut TestAppContext) {
    let (_view, _store, cx) = fixture(cx);
    toggle("quick-group-one", cx);
    assert_eq!(badge("one", cx).as_deref(), Some("1"));
    toggle("quick-group-one", cx);
    assert_eq!(
        badge("one", cx),
        None,
        "expanded groups have no folded badge"
    );
}

#[gpui_kit::gpui::test]
fn a_folded_group_counts_its_header_command_and_stops_at_the_next_group(cx: &mut TestAppContext) {
    let (_view, _store, cx) = fixture(cx);
    toggle("quick-group-many", cx);
    toggle("quick-group-other", cx);
    assert_eq!(badge("many", cx).as_deref(), Some("3"));
    assert_eq!(badge("other", cx).as_deref(), Some("1"));
}

#[gpui_kit::gpui::test]
fn an_empty_registered_group_has_no_command_badge(cx: &mut TestAppContext) {
    let (_view, _store, cx) = fixture(cx);
    toggle("quick-group-empty", cx);
    assert_eq!(badge("empty", cx), None);
}

#[gpui_kit::gpui::test]
fn refreshing_a_filtered_command_subset_updates_only_its_folded_group(cx: &mut TestAppContext) {
    let (view, store, cx) = fixture(cx);
    toggle("quick-group-many", cx);
    toggle("quick-group-other", cx);
    // There is no search/filter control in this dock. Exercise the refreshed
    // projection after configuration changes remove a subset of commands.
    let subset = store
        .borrow()
        .quick_commands()
        .iter()
        .filter(|command| command.name != "a" && command.name != "c")
        .cloned()
        .collect();
    store.borrow_mut().set_quick_commands(subset);
    view.update(cx, |view, cx| view.refresh(cx));
    draw(cx);
    assert_eq!(badge("many", cx).as_deref(), Some("1"));
    assert_eq!(badge("other", cx).as_deref(), Some("1"));
    let subset = store
        .borrow()
        .quick_commands()
        .iter()
        .filter(|command| command.group != "many")
        .cloned()
        .collect();
    store.borrow_mut().set_quick_commands(subset);
    view.update(cx, |view, cx| view.refresh(cx));
    draw(cx);
    assert_eq!(badge("many", cx), None);
    assert_eq!(badge("other", cx).as_deref(), Some("1"));
}
