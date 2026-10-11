//! Click real row buttons at each measured row, including rows added by Duplicate.
use super::*;
use gpui_kit::gpui::{TestAppContext, VisualTestContext};
use gpui_kit::point;
use gpui_kit::test::TestWindowExt as _;

fn fixture(
    cx: &mut TestAppContext,
) -> (
    Entity<QuickManagerView>,
    tempfile::TempDir,
    &mut VisualTestContext,
) {
    cx.update(gpui_kit::init);
    let directory = tempfile::tempdir().unwrap();
    let cache = crate::config::ConfigFile {
        quick_commands: ["A", "B"]
            .into_iter()
            .map(|name| QuickCommand {
                name: name.into(),
                command: format!("echo synthetic-{name}"),
                group: "qa-quick-group".into(),
                send_enter: true,
            })
            .collect(),
        ..Default::default()
    };
    let store = Rc::new(RefCell::new(ConfigStore {
        path: directory.path().join("synthetic-quick.db"),
        backup_dir: None,
        key: [0; 32],
        keyring_enabled: false,
        saved_state: std::sync::Mutex::new(crate::config::SavedState::of_cache(&cache)).into(),
        cache,
    }));
    let (view, cx) = cx.add_window_view(|window, cx| {
        window.activate_window();
        QuickManagerView::new(store, window, cx)
    });
    draw(cx);
    (view, directory, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn row_click(action: &'static str, row: &'static str, cx: &mut VisualTestContext) {
    let button = cx.update(|window, _| {
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .find(|element| element.path().last() == Some(&action.into()))
            .expect("rendered action button")
            .bounds()
    });
    let bounds = cx.debug_bounds(row).expect("rendered target row");
    // The buttons share column positions. Read that column from the real
    // button, and its row from debug geometry (which does not create an ID).
    // This also exercises the baseline where sibling button IDs collide.
    cx.simulate_click(
        point(button.center().x, bounds.center().y),
        Default::default(),
    );
    draw(cx);
}

fn cancel(cx: &mut VisualTestContext) {
    let bounds = cx.update(|window, _| window.find("quick-cancel").bounds());
    cx.simulate_click(bounds.center(), Default::default());
    draw(cx);
}

fn names(view: &Entity<QuickManagerView>, cx: &VisualTestContext) -> Vec<String> {
    view.read_with(cx, |view, _| {
        view.store
            .borrow()
            .quick_commands()
            .iter()
            .map(|row| row.name.clone())
            .collect()
    })
}

#[gpui_kit::gpui::test]
fn duplicate_second_row_moves_up_and_down_without_targeting_its_neighbor(cx: &mut TestAppContext) {
    let (view, _directory, cx) = fixture(cx);
    row_click("quick-row-copy", "quick-manager-row-0", cx);
    assert_eq!(names(&view, cx), ["A", "A 2", "B"]);
    cancel(cx);
    row_click("quick-row-up", "quick-manager-row-1", cx);
    assert_eq!(
        names(&view, cx),
        ["A 2", "A", "B"],
        "the second row's up button must move that copy"
    );
    assert_eq!(
        view.read_with(cx, |view, cx| view.name.read(cx).value().to_string()),
        "A 2"
    );
    cancel(cx);
    row_click("quick-row-down", "quick-manager-row-0", cx);
    assert_eq!(names(&view, cx), ["A", "A 2", "B"]);
    assert_eq!(
        view.read_with(cx, |view, cx| view.name.read(cx).value().to_string()),
        "A 2"
    );
    cancel(cx);
    row_click("quick-select", "quick-manager-row-1", cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.name.read(cx).value().to_string()),
        "A 2"
    );
    cancel(cx);
    row_click("quick-row-delete", "quick-manager-row-1", cx);
    assert_eq!(names(&view, cx), ["A", "B"]);
}

#[gpui_kit::gpui::test]
fn edit_duplicate_and_delete_on_later_rows_use_the_clicked_command(cx: &mut TestAppContext) {
    let (view, _directory, cx) = fixture(cx);
    row_click("quick-select", "quick-manager-row-1", cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.name.read(cx).value().to_string()),
        "B",
        "second-row pencil must edit B"
    );
    cancel(cx);
    row_click("quick-row-copy", "quick-manager-row-1", cx);
    assert_eq!(names(&view, cx), ["A", "B", "B 2"]);
    cancel(cx);
    row_click("quick-row-delete", "quick-manager-row-1", cx);
    assert_eq!(names(&view, cx), ["A", "B 2"]);
    row_click("quick-select", "quick-manager-row-1", cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.name.read(cx).value().to_string()),
        "B 2"
    );
}

#[gpui_kit::gpui::test]
fn reordered_rows_in_sorted_groups_rebind_edit_and_delete_to_current_records(
    cx: &mut TestAppContext,
) {
    let (view, _directory, cx) = fixture(cx);
    view.update(cx, |view, cx| {
        view.store.borrow_mut().set_quick_commands(
            [("A", "z-group"), ("B", "a-group"), ("C", "z-group")]
                .into_iter()
                .map(|(name, group)| QuickCommand {
                    name: name.into(),
                    command: format!("echo synthetic-{name}"),
                    group: group.into(),
                    send_enter: true,
                })
                .collect(),
        );
        cx.notify();
    });
    draw(cx);
    // Display order is B / A,C, but the rows retain their source indices.
    row_click("quick-row-up", "quick-manager-row-2", cx);
    assert_eq!(names(&view, cx), ["C", "B", "A"]);
    cancel(cx);
    row_click("quick-select", "quick-manager-row-0", cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.name.read(cx).value().to_string()),
        "C"
    );
    cancel(cx);
    // Delete B, the first displayed row, after a move in the other group.
    row_click("quick-row-delete", "quick-manager-row-1", cx);
    assert_eq!(names(&view, cx), ["C", "A"]);
    row_click("quick-select", "quick-manager-row-1", cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.name.read(cx).value().to_string()),
        "A"
    );
}

#[gpui_kit::gpui::test]
fn interleaved_group_move_keeps_the_editor_on_the_moved_command(cx: &mut TestAppContext) {
    let (view, _directory, cx) = fixture(cx);
    view.update(cx, |view, cx| {
        view.store.borrow_mut().set_quick_commands(
            [("A", "z-group"), ("B", "a-group"), ("C", "z-group")]
                .into_iter()
                .map(|(name, group)| QuickCommand {
                    name: name.into(),
                    command: format!("echo synthetic-{name}"),
                    group: group.into(),
                    send_enter: true,
                })
                .collect(),
        );
        cx.notify();
    });
    draw(cx);
    row_click("quick-row-up", "quick-manager-row-2", cx);
    assert_eq!(names(&view, cx), ["C", "B", "A"]);
    assert_eq!(
        view.read_with(cx, |view, cx| view.name.read(cx).value().to_string()),
        "C",
        "move must keep editing its own command across intervening groups"
    );
    row_click("quick-row-down", "quick-manager-row-0", cx);
    assert_eq!(names(&view, cx), ["A", "B", "C"]);
    assert_eq!(
        view.read_with(cx, |view, cx| view.name.read(cx).value().to_string()),
        "C"
    );
    row_click("quick-row-down", "quick-manager-row-2", cx);
    assert_eq!(names(&view, cx), ["A", "B", "C"]);
    assert_eq!(
        view.read_with(cx, |view, cx| view.name.read(cx).value().to_string()),
        "C"
    );
}
