use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{App, FocusedPane};
use grein_tui::runner::handle_key_event;

#[test]
fn test_composer_enter_updates_transcript_anchors_blackboard_and_sqlite() {
    let mut app = App::new();
    app.set_focus(FocusedPane::Composer);
    for ch in "Inspect DAG anchors".chars() {
        let _ = handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
        );
    }
    let _ = handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app
        .active_transcript_md
        .contains("# User: Inspect DAG anchors"));
    assert_eq!(app.threads.len(), 1);
    assert!(!app.threads[0].anchors.is_empty());
    assert!(!app.tree.nodes.is_empty());
    app.upsert_blackboard_entry("goal", "inspect");
    assert!(app.blackboard_notes.contains("goal: inspect"));

    app.apply_agent_response("### Verification Plan\n- Checked SQLite persistence");
    assert!(app.active_transcript_md.contains("## Agent (agy)"));
    assert!(app.threads[0].anchors.len() >= 2);
    assert!(app.blackboard_notes.contains("goal: inspect"));
}

#[test]
fn test_sqlite_persistence_roundtrip() {
    let temp_db =
        std::env::temp_dir().join(format!("grein-tui-test-{}.sqlite3", std::process::id()));
    let _ = std::fs::remove_file(&temp_db);

    let mut app = App::new();
    app.submit_composer_prompt("Verify persistence roundtrip".to_string());
    app.persist_state_to_sqlite(&temp_db);

    let mut loaded_app = App::new();
    loaded_app.load_state_from_sqlite(&temp_db);

    assert_eq!(loaded_app.threads.len(), 1);
    assert_eq!(loaded_app.threads[0].title, "Verify persistence roundtrip");
    assert!(loaded_app
        .active_transcript_md
        .contains("# User: Verify persistence roundtrip"));
    assert!(!loaded_app.threads[0].anchors.is_empty());

    let _ = std::fs::remove_file(&temp_db);
}
