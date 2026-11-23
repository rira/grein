use super::*;
use crate::app::{App, FocusedPane, PendingPermission, ThreadItem};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn test_composer_multiline_auto_grow_and_scroll() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = App::new();
    app.input_buffer = "line1\nline2\nline3\nline4".to_string();

    terminal.draw(|f| render_app(f, &app)).unwrap();

    let buf = terminal.backend().buffer();
    let mut rendered = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            rendered.push_str(buf[(x, y)].symbol());
        }
        rendered.push('\n');
    }

    assert!(rendered.contains("line1"));
    assert!(rendered.contains("line2"));
    assert!(rendered.contains("line3"));
    assert!(rendered.contains("line4"));
}

#[test]
fn test_render_writes_max_scroll_renders_popup_and_tree_status() {
    let backend = TestBackend::new(110, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = App::new();
    app.active_thread_id = "t-1".to_string();
    app.threads = vec![
        ThreadItem {
            id: "t-1".to_string(),
            title: "Trunk".to_string(),
            custom_name: None,
            anchors: Vec::new(),
        },
        ThreadItem {
            id: "t-2".to_string(),
            title: "Side".to_string(),
            custom_name: None,
            anchors: Vec::new(),
        },
    ];
    app.thread_statuses
        .insert("t-1".to_string(), "streaming".to_string());
    app.thread_statuses
        .insert("t-2".to_string(), "error".to_string());
    app.active_transcript_md = (0..40)
        .map(|i| format!("Line {i}: long wrapped content for testing transcript scroll"))
        .collect::<Vec<_>>()
        .join("\n");
    app.sync_anchors();

    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();
    let vp = app.transcript_viewport("t-1");
    assert!(vp.max_scroll() > 0);
    assert_eq!(vp.scroll_offset(), vp.max_scroll());

    let expected_before = vp.max_scroll();
    app.set_focus(FocusedPane::Transcript);
    app.handle_key_event(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(app.active_transcript_scroll_offset(), expected_before - 1);

    app.push_permission(PendingPermission {
        request_id: "r1".to_string(),
        description: "Allow shell execution?".to_string(),
    });
    terminal.draw(|f| render_app_mut(f, &mut app)).unwrap();

    let buf = terminal.backend().buffer();
    let mut screen = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            screen.push_str(buf[(x, y)].symbol());
        }
        screen.push('\n');
    }
    assert!(screen.contains("Permission Required"));
    assert!(screen.contains("[y] Allow  [n] Deny"));
    assert!(!screen.contains("Blackboard"));
    assert!(screen.contains("●"));
    assert!(screen.contains("⚠"));
    assert!(screen.contains("WRITE · AGENT"));
    assert!(screen.contains("Esc cancel · Ctrl+G manage · Alt+Z sidebar"));
    assert!(!screen.contains("Enter: Send"));
}
