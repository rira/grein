use grein_tui::ui::focus::{FocusNavigation, PaneFocus};

#[test]
fn test_pane_focus_tree_variant_exists_and_traits() {
    let focus = PaneFocus::Tree;
    let cloned = focus;
    assert_eq!(focus, cloned);
    assert_eq!(format!("{:?}", focus), "Tree");
}

#[test]
fn test_focus_navigation_includes_tree_in_tab_cycle() {
    let mut nav = FocusNavigation::default();

    // Cycle forward through the 3-pane navigation sequence
    let mut visited = Vec::new();
    for _ in 0..3 {
        visited.push(nav.current());
        nav.next();
    }

    assert!(
        visited.contains(&PaneFocus::Tree),
        "PaneFocus::Tree must be registered in the forward Tab focus navigation cycle"
    );
    assert_eq!(
        nav.current(),
        visited[0],
        "Focus navigation should wrap around after 3 Tab advances"
    );
}

#[test]
fn test_focus_navigation_includes_tree_in_backtab_cycle() {
    let mut nav = FocusNavigation::default();

    // Cycle backward through the 3-pane navigation sequence
    let mut visited = Vec::new();
    for _ in 0..3 {
        visited.push(nav.current());
        nav.prev();
    }

    assert!(
        visited.contains(&PaneFocus::Tree),
        "PaneFocus::Tree must be registered in the backward BackTab focus navigation cycle"
    );
    assert_eq!(
        nav.current(),
        visited[0],
        "Focus navigation should wrap around after 3 BackTab steps"
    );
}

#[test]
fn test_focus_navigation_set_tree_focus_explicitly() {
    let mut nav = FocusNavigation::default();
    nav.set_focus(PaneFocus::Tree);
    assert_eq!(nav.current(), PaneFocus::Tree);

    nav.next();
    assert_ne!(nav.current(), PaneFocus::Tree);

    nav.prev();
    assert_eq!(nav.current(), PaneFocus::Tree);
}
