use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, FocusedPane};

#[test]
fn test_per_thread_transcript_viewport_independence() {
    let mut app = App::new();
    assert_eq!(app.active_thread_id(), "trunk");

    // Initial viewport for trunk should have defaults
    let initial_vp = app.transcript_viewport("trunk");
    assert_eq!(initial_vp.scroll_offset(), 0);
    assert_eq!(initial_vp.max_scroll(), 0);
    assert!(initial_vp.auto_scroll());

    // Mutate viewport for trunk
    let trunk_vp = app.transcript_viewport_mut("trunk");
    trunk_vp.set_max_scroll(50);
    trunk_vp.scroll_to_bottom();
    assert_eq!(trunk_vp.scroll_offset(), 50);
    trunk_vp.scroll_up(15);
    assert_eq!(trunk_vp.scroll_offset(), 35);
    assert!(!trunk_vp.auto_scroll());
    assert_eq!(app.active_transcript_scroll_offset(), 35);

    // Switch to another thread: "feature-1"
    app.switch_thread("feature-1");
    assert_eq!(app.active_thread_id(), "feature-1");

    // Viewport for "feature-1" should be fresh and independent
    let feat_vp = app.transcript_viewport("feature-1");
    assert_eq!(feat_vp.scroll_offset(), 0);
    assert_eq!(feat_vp.max_scroll(), 0);
    assert!(feat_vp.auto_scroll());
    assert_eq!(app.active_transcript_scroll_offset(), 0);

    // Mutate "feature-1" viewport
    let feat_vp_mut = app.transcript_viewport_mut("feature-1");
    feat_vp_mut.set_max_scroll(20);
    feat_vp_mut.scroll_to_bottom();
    assert_eq!(feat_vp_mut.scroll_offset(), 20);
    feat_vp_mut.scroll_up(5);
    assert_eq!(feat_vp_mut.scroll_offset(), 15);
    assert_eq!(app.active_transcript_scroll_offset(), 15);

    // Switch back to "trunk" and verify original viewport state is preserved
    app.switch_thread("trunk");
    assert_eq!(app.active_thread_id(), "trunk");
    assert_eq!(app.active_transcript_scroll_offset(), 35);
    let trunk_vp_again = app.transcript_viewport("trunk");
    assert_eq!(trunk_vp_again.scroll_offset(), 35);
    assert_eq!(trunk_vp_again.max_scroll(), 50);
    assert!(!trunk_vp_again.auto_scroll());

    // Switch back to "feature-1" and verify preserved
    app.switch_thread("feature-1");
    assert_eq!(app.active_thread_id(), "feature-1");
    assert_eq!(app.active_transcript_scroll_offset(), 15);
    let feat_vp_again = app.transcript_viewport("feature-1");
    assert_eq!(feat_vp_again.scroll_offset(), 15);
    assert_eq!(feat_vp_again.max_scroll(), 20);
}

#[test]
fn test_transcript_keyboard_scrolling_key_events() {
    let mut app = App::new();
    app.set_focus(FocusedPane::Transcript);
    assert_eq!(app.focused_pane(), FocusedPane::Transcript);

    // Setup viewport with max_scroll 100
    {
        let vp = app.transcript_viewport_mut("trunk");
        vp.set_max_scroll(100);
        vp.scroll_to_bottom();
    }
    assert_eq!(app.active_transcript_scroll_offset(), 100);
    assert!(app.transcript_viewport("trunk").auto_scroll());

    // Up key should scroll up by 1 and disable auto_scroll
    let handled = app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert!(handled);
    assert_eq!(app.active_transcript_scroll_offset(), 99);
    assert!(!app.transcript_viewport("trunk").auto_scroll());

    // PageUp should scroll up by 10
    let handled = app.handle_key_event(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert!(handled);
    assert_eq!(app.active_transcript_scroll_offset(), 89);

    // Home should scroll to top (0)
    let handled = app.handle_key_event(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert!(handled);
    assert_eq!(app.active_transcript_scroll_offset(), 0);
    assert!(!app.transcript_viewport("trunk").auto_scroll());

    // Down key should scroll down by 1
    let handled = app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(handled);
    assert_eq!(app.active_transcript_scroll_offset(), 1);
    assert!(!app.transcript_viewport("trunk").auto_scroll());

    // PageDown should scroll down by 10
    let handled = app.handle_key_event(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert!(handled);
    assert_eq!(app.active_transcript_scroll_offset(), 11);
    assert!(!app.transcript_viewport("trunk").auto_scroll());

    // End should scroll to bottom (max_scroll) and enable auto_scroll
    let handled = app.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert!(handled);
    assert_eq!(app.active_transcript_scroll_offset(), 100);
    assert!(app.transcript_viewport("trunk").auto_scroll());
}

#[test]
fn test_per_thread_composer_draft_save_and_restore() {
    let mut app = App::new();
    assert_eq!(app.active_thread_id(), "trunk");
    assert_eq!(app.input_buffer, "");
    assert_eq!(app.cursor_position, 0);

    // Enter draft in trunk
    let trunk_draft = "draft for trunk branch";
    app.input_buffer = trunk_draft.to_string();
    app.cursor_position = app.input_buffer.len();

    // Switch to thread "investigation/issue-42"
    app.switch_thread("investigation/issue-42");
    assert_eq!(app.active_thread_id(), "investigation/issue-42");
    assert_eq!(app.input_buffer, "");
    assert_eq!(app.cursor_position, 0);

    // Enter draft in investigation thread
    let issue_draft = "Investigating memory leak on allocator";
    app.input_buffer = issue_draft.to_string();
    app.cursor_position = app.input_buffer.len();

    // Switch to a third thread with no prior draft
    app.switch_thread("bugfix/fast-path");
    assert_eq!(app.active_thread_id(), "bugfix/fast-path");
    assert_eq!(app.input_buffer, "");
    assert_eq!(app.cursor_position, 0);

    // Switch back to "trunk": draft should be restored
    app.switch_thread("trunk");
    assert_eq!(app.active_thread_id(), "trunk");
    assert_eq!(app.input_buffer, trunk_draft);
    assert_eq!(app.cursor_position, trunk_draft.len());

    // Switch back to "investigation/issue-42": draft should be restored
    app.switch_thread("investigation/issue-42");
    assert_eq!(app.active_thread_id(), "investigation/issue-42");
    assert_eq!(app.input_buffer, issue_draft);
    assert_eq!(app.cursor_position, issue_draft.len());
}

#[test]
fn test_global_scroll_shortcuts_from_composer_and_tree_preview() {
    let mut app = App::new();
    app.threads = vec![
        grein_tui::app::ThreadItem {
            id: "trunk".to_string(),
            title: "Trunk".to_string(),
            anchors: Vec::new(),
        },
        grein_tui::app::ThreadItem {
            id: "preview".to_string(),
            title: "Preview".to_string(),
            anchors: Vec::new(),
        },
    ];
    app.sync_anchors();
    app.select_active_thread_in_tree();

    app.transcript_viewport_mut("trunk").set_max_scroll(100);
    app.transcript_viewport_mut("trunk").scroll_to_bottom();
    app.transcript_viewport_mut("preview").set_max_scroll(60);
    app.transcript_viewport_mut("preview").scroll_to_bottom();

    // Composer focused: Alt+Up/Down scroll by 6, PageUp/PageDown by 10, Alt+Home/End top/bottom
    app.set_focus(FocusedPane::Composer);
    app.input_buffer = "keep".to_string();
    app.cursor_position = 2;

    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)));
    assert_eq!(app.transcript_viewport("trunk").scroll_offset(), 94);
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)));
    assert_eq!(app.transcript_viewport("trunk").scroll_offset(), 100);

    assert!(app.handle_key_event(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)));
    assert_eq!(app.transcript_viewport("trunk").scroll_offset(), 90);
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)));
    assert_eq!(app.transcript_viewport("trunk").scroll_offset(), 100);

    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Home, KeyModifiers::ALT)));
    assert_eq!(app.transcript_viewport("trunk").scroll_offset(), 0);
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::ALT)));
    assert_eq!(app.transcript_viewport("trunk").scroll_offset(), 100);

    // Plain Home/End still operate on Composer line bounds
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)));
    assert_eq!(app.cursor_position, 0);
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)));
    assert_eq!(app.cursor_position, 4);
    assert_eq!(app.transcript_viewport("trunk").scroll_offset(), 100);

    // Tree focused & previewing "preview": scrolling targets previewed thread, not trunk
    app.set_focus(FocusedPane::Tree);
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.active_thread_id(), "trunk");
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)));
    assert_eq!(app.transcript_viewport("preview").scroll_offset(), 54);
    assert_eq!(app.transcript_viewport("trunk").scroll_offset(), 100);
}
