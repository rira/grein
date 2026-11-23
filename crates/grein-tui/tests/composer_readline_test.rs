use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use grein_tui::app::{
    line_end, line_start, next_word_boundary, prev_word_boundary, App, FocusedPane,
};
use grein_tui::runner::draw_frame_mut;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn ctrl(ch: char) -> KeyEvent {
    key(KeyCode::Char(ch), KeyModifiers::CONTROL)
}

fn alt(ch: char) -> KeyEvent {
    key(KeyCode::Char(ch), KeyModifiers::ALT)
}

fn render_screen(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let mut screen = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            screen.push_str(buf[(x, y)].symbol());
        }
        screen.push('\n');
    }
    screen
}

#[test]
fn test_helpers_line_and_word_boundaries_edge_cases() {
    assert_eq!(line_start("", 0), 0);
    assert_eq!(line_end("", 0), 0);
    assert_eq!(prev_word_boundary("", 0), 0);
    assert_eq!(next_word_boundary("", 0), 0);

    let multi = "first\nsecond\n\nthird";
    assert_eq!(line_start(multi, 0), 0);
    assert_eq!(line_end(multi, 0), 5);
    assert_eq!(line_start(multi, 5), 0);
    assert_eq!(line_end(multi, 5), 5);
    assert_eq!(line_start(multi, 6), 6);
    assert_eq!(line_end(multi, 6), 12);
    assert_eq!(line_start(multi, 13), 13);
    assert_eq!(line_end(multi, 13), 13);
    assert_eq!(line_start(multi, 14), 14);
    assert_eq!(line_end(multi, 14), multi.len());
    assert_eq!(line_start(multi, 999), 14);
    assert_eq!(line_end(multi, 999), multi.len());

    let path = "foo/bar baz   ";
    let end = path.len();
    assert_eq!(prev_word_boundary(path, 0), 0);
    let p1 = prev_word_boundary(path, end);
    assert_eq!(&path[p1..], "baz   ");
    let p2 = prev_word_boundary(path, p1);
    assert_eq!(&path[p2..], "bar baz   ");
    let p3 = prev_word_boundary(path, p2);
    assert_eq!(p3, 0);

    let n1 = next_word_boundary(path, 0);
    assert_eq!(&path[..n1], "foo");
    let n2 = next_word_boundary(path, n1);
    assert_eq!(&path[..n2], "foo/bar");
    let n3 = next_word_boundary(path, n2);
    assert_eq!(&path[..n3], "foo/bar baz");
    let n4 = next_word_boundary(path, n3);
    assert_eq!(n4, path.len());

    let utf8 = "héllo 🦀 wörld";
    assert!(utf8.is_char_boundary(prev_word_boundary(utf8, utf8.len())));
    assert!(utf8.is_char_boundary(prev_word_boundary(utf8, 8))); // mid-emoji byte index
    assert!(utf8.is_char_boundary(next_word_boundary(utf8, 8))); // mid-emoji byte index
    assert!(utf8.is_char_boundary(line_start(utf8, 8)));
    assert!(utf8.is_char_boundary(line_end(utf8, 8)));
}

#[test]
fn test_ctrl_a_ctrl_e_and_multiline_home_end() {
    let mut app = App::new();
    app.input_buffer = "alpha beta\ngamma delta\nomega".to_string();
    app.cursor_position = 18; // "gamma d|elta"

    assert!(app.handle_key_event(ctrl('a')));
    assert_eq!(app.cursor_position, 11);

    assert!(app.handle_key_event(ctrl('e')));
    assert_eq!(app.cursor_position, 22);

    assert!(app.handle_key_event(key(KeyCode::Home, KeyModifiers::NONE)));
    assert_eq!(app.cursor_position, 11);

    assert!(app.handle_key_event(key(KeyCode::End, KeyModifiers::NONE)));
    assert_eq!(app.cursor_position, 22);

    // Uppercase Ctrl+A / Ctrl+E
    app.cursor_position = 4;
    assert!(app.handle_key_event(ctrl('E')));
    assert_eq!(app.cursor_position, 10);
    assert!(app.handle_key_event(ctrl('A')));
    assert_eq!(app.cursor_position, 0);

    // Last line
    app.cursor_position = 25;
    assert!(app.handle_key_event(key(KeyCode::Home, KeyModifiers::NONE)));
    assert_eq!(app.cursor_position, 23);
    assert!(app.handle_key_event(key(KeyCode::End, KeyModifiers::NONE)));
    assert_eq!(app.cursor_position, app.input_buffer.len());
}

#[test]
fn test_ctrl_k_and_ctrl_u_multiline_and_kill_ring() {
    let mut app = App::new();
    app.input_buffer = "first line\nsecond line\nthird".to_string();
    app.cursor_position = 18; // "second |line"

    // Ctrl+K mid-line kills to line_end
    assert!(app.handle_key_event(ctrl('k')));
    assert_eq!(app.input_buffer, "first line\nsecond \nthird");
    assert_eq!(app.cursor_position, 18);
    assert_eq!(app.kill_ring, "line");

    // Ctrl+K at line_end kills single newline
    assert!(app.handle_key_event(ctrl('k')));
    assert_eq!(app.input_buffer, "first line\nsecond third");
    assert_eq!(app.cursor_position, 18);
    assert_eq!(app.kill_ring, "\n");

    // Ctrl+K at EOF does not overwrite kill_ring
    app.cursor_position = app.input_buffer.len();
    assert!(app.handle_key_event(ctrl('k')));
    assert_eq!(app.input_buffer, "first line\nsecond third");
    assert_eq!(app.kill_ring, "\n");

    // Ctrl+U at line_start kills preceding newline
    app.cursor_position = 11; // start of line 2
    assert!(app.handle_key_event(ctrl('u')));
    assert_eq!(app.input_buffer, "first linesecond third");
    assert_eq!(app.cursor_position, 10);
    assert_eq!(app.kill_ring, "\n");

    // Ctrl+U mid-line kills from line_start to cursor
    app.cursor_position = 6; // "first |linesecond third"
    assert!(app.handle_key_event(ctrl('u')));
    assert_eq!(app.input_buffer, "linesecond third");
    assert_eq!(app.cursor_position, 0);
    assert_eq!(app.kill_ring, "first ");

    // Ctrl+U at index 0 preserves kill_ring
    assert!(app.handle_key_event(ctrl('u')));
    assert_eq!(app.input_buffer, "linesecond third");
    assert_eq!(app.kill_ring, "first ");
}

#[test]
fn test_backward_and_forward_word_kills() {
    let mut app = App::new();
    app.input_buffer = "src/main.rs test_fn".to_string();
    app.cursor_position = app.input_buffer.len();

    // Ctrl+W kills prev word
    assert!(app.handle_key_event(ctrl('w')));
    assert_eq!(app.input_buffer, "src/main.rs ");
    assert_eq!(app.cursor_position, 12);
    assert_eq!(app.kill_ring, "test_fn");

    // Alt+Backspace kills prev word including trailing space
    assert!(app.handle_key_event(key(KeyCode::Backspace, KeyModifiers::ALT)));
    assert_eq!(app.input_buffer, "src/main.");
    assert_eq!(app.cursor_position, 9);
    assert_eq!(app.kill_ring, "rs ");

    // Move to start and test Alt+D, Alt+Delete, Ctrl+Delete
    app.cursor_position = 0;
    assert!(app.handle_key_event(alt('d')));
    assert_eq!(app.input_buffer, "/main.");
    assert_eq!(app.cursor_position, 0);
    assert_eq!(app.kill_ring, "src");

    assert!(app.handle_key_event(key(KeyCode::Delete, KeyModifiers::ALT)));
    assert_eq!(app.input_buffer, ".");
    assert_eq!(app.cursor_position, 0);
    assert_eq!(app.kill_ring, "/main");

    app.input_buffer = "alpha beta gamma".to_string();
    app.cursor_position = 5; // "alpha| beta gamma"
    assert!(app.handle_key_event(key(KeyCode::Delete, KeyModifiers::CONTROL)));
    assert_eq!(app.input_buffer, "alpha gamma");
    assert_eq!(app.cursor_position, 5);
    assert_eq!(app.kill_ring, " beta");

    // Alt+D at EOF preserves kill_ring
    app.cursor_position = app.input_buffer.len();
    assert!(app.handle_key_event(alt('D')));
    assert_eq!(app.kill_ring, " beta");
}

#[test]
fn test_word_navigation_char_f_d_and_yank_roundtrip() {
    let mut app = App::new();
    app.input_buffer = "one two/three four".to_string();
    app.cursor_position = app.input_buffer.len();

    assert!(app.handle_key_event(key(KeyCode::Left, KeyModifiers::ALT)));
    assert_eq!(&app.input_buffer[app.cursor_position..], "four");

    assert!(app.handle_key_event(alt('b')));
    assert_eq!(&app.input_buffer[app.cursor_position..], "three four");

    assert!(app.handle_key_event(key(KeyCode::Left, KeyModifiers::CONTROL)));
    assert_eq!(&app.input_buffer[app.cursor_position..], "two/three four");

    assert!(app.handle_key_event(alt('B')));
    assert_eq!(app.cursor_position, 0);

    assert!(app.handle_key_event(key(KeyCode::Right, KeyModifiers::ALT)));
    assert_eq!(app.cursor_position, 3);

    assert!(app.handle_key_event(alt('f')));
    assert_eq!(app.cursor_position, 7);

    assert!(app.handle_key_event(key(KeyCode::Right, KeyModifiers::CONTROL)));
    assert_eq!(app.cursor_position, 13);

    assert!(app.handle_key_event(alt('F')));
    assert_eq!(app.cursor_position, app.input_buffer.len());

    // Right arrow and Ctrl+D on multi-byte text (Ctrl+F opens Forward Search)
    app.input_buffer = "héllo".to_string();
    app.cursor_position = 1; // right before 'é'
    assert!(app.handle_key_event(key(KeyCode::Right, KeyModifiers::NONE)));
    assert_eq!(app.cursor_position, 3); // after 2-byte 'é'
    assert!(app.handle_key_event(ctrl('d')));
    assert_eq!(app.input_buffer, "hélo");
    assert_eq!(app.cursor_position, 3);

    // Roundtrip kill -> move -> yank
    app.input_buffer = "hello world".to_string();
    app.cursor_position = app.input_buffer.len();
    assert!(app.handle_key_event(ctrl('w'))); // kills "world"
    assert_eq!(app.input_buffer, "hello ");
    assert_eq!(app.kill_ring, "world");

    assert!(app.handle_key_event(ctrl('a')));
    assert_eq!(app.cursor_position, 0);

    assert!(app.handle_key_event(ctrl('y')));
    assert_eq!(app.input_buffer, "worldhello ");
    assert_eq!(app.cursor_position, 5);

    // Kill ring remains reusable for multiple yanks
    assert!(app.handle_key_event(ctrl('e')));
    assert!(app.handle_key_event(ctrl('y')));
    assert_eq!(app.input_buffer, "worldhello world");
    assert_eq!(app.cursor_position, 16);
}

#[test]
fn test_utf8_emoji_safety_no_panic() {
    let mut app = App::new();
    let sample = "héllo 🦀 wörld\nline2 🚀";
    app.input_buffer = sample.to_string();
    app.cursor_position = 0;

    let keys = [
        key(KeyCode::Right, KeyModifiers::NONE),
        key(KeyCode::Right, KeyModifiers::NONE),
        key(KeyCode::Right, KeyModifiers::NONE),
        key(KeyCode::Right, KeyModifiers::ALT),
        key(KeyCode::Right, KeyModifiers::ALT),
        ctrl('d'),
        key(KeyCode::Left, KeyModifiers::ALT),
        ctrl('e'),
        ctrl('w'),
        ctrl('a'),
        ctrl('y'),
        key(KeyCode::End, KeyModifiers::NONE),
        key(KeyCode::Home, KeyModifiers::NONE),
        alt('d'),
        ctrl('k'),
        ctrl('y'),
        ctrl('u'),
        ctrl('y'),
    ];

    for ev in keys {
        app.handle_key_event(ev);
        assert!(
            app.input_buffer.is_char_boundary(app.cursor_position),
            "cursor {} is not a char boundary in {:?}",
            app.cursor_position,
            app.input_buffer
        );
    }

    // Explicit multi-byte emoji kill/yank sanity
    app.input_buffer = "héllo 🦀 wörld".to_string();
    app.cursor_position = "héllo 🦀".len();
    app.handle_key_event(key(KeyCode::Right, KeyModifiers::NONE)); // over ' '
    app.handle_key_event(key(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(&app.input_buffer[..app.cursor_position], "");
    app.handle_key_event(key(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(&app.input_buffer[..app.cursor_position], "héllo");
    app.handle_key_event(alt('d'));
    assert_eq!(app.kill_ring, " 🦀 wörld");
    assert_eq!(app.input_buffer, "héllo");
    app.handle_key_event(ctrl('y'));
    assert_eq!(app.input_buffer, "héllo 🦀 wörld");
}

#[test]
fn test_preservations_non_composer_isolation_and_help_modal() {
    let mut app = App::new();
    app.input_buffer = "keep this draft".to_string();
    app.cursor_position = 4;

    // Ctrl+G toggles Composer <-> Tree while preserving input_buffer and cursor_position
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert!(app.handle_key_event(ctrl('g')));
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app.handle_key_event(ctrl('g')));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert_eq!(app.input_buffer, "keep this draft");
    assert_eq!(app.cursor_position, 4);

    // Ctrl+N does not hijack Composer in WRITE mode
    assert!(!app.handle_key_event(ctrl('n')));
    assert!(app.pending_actions.is_empty());

    // Ctrl+F moves cursor forward cleanly (Readline symmetry with Ctrl+B)
    app.cursor_position = 0;
    assert!(app.handle_key_event(ctrl('f')));
    assert_eq!(app.cursor_position, 1);
    assert!(app.handle_key_event(ctrl('b')));
    assert_eq!(app.cursor_position, 0);
    app.cursor_position = 4;

    // Ctrl+J still inserts newline in Composer
    assert!(app.handle_key_event(ctrl('j')));
    assert_eq!(app.input_buffer, "keep\n this draft");
    assert_eq!(app.cursor_position, 5);

    // Alt+1..9 are reserved and must not be bound in Composer
    for digit in '1'..='9' {
        assert!(!app.handle_key_event(alt(digit)));
    }
    assert_eq!(app.input_buffer, "keep\n this draft");
    assert_eq!(app.cursor_position, 5);

    // Non-Composer panes must not trigger Composer Readline edits
    app.kill_ring = "preserved".to_string();
    for pane in [FocusedPane::Tree, FocusedPane::Transcript] {
        app.set_focus(pane);
        for ev in [
            ctrl('a'),
            ctrl('e'),
            ctrl('k'),
            ctrl('u'),
            ctrl('w'),
            ctrl('d'),
            ctrl('y'),
            alt('b'),
            alt('f'),
            alt('d'),
            key(KeyCode::Left, KeyModifiers::ALT),
            key(KeyCode::Right, KeyModifiers::ALT),
            key(KeyCode::Backspace, KeyModifiers::ALT),
            key(KeyCode::Delete, KeyModifiers::ALT),
        ] {
            app.handle_key_event(ev);
        }
        assert_eq!(app.input_buffer, "keep\n this draft");
        assert_eq!(app.cursor_position, 5);
        assert_eq!(app.kill_ring, "preserved");
    }

    // '/' in MANAGE opens Forward Search without mutating Composer draft/cursor; Esc clears it
    app.set_focus(FocusedPane::Tree);
    assert!(app.handle_key_event(key(KeyCode::Char('/'), KeyModifiers::NONE)));
    assert!(app.transcript_search.editing);
    assert_eq!(app.input_buffer, "keep\n this draft");
    assert_eq!(app.cursor_position, 5);
    app.handle_key_event(key(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.transcript_search.is_visible());

    // '?' in MANAGE opens Help Modal displaying Readline & 2-mode shortcuts without clipping
    assert!(app.handle_key_event(key(KeyCode::Char('?'), KeyModifiers::NONE)));
    assert!(app.show_help_modal);

    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    draw_frame_mut(&mut terminal, &mut app).unwrap();
    let screen = render_screen(&terminal);
    assert!(screen.contains("WRITE Mode"), "Missing WRITE Mode in help:\n{screen}");
    assert!(screen.contains("MANAGE Mode"), "Missing MANAGE Mode in help:\n{screen}");
    assert!(screen.contains("Ctrl+A/E/B/F/W/U/K/Y"), "Missing Readline in help:\n{screen}");
    assert!(
        screen.contains("Esc/q/?/F1 Close") || screen.contains("Esc, q, ?, F1, or Enter to close"),
        "Help modal footer clipped:\n{screen}"
    );
}
