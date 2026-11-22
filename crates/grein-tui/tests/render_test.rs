use grein_tui::layout::compute_layout;
use ratatui::layout::Rect;

#[test]
fn compute_layout_allocates_34_66_two_column_split() {
    let area = Rect::new(0, 0, 120, 40);
    let layout = compute_layout(area);

    assert_eq!(layout.tree_area.width, 41);
    assert_eq!(layout.transcript_area.width, 79);
    assert_eq!(layout.tree_area.width + layout.transcript_area.width, 120);
    assert!(layout.transcript_area.width > layout.tree_area.width);
    assert_eq!(layout.composer_area.width, 120);
    assert_eq!(layout.composer_area.height, 3);
}
