use grein_tui::focus::{FocusManager, FocusNavigationAction, PaneFocus};

#[test]
fn test_pane_focus_transcript_variant_exists_and_equality() {
    let focus = PaneFocus::Transcript;
    assert_eq!(focus, PaneFocus::Transcript);
    assert_ne!(focus, PaneFocus::Sidebar);
    assert_ne!(focus, PaneFocus::Composer);
}

#[test]
fn test_forward_tab_navigation_cycles_into_and_out_of_transcript() {
    let mut manager = FocusManager::new(PaneFocus::Sidebar);
    assert_eq!(manager.current(), PaneFocus::Sidebar);

    // Tab forward from Sidebar -> Transcript
    manager.handle_action(FocusNavigationAction::Tab);
    assert_eq!(
        manager.current(),
        PaneFocus::Transcript,
        "Tab from Sidebar must navigate into center Transcript pane"
    );

    // Tab forward from Transcript -> Composer
    manager.handle_action(FocusNavigationAction::Tab);
    assert_eq!(
        manager.current(),
        PaneFocus::Composer,
        "Tab from Transcript must navigate into Composer pane"
    );
}

#[test]
fn test_backward_backtab_navigation_cycles_into_and_out_of_transcript() {
    let mut manager = FocusManager::new(PaneFocus::Composer);
    assert_eq!(manager.current(), PaneFocus::Composer);

    // BackTab backward from Composer -> Transcript
    manager.handle_action(FocusNavigationAction::BackTab);
    assert_eq!(
        manager.current(),
        PaneFocus::Transcript,
        "BackTab from Composer must navigate back into center Transcript pane"
    );

    // BackTab backward from Transcript -> Sidebar
    manager.handle_action(FocusNavigationAction::BackTab);
    assert_eq!(
        manager.current(),
        PaneFocus::Sidebar,
        "BackTab from Transcript must navigate back into Sidebar pane"
    );
}

#[test]
fn test_direct_pane_focus_next_and_previous_cycle_transcript() {
    let transcript = PaneFocus::Transcript;

    assert_eq!(transcript.previous(), PaneFocus::Sidebar);
    assert_eq!(transcript.next(), PaneFocus::Composer);

    // Invariant: Bidirectional roundtrip navigation
    assert_eq!(transcript.next().previous(), PaneFocus::Transcript);
    assert_eq!(transcript.previous().next(), PaneFocus::Transcript);
}

#[test]
fn test_full_bidirectional_cyclic_invariants_across_all_panes() {
    let all_panes = [
        PaneFocus::Sidebar,
        PaneFocus::Transcript,
        PaneFocus::Composer,
        PaneFocus::Inspector,
    ];

    for &pane in &all_panes {
        assert_eq!(
            pane.next().previous(),
            pane,
            "Forward then backward must be identity for {:?}",
            pane
        );
        assert_eq!(
            pane.previous().next(),
            pane,
            "Backward then forward must be identity for {:?}",
            pane
        );
    }
}
