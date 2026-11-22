use grein_tui::focus::PaneFocus;

#[test]
fn test_pane_focus_composer_variant_exists() {
    let composer = PaneFocus::Composer;
    assert_eq!(composer, PaneFocus::Composer);
    assert_eq!(format!("{:?}", composer), "Composer");
}

#[test]
fn test_pane_focus_three_pane_tab_cycle() {
    let start = PaneFocus::Composer;
    let p1 = start.next();
    let p2 = p1.next();
    let p3 = p2.next();

    assert_eq!(
        p3, start,
        "Advancing 3 times via Tab must cycle back to starting pane"
    );

    let panes = [start, p1, p2];
    for i in 0..panes.len() {
        for j in (i + 1)..panes.len() {
            assert_ne!(
                panes[i], panes[j],
                "All 3 panes in the Tab cycle must be distinct"
            );
        }
    }
}

#[test]
fn test_pane_focus_three_pane_backtab_cycle() {
    let start = PaneFocus::Composer;
    let prev1 = start.prev();
    let prev2 = prev1.prev();
    let prev3 = prev2.prev();

    assert_eq!(
        prev3, start,
        "Retreating 3 times via BackTab must cycle back to starting pane"
    );

    let panes = [start, prev1, prev2];
    for i in 0..panes.len() {
        for j in (i + 1)..panes.len() {
            assert_ne!(
                panes[i], panes[j],
                "All 3 panes in the BackTab cycle must be distinct"
            );
        }
    }
}

#[test]
fn test_pane_focus_tab_backtab_inverses() {
    let focus = PaneFocus::Composer;
    assert_eq!(focus.next().prev(), focus, "prev() must invert next()");
    assert_eq!(focus.prev().next(), focus, "next() must invert prev()");
    assert_eq!(
        focus.prev(),
        focus.next().next(),
        "prev() must equal next() 2 times in 3-pane cycle"
    );
}
