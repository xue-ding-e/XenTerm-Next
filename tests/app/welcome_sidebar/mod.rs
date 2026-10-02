use super::update_welcome_tab;
use crate::layout::Layout;

#[test]
fn welcome_tab_can_toggle_from_an_empty_layout_without_duplicates() {
    let mut layout = Layout::new(Vec::new(), String::new());

    update_welcome_tab(&mut layout, false);
    update_welcome_tab(&mut layout, false);
    let pane = &layout.flatten(0.0, 0.0, 800.0, 600.0).0[0];
    assert_eq!(pane.tabs, ["welcome"]);
    assert_eq!(pane.active, "welcome");

    update_welcome_tab(&mut layout, true);
    update_welcome_tab(&mut layout, true);
    assert!(layout.leaf_of_tab("welcome").is_none());
}

#[test]
fn hiding_welcome_preserves_open_session_tabs() {
    let mut layout = Layout::new(vec!["welcome".into(), "session-1".into()], "welcome".into());

    update_welcome_tab(&mut layout, true);
    let pane = &layout.flatten(0.0, 0.0, 800.0, 600.0).0[0];
    assert_eq!(pane.tabs, ["session-1"]);
    assert_eq!(pane.active, "session-1");
}
