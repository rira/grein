use grein_tui::layout::{compute_layout, COMPOSER_HEIGHT};
use ratatui::layout::Rect;

#[test]
fn test_four_pane_layout_expanded() {
    let area = Rect::new(0, 0, 100, 50);
    let layout = compute_layout(area, false);

    // Bottom composer has fixed height constraint
    assert_eq!(layout.composer.height, COMPOSER_HEIGHT);
    assert_eq!(layout.composer.y, area.height - COMPOSER_HEIGHT);
    assert_eq!(layout.composer.x, 0);
    assert_eq!(layout.composer.width, area.width);

    let top_height = area.height - COMPOSER_HEIGHT;
    assert_eq!(layout.navigator.height, top_height);
    assert_eq!(layout.transcript.height, top_height);

    // Blackboard is present when not collapsed
    assert!(layout.blackboard.is_some());
    let blackboard = layout.blackboard.unwrap();
    assert_eq!(blackboard.height, top_height);

    // Verify horizontal alignment and non-overlapping adjacency
    assert_eq!(layout.navigator.x, 0);
    assert_eq!(layout.transcript.x, layout.navigator.width);
    assert_eq!(
        blackboard.x,
        layout.navigator.width + layout.transcript.width
    );

    // Proportions: 22% navigator, 56% transcript, 22% blackboard on 100 width
    assert_eq!(layout.navigator.width, 22);
    assert_eq!(layout.transcript.width, 56);
    assert_eq!(blackboard.width, 22);
    assert_eq!(
        layout.navigator.width + layout.transcript.width + blackboard.width,
        area.width
    );
}

#[test]
fn test_layout_with_blackboard_collapsed() {
    let area = Rect::new(0, 0, 100, 50);
    let layout = compute_layout(area, true);

    // Blackboard is absent when collapsed per Zero-Sentinel invariant (Option::None)
    assert!(layout.blackboard.is_none());

    let top_height = area.height - COMPOSER_HEIGHT;
    assert_eq!(layout.navigator.height, top_height);
    assert_eq!(layout.transcript.height, top_height);

    // Navigator remains 22%, transcript expands to fill remaining 78%
    assert_eq!(layout.navigator.x, 0);
    assert_eq!(layout.navigator.width, 22);
    assert_eq!(layout.transcript.x, 22);
    assert_eq!(layout.transcript.width, 78);
    assert_eq!(layout.navigator.width + layout.transcript.width, area.width);

    // Composer remains fixed at bottom
    assert_eq!(layout.composer.height, COMPOSER_HEIGHT);
    assert_eq!(layout.composer.y, area.height - COMPOSER_HEIGHT);
}

#[test]
fn test_fixed_height_composer_across_varying_heights() {
    for height in [10, 24, 40, 80, 120] {
        let area = Rect::new(0, 0, 120, height);
        let layout = compute_layout(area, false);

        assert_eq!(layout.composer.height, COMPOSER_HEIGHT);
        assert_eq!(layout.composer.y, height - COMPOSER_HEIGHT);
        assert_eq!(layout.navigator.height, height - COMPOSER_HEIGHT);
        assert_eq!(layout.transcript.height, height - COMPOSER_HEIGHT);
    }
}

#[test]
fn test_responsive_horizontal_distribution() {
    let area = Rect::new(0, 0, 200, 60);
    let layout = compute_layout(area, false);

    let blackboard = layout.blackboard.expect("blackboard should be present");

    // 22% of 200 = 44, 56% of 200 = 112, 22% of 200 = 44
    assert_eq!(layout.navigator.width, 44);
    assert_eq!(layout.transcript.width, 112);
    assert_eq!(blackboard.width, 44);
    assert_eq!(
        layout.navigator.width + layout.transcript.width + blackboard.width,
        200
    );
}
