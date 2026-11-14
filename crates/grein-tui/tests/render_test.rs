use grein_tui::layout::compute_layout;
use ratatui::layout::Rect;

#[test]
fn compute_layout_allocates_34_46_20_percentages_when_expanded_and_34_66_when_collapsed() {
    let area = Rect::new(0, 0, 120, 40);
    let expanded = compute_layout(area, false);
    let collapsed = compute_layout(area, true);

    assert_eq!(expanded.tree_area.width, 41);
    assert_eq!(expanded.transcript_area.width, 55);
    assert_eq!(expanded.blackboard_area.expect("blackboard area").width, 24);
    assert!(expanded.transcript_area.width > expanded.tree_area.width);

    assert_eq!(collapsed.tree_area.width, 41);
    assert_eq!(collapsed.transcript_area.width, 79);
    assert!(collapsed.blackboard_area.is_none());
    assert_eq!(collapsed.tree_area.width, expanded.tree_area.width);
}
