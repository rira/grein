use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;

use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use grein_core::ThreadKind;
use grein_tui::app::{
    App, AppAction, FocusedPane, RenamePromptState, SidebarView, ThreadItem,
};
use grein_tui::engine_port::{
    PtyCell, PtyMouseEncoding, PtyMouseMode, PtyScreenSnapshot, ThreadMeta, WorktreeEntry,
};
use grein_tui::render::render_app_mut;
use grein_tui::runner::now_epoch_ms;
use grein_tui::selection::SelectionPane;
use ratatui::backend::TestBackend;
use ratatui::text::Span;
use ratatui::Terminal;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct ClipboardMockGuard {
    dir: PathBuf,
    prev_path: Option<OsString>,
    prev_wayland: Option<OsString>,
}

impl ClipboardMockGuard {
    fn new(clipboard_text: &str, primary_text: Option<&str>) -> Self {
        let unique = format!(
            "grein-clip-test-{}-{}",
            std::process::id(),
            now_epoch_ms()
        );
        let dir = std::env::temp_dir().join(unique);
        fs::create_dir_all(&dir).unwrap();

        let clip_file = dir.join("clip.txt");
        let prim_file = dir.join("prim.txt");
        fs::write(&clip_file, clipboard_text).unwrap();
        if let Some(p) = primary_text {
            fs::write(&prim_file, p).unwrap();
        }

        let wl_paste_script = format!(
            "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"--primary\" ]; then\n    if [ -f \"{prim}\" ]; then\n      cat \"{prim}\"\n      exit 0\n    fi\n    exit 1\n  fi\ndone\ncat \"{clip}\"\n",
            prim = prim_file.display(),
            clip = clip_file.display(),
        );
        let wl_path = dir.join("wl-paste");
        fs::write(&wl_path, wl_paste_script).unwrap();
        let mut perms = fs::metadata(&wl_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&wl_path, perms).unwrap();

        let prev_path = std::env::var_os("PATH");
        let prev_wayland = std::env::var_os("WAYLAND_DISPLAY");
        let new_path = match &prev_path {
            Some(p) => format!("{}:{}", dir.display(), p.to_string_lossy()),
            None => dir.display().to_string(),
        };
        std::env::set_var("PATH", new_path);
        std::env::set_var("WAYLAND_DISPLAY", "grein-test-wayland");

        Self {
            dir,
            prev_path,
            prev_wayland,
        }
    }
}

impl Drop for ClipboardMockGuard {
    fn drop(&mut self) {
        match &self.prev_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        match &self.prev_wayland {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn mouse(kind: MouseEventKind, column: u16, row: u16, modifiers: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers,
    }
}

fn make_meta(id: &str, parent: Option<&str>, kind: ThreadKind) -> ThreadMeta {
    ThreadMeta {
        thread_id: id.to_string(),
        parent_thread_id: parent.map(str::to_string),
        inherited_turn_count: 0,
        merged: false,
        kind,
        cwd: None,
        git_branch: None,
        git_dirty: 0,
        custom_name: None,
    }
}

fn pty_row_from_str(text: &str, total_cols: usize) -> Vec<PtyCell> {
    let mut row = Vec::with_capacity(total_cols);
    for ch in text.chars() {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        let w = Span::raw(&*s).width();
        if w == 2 {
            row.push(PtyCell {
                contents: s.to_string(),
                ..PtyCell::default()
            });
            row.push(PtyCell {
                contents: String::new(),
                is_wide_continuation: true,
                ..PtyCell::default()
            });
        } else {
            row.push(PtyCell {
                contents: s.to_string(),
                ..PtyCell::default()
            });
        }
    }
    while row.len() < total_cols {
        row.push(PtyCell::default());
    }
    row
}

fn draw_app(terminal: &mut Terminal<TestBackend>, app: &mut App) {
    terminal.draw(|f| render_app_mut(f, app)).unwrap();
}

fn setup_agent_app() -> App {
    let mut app = App::new();
    app.active_thread_id = "t1".to_string();
    app.threads = vec![
        ThreadItem {
            id: "t1".to_string(),
            title: "Alpha".to_string(),
            custom_name: None,
            anchors: Vec::new(),
        },
        ThreadItem {
            id: "t2".to_string(),
            title: "Beta".to_string(),
            custom_name: None,
            anchors: Vec::new(),
        },
        ThreadItem {
            id: "term1".to_string(),
            title: "Shell".to_string(),
            custom_name: None,
            anchors: Vec::new(),
        },
    ];
    app.thread_meta
        .insert("t1".to_string(), make_meta("t1", None, ThreadKind::Agent));
    app.thread_meta
        .insert("t2".to_string(), make_meta("t2", Some("t1"), ThreadKind::Agent));
    app.thread_meta.insert(
        "term1".to_string(),
        make_meta("term1", Some("t1"), ThreadKind::Terminal),
    );
    app.active_transcript_md = [
        "# User: check prose",
        "",
        "## Assistant",
        "alpha beta gamma delta epsilon zeta",
        "",
        "See https://example.com/very/long/path/that/wraps/across/rows",
        "",
        "hi 🦀world!",
    ]
    .join("\n");
    app.thread_transcripts
        .insert("t1".to_string(), app.active_transcript_md.clone());
    app.sync_anchors();
    app.select_active_thread_in_tree();
    app.focused_pane = FocusedPane::Composer;
    app
}

fn setup_terminal_app(mode: PtyMouseMode) -> (App, Terminal<TestBackend>) {
    let mut app = setup_agent_app();
    app.switch_thread("term1");
    let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
    draw_app(&mut terminal, &mut app);
    let right_inner = app.pane_rects.right_inner.unwrap();
    let cols = right_inner.width as usize;
    let rows = right_inner.height as usize;

    let mut cells = vec![vec![PtyCell::default(); cols]; rows];
    cells[0] = pty_row_from_str("echo hi 🦀!", cols);
    let mut wrapped_top = pty_row_from_str("hello ", 6);
    wrapped_top.truncate(6);
    if cols > 6 {
        wrapped_top = pty_row_from_str(&format!("{:<width$}", "hello_long_word", width = cols), cols);
    }
    cells[1] = wrapped_top;
    cells[2] = pty_row_from_str("continued_tail", cols);
    let mut row_wrapped = vec![false; rows];
    row_wrapped[1] = true;

    app.pty_screens.insert(
        "term1".to_string(),
        PtyScreenSnapshot {
            rows: rows as u16,
            cols: cols as u16,
            cells,
            row_wrapped,
            mouse_protocol_mode: mode,
            mouse_protocol_encoding: PtyMouseEncoding::Sgr,
            ..PtyScreenSnapshot::default()
        },
    );
    draw_app(&mut terminal, &mut app);
    (app, terminal)
}

#[test]
fn test_transcript_drag_clamped_highlighting_soft_wraps_and_unmoved_click() {
    let mut app = setup_agent_app();
    let mut terminal = Terminal::new(TestBackend::new(50, 20)).unwrap();
    draw_app(&mut terminal, &mut app);

    let rects = app.pane_rects;
    let tree_inner = rects.tree_inner.expect("tree_inner cached");
    let right_inner = rects.right_inner.expect("right_inner cached");
    let composer_inner = rects.composer_inner.expect("composer_inner cached");
    assert!(tree_inner.right() < right_inner.left());
    assert!(composer_inner.top() > right_inner.bottom());

    // Single unmoved click clears selection and never copies.
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        right_inner.x + 2,
        right_inner.y + 1,
        KeyModifiers::NONE,
    )));
    assert!(app.pane_selection.as_ref().is_some_and(|s| s.dragging));
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Up(MouseButton::Left),
        right_inner.x + 2,
        right_inner.y + 1,
        KeyModifiers::NONE,
    )));
    assert!(app.pane_selection.is_none());
    assert!(app.last_copied_text.is_none());

    // Find wrapped "See https://..." rows in rendered lines.
    let url_start_row = app
        .last_rendered_lines
        .iter()
        .position(|l| l.spans.iter().any(|s| s.content.contains("See")))
        .expect("url intro row");
    let url_end_row = app
        .last_rendered_lines
        .iter()
        .rposition(|l| l.spans.iter().any(|s| s.content.contains("rows")))
        .expect("url end row");
    assert!(url_end_row > url_start_row, "URL must wrap across rows");

    let scroll_y = app.last_transcript_scroll_y;
    assert!(url_start_row >= scroll_y);
    let vis_start_y = right_inner.y + (url_start_row - scroll_y) as u16;
    let vis_end_y = right_inner.y + (url_end_row - scroll_y) as u16;

    // Capture baseline buffer before drag to verify zero bleed outside `right_inner`.
    draw_app(&mut terminal, &mut app);
    let baseline_buf = terminal.backend().buffer().clone();

    // Start dragging from right side of URL end row, drag LEFT across `│` border into sidebar!
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        right_inner.right() - 1,
        vis_end_y,
        KeyModifiers::NONE,
    )));
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        tree_inner.x, // well left of `│` border
        vis_start_y,
        KeyModifiers::NONE,
    )));

    // Render mid-drag and assert selection highlight NEVER bleeds outside `right_inner`!
    draw_app(&mut terminal, &mut app);
    let sel_bg = app.theme_palette.selection_bg;
    let buf = terminal.backend().buffer();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let inside_right = x >= right_inner.left()
                && x < right_inner.right()
                && y >= right_inner.top()
                && y < right_inner.bottom();
            if !inside_right {
                assert_eq!(
                    buf[(x, y)],
                    baseline_buf[(x, y)],
                    "highlight bled outside right_inner at ({x}, {y})"
                );
            }
        }
        assert_ne!(
            buf[(right_inner.left() - 1, y)].bg,
            sel_bg,
            "highlight bled into left border `│` at y={y}"
        );
        assert_ne!(
            buf[(right_inner.right(), y)].bg,
            sel_bg,
            "highlight bled into right border `│` at y={y}"
        );
    }
    assert_eq!(buf[(right_inner.x, vis_start_y)].bg, sel_bg);

    // Release outside left border -> clamps cleanly and copies soft-wrapped URL without newline or border.
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Up(MouseButton::Left),
        0,
        vis_start_y,
        KeyModifiers::NONE,
    )));
    let copied = app.last_copied_text.clone().expect("auto-copied URL");
    assert_eq!(
        copied,
        "See https://example.com/very/long/path/that/wraps/across/rows"
    );
    assert!(!copied.contains('│'));
    assert!(app.status_notice.as_deref().unwrap_or("").contains("selection"));
}

#[test]
fn test_double_triple_click_and_wide_glyph_snap_in_transcript() {
    let mut app = setup_agent_app();
    let mut terminal = Terminal::new(TestBackend::new(60, 22)).unwrap();
    draw_app(&mut terminal, &mut app);

    let right_inner = app.pane_rects.right_inner.unwrap();
    let scroll_y = app.last_transcript_scroll_y;
    let emoji_row = app
        .last_rendered_lines
        .iter()
        .position(|l| l.spans.iter().any(|s| s.content.contains("🦀world")))
        .expect("emoji row");
    let vis_y = right_inner.y + (emoji_row - scroll_y) as u16;

    // "hi 🦀world!" -> 'h'(0) 'i'(1) ' '(2) '🦀'(3..=4) 'w'(5)..'d'(9) '!'(10)
    // Double-click on 'w' (col 6 in "hi 🦀world!"): wait '🦀' is 2 cols -> col 5..=9 is "world".
    let click_x = right_inner.x + 6;
    app.last_click_info = None;
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        click_x,
        vis_y,
        KeyModifiers::NONE,
    )));
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Up(MouseButton::Left),
        click_x,
        vis_y,
        KeyModifiers::NONE,
    )));
    // Second click within 400ms -> double-click selects "world" immediately!
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        click_x,
        vis_y,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.last_copied_text.as_deref(), Some("world"));
    assert_eq!(
        app.pane_selection.as_ref().map(|s| s.ordered()),
        Some(((emoji_row, 5), (emoji_row, 9)))
    );
    // Up after double-click keeps highlight & copy.
    assert!(!app.handle_mouse_event(mouse(
        MouseEventKind::Up(MouseButton::Left),
        click_x,
        vis_y,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.last_copied_text.as_deref(), Some("world"));

    // Third click within 400ms -> triple-click selects full row [0..=line_end]!
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        click_x,
        vis_y,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.last_copied_text.as_deref(), Some("hi 🦀world!"));
    assert_eq!(
        app.pane_selection.as_ref().map(|s| s.ordered()),
        Some(((emoji_row, 0), (emoji_row, 10)))
    );

    // Click after >400ms resets click count back to 1 and snaps wide emoji cleanly on drag.
    app.last_click_info = Some((now_epoch_ms().saturating_sub(500), right_inner.x + 4, vis_y, 2));
    app.last_copied_text = None;
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        right_inner.x + 4, // second half of 🦀
        vis_y,
        KeyModifiers::NONE,
    )));
    assert!(app.last_copied_text.is_none());
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        right_inner.x + 5, // 'w'
        vis_y,
        KeyModifiers::NONE,
    )));
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Up(MouseButton::Left),
        right_inner.x + 5,
        vis_y,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.last_copied_text.as_deref(), Some("🦀w"));

    // Double-click in trailing blank padding on the same row clears selection without copying.
    app.last_copied_text = None;
    let pad_x = right_inner.x + 25;
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        pad_x,
        vis_y,
        KeyModifiers::NONE,
    )));
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Up(MouseButton::Left),
        pad_x,
        vis_y,
        KeyModifiers::NONE,
    )));
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        pad_x,
        vis_y,
        KeyModifiers::NONE,
    )));
    assert!(app.last_copied_text.is_none());
    assert!(app.pane_selection.is_none());
}

#[test]
fn test_pty_sgr_forwarding_vs_alt_drag_override_and_frozen_snapshot() {
    let (mut app, mut terminal) = setup_terminal_app(PtyMouseMode::PressRelease);
    let right_inner = app.pane_rects.right_inner.unwrap();
    app.pending_actions.clear();

    // Normal click + release + scroll inside live Terminal forwards SGR sequences.
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        right_inner.x + 2,
        right_inner.y + 1,
        KeyModifiers::NONE,
    )));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "term1".to_string(),
            bytes: b"\x1b[<0;3;2M".to_vec(),
        })
    );
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Up(MouseButton::Left),
        right_inner.x + 2,
        right_inner.y + 1,
        KeyModifiers::NONE,
    )));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "term1".to_string(),
            bytes: b"\x1b[<0;3;2m".to_vec(),
        })
    );
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::ScrollUp,
        right_inner.x + 1,
        right_inner.y,
        KeyModifiers::NONE,
    )));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "term1".to_string(),
            bytes: b"\x1b[<64;2;1M".to_vec(),
        })
    );
    assert!(!app.handle_mouse_event(mouse(
        MouseEventKind::Moved,
        right_inner.x + 1,
        right_inner.y,
        KeyModifiers::NONE,
    )));

    // Alt+Down overrides PTY forwarding, freezes PTY snapshot, and copies cleanly on release
    // even if Alt is released mid-drag and live PTY frame mutates `pty_screens`!
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        right_inner.x,
        right_inner.y,
        KeyModifiers::ALT,
    )));
    assert!(app.pending_actions.is_empty());
    assert!(app.frozen_pty_snapshot.is_some());
    assert!(app.pane_selection.as_ref().is_some_and(|s| s.dragging));

    // Mutate live PTY snapshot mid-drag to prove frozen snapshot protects selection.
    if let Some(snap) = app.pty_screens.get_mut("term1") {
        snap.cells[0] = pty_row_from_str("MUTATED LIVE FRAME", snap.cols as usize);
    }
    draw_app(&mut terminal, &mut app);
    let buf = terminal.backend().buffer();
    assert_eq!(buf[(right_inner.x, right_inner.y)].symbol(), "e");

    // Drag across `│` border (without Alt) -> stays in host drag override and clamps.
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        0,
        right_inner.y,
        KeyModifiers::NONE,
    )));
    draw_app(&mut terminal, &mut app);
    let term_buf = terminal.backend().buffer();
    let sel_bg = app.theme_palette.selection_bg;
    assert_ne!(
        term_buf[(right_inner.left() - 1, right_inner.y)].bg,
        sel_bg,
        "terminal highlight bled into `│` border"
    );
    assert_eq!(term_buf[(right_inner.x, right_inner.y)].bg, sel_bg);
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        right_inner.right() + 5,
        right_inner.y,
        KeyModifiers::NONE,
    )));
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Up(MouseButton::Left),
        right_inner.right() + 5,
        right_inner.y,
        KeyModifiers::NONE,
    )));
    assert!(app.frozen_pty_snapshot.is_none());
    assert_eq!(app.last_copied_text.as_deref(), Some("echo hi 🦀!"));
    assert!(app.pending_actions.is_empty());

    // After release, next draw renders live updated PTY frame.
    draw_app(&mut terminal, &mut app);
    assert_eq!(
        terminal.backend().buffer()[(right_inner.x, right_inner.y)].symbol(),
        "M"
    );
}

#[test]
fn test_terminal_double_triple_click_and_click_to_focus_tree_composer() {
    let (mut app, mut terminal) = setup_terminal_app(PtyMouseMode::None);
    let right_inner = app.pane_rects.right_inner.unwrap();
    let tree_inner = app.pane_rects.tree_inner.unwrap();

    // Double-click "hi" on row 0 ("echo hi 🦀!").
    let hi_x = right_inner.x + 5;
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        hi_x,
        right_inner.y,
        KeyModifiers::NONE,
    )));
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Up(MouseButton::Left),
        hi_x,
        right_inner.y,
        KeyModifiers::NONE,
    )));
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        hi_x,
        right_inner.y,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.last_copied_text.as_deref(), Some("hi"));
    assert_eq!(app.pane_selection.as_ref().map(|s| s.pane), Some(SelectionPane::Terminal));

    // Triple-click selects whole trimmed row.
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        hi_x,
        right_inner.y,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.last_copied_text.as_deref(), Some("echo hi 🦀!"));

    // Click row 0 in `tree_inner` clears `pane_selection`, focuses `Tree`, selects row 0 ("t1").
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        tree_inner.x + 1,
        tree_inner.y,
        KeyModifiers::NONE,
    )));
    assert!(app.pane_selection.is_none());
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert_eq!(app.tree.selected_index(), Some(0));

    // Switch back to agent thread `t1`, render so `composer_inner` appears, then click `composer_inner`!
    app.switch_thread("t1");
    app.focus_set_tree_for_test();
    draw_app(&mut terminal, &mut app);
    let composer_inner = app.pane_rects.composer_inner.expect("composer_inner visible");
    assert_eq!(app.focused_pane(), FocusedPane::Tree);
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        composer_inner.x + 1,
        composer_inner.y,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
}

#[test]
fn test_worktree_diff_selection_and_worktree_row_click() {
    let mut app = setup_agent_app();
    app.worktrees = vec![
        WorktreeEntry {
            repo_root: "/repo".to_string(),
            repo_name: "grein".to_string(),
            path: "/repo".to_string(),
            branch: Some("main".to_string()),
            head_short: Some("abc".to_string()),
            is_primary: true,
            is_detached: false,
            is_grein_managed: false,
            dirty_count: 0,
            attached_thread_ids: vec!["t1".to_string()],
        },
        WorktreeEntry {
            repo_root: "/repo".to_string(),
            repo_name: "grein".to_string(),
            path: "/repo/.grein/worktrees/wt-1".to_string(),
            branch: Some("wt-1".to_string()),
            head_short: Some("def".to_string()),
            is_primary: false,
            is_detached: false,
            is_grein_managed: true,
            dirty_count: 1,
            attached_thread_ids: vec![],
        },
    ];
    app.sidebar_view = SidebarView::Worktrees;
    app.set_focus(FocusedPane::Tree);
    app.worktree_selected_idx = 1;
    app.worktree_diff_preview = "diff --git a/src/lib.rs b/src/lib.rs\n+let ok = 1;\n".to_string();

    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    draw_app(&mut terminal, &mut app);
    let tree_inner = app.pane_rects.tree_inner.unwrap();
    let right_inner = app.pane_rects.right_inner.unwrap();

    // Drag across "+let ok = 1;" in WorktreeDiff pane.
    let diff_row = app
        .last_rendered_lines
        .iter()
        .position(|l| l.spans.iter().any(|s| s.content.contains("+let ok = 1;")))
        .expect("diff line");
    let vis_y = right_inner.y + diff_row as u16;
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        right_inner.x,
        vis_y,
        KeyModifiers::NONE,
    )));
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        right_inner.x + 11,
        vis_y,
        KeyModifiers::NONE,
    )));
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Up(MouseButton::Left),
        right_inner.x + 11,
        vis_y,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.last_copied_text.as_deref(), Some("+let ok = 1;"));
    assert_eq!(
        app.pane_selection.as_ref().map(|s| s.pane),
        Some(SelectionPane::WorktreeDiff)
    );

    // Click row 2 in `tree_inner` selects worktree entry index 2 and clears selection.
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        tree_inner.x + 1,
        tree_inner.y + 2,
        KeyModifiers::NONE,
    )));
    assert!(app.pane_selection.is_none());
    assert_eq!(app.worktree_selected_idx, 2);
}

#[test]
fn test_composer_paste_normalization_ctrl_v_middle_click_and_inline_prompts() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = setup_agent_app();
    let mut terminal = Terminal::new(TestBackend::new(70, 20)).unwrap();
    draw_app(&mut terminal, &mut app);

    // 1. Composer `\r\n` and `\r` -> `\n` normalization.
    app.set_focus(FocusedPane::Composer);
    assert!(app.handle_paste_event("line1\r\nline2\rline3"));
    assert_eq!(app.input_buffer, "line1\nline2\nline3");

    // 2. Inline prompt paste: rename_prompt, palette_active, worktree_filter_active, transcript_search.
    app.set_focus(FocusedPane::Tree);
    app.rename_prompt = Some(RenamePromptState {
        thread_id: "t1".to_string(),
        buffer: "ab".to_string(),
        cursor: 1,
    });
    assert!(app.handle_paste_event("X\r\nY\n"));
    assert_eq!(app.rename_prompt.as_ref().unwrap().buffer, "aXYb");
    app.rename_prompt = None;

    app.open_command_palette("th");
    assert!(app.handle_paste_event("em\ne\r\n"));
    assert_eq!(app.palette_input, "theme");
    app.palette_active = false;

    app.sidebar_view = SidebarView::Worktrees;
    app.worktree_filter_active = true;
    assert!(app.handle_paste_event("feat\r\n-1"));
    assert_eq!(app.worktree_filter_query, "feat-1");
    app.worktree_filter_active = false;
    app.sidebar_view = SidebarView::Grein;

    app.transcript_search.editing = true;
    assert!(app.handle_paste_event("alpha\n beta\r\n"));
    assert_eq!(app.transcript_search.query, "alpha beta");
    app.transcript_search.clear();

    // Tree with no active prompt ignores paste.
    assert!(!app.handle_paste_event("ignored"));

    // 3. Ctrl+V in Composer reads clipboard; Middle click reads primary -> clipboard and focuses hovered Composer.
    {
        let _clip = ClipboardMockGuard::new("clip\r\npayload", Some("primary-payload"));
        app.input_buffer.clear();
        app.cursor_position = 0;
        app.set_focus(FocusedPane::Composer);
        assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)));
        assert_eq!(app.input_buffer, "clip\npayload");

        // Switch focus to Tree, middle-click inside Composer -> focuses Composer and pastes primary!
        app.set_focus(FocusedPane::Tree);
        draw_app(&mut terminal, &mut app);
        let composer_inner = app.pane_rects.composer_inner.unwrap();
        app.input_buffer.clear();
        app.cursor_position = 0;
        assert!(app.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Middle),
            composer_inner.x + 1,
            composer_inner.y,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.focused_pane(), FocusedPane::Composer);
        assert_eq!(app.input_buffer, "primary-payload");
    }

    // 4. `>_` Terminal Ctrl+V still emits raw `\x16` untouched!
    app.switch_thread("term1");
    app.pending_actions.clear();
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyWrite {
            thread_id: "term1".to_string(),
            bytes: vec![0x16],
        })
    );

    // 5. Middle-click with no primary falls back to clipboard, focuses hovered Terminal, and writes PTY paste.
    {
        let _clip = ClipboardMockGuard::new("fallback-clip", None);
        app.set_focus(FocusedPane::Tree);
        draw_app(&mut terminal, &mut app);
        let right_inner = app.pane_rects.right_inner.unwrap();
        app.pending_actions.clear();
        assert!(app.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Middle),
            right_inner.x + 1,
            right_inner.y,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.focused_pane(), FocusedPane::Terminal);
        assert_eq!(
            app.pending_actions.pop_front(),
            Some(AppAction::PtyWrite {
                thread_id: "term1".to_string(),
                bytes: b"fallback-clip".to_vec(),
            })
        );
    }
}

#[test]
fn test_pty_scrollback_bypasses_forwarding_and_scrolled_tree_click_and_headless_fallback() {
    let (mut app, mut terminal) = setup_terminal_app(PtyMouseMode::PressRelease);
    let right_inner = app.pane_rects.right_inner.unwrap();
    if let Some(snap) = app.pty_screens.get_mut("term1") {
        snap.scrollback_offset = 2;
    }
    app.pending_actions.clear();

    // ScrollUp with scrollback_offset > 0 must NOT emit PtyWrite mouse SGR; dispatches PtyScroll.
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::ScrollUp,
        right_inner.x + 1,
        right_inner.y,
        KeyModifiers::NONE,
    )));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::PtyScroll {
            thread_id: "term1".to_string(),
            delta: 3,
        })
    );

    // Scrolled Grein tree click maps `scroll_offset + (row - tree_inner.y)`.
    app.threads.push(ThreadItem {
        id: "t3".to_string(),
        title: "Gamma".to_string(),
        custom_name: None,
        anchors: Vec::new(),
    });
    app.thread_meta
        .insert("t3".to_string(), make_meta("t3", Some("t1"), ThreadKind::Agent));
    grein_tui::runner::rebuild_tree_nodes(&mut app);
    app.set_focus(FocusedPane::Tree);
    draw_app(&mut terminal, &mut app);
    let tree_inner = app.pane_rects.tree_inner.unwrap();
    app.tree.scroll_offset = 1;
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        tree_inner.x + 1,
        tree_inner.y + 1,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.tree.selected_index(), Some(2));

    // Headless fallback when `pane_rects` is cleared but `last_frame_area` is set.
    app.switch_thread("t1");
    app.set_focus(FocusedPane::Tree);
    draw_app(&mut terminal, &mut app);
    let expected_composer = app.pane_rects.composer_inner.unwrap();
    app.pane_rects = Default::default();
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        expected_composer.x + 1,
        expected_composer.y,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
}

trait AppTestExt {
    fn focus_set_tree_for_test(&mut self);
}

impl AppTestExt for App {
    fn focus_set_tree_for_test(&mut self) {
        self.set_focus(FocusedPane::Tree);
    }
}

struct InjectedTestClipboard {
    copied: Mutex<Vec<String>>,
    clipboard_result: Mutex<Result<String, String>>,
    primary_result: Mutex<Result<String, String>>,
}

impl InjectedTestClipboard {
    fn new(clipboard: Result<&str, &str>, primary: Result<&str, &str>) -> Self {
        Self {
            copied: Mutex::new(Vec::new()),
            clipboard_result: Mutex::new(
                clipboard.map(str::to_string).map_err(str::to_string),
            ),
            primary_result: Mutex::new(primary.map(str::to_string).map_err(str::to_string)),
        }
    }

    fn copied(&self) -> Vec<String> {
        self.copied.lock().unwrap().clone()
    }

    fn set_primary(&self, primary: Result<&str, &str>) {
        *self.primary_result.lock().unwrap() =
            primary.map(str::to_string).map_err(str::to_string);
    }
}

impl grein_tui::ClipboardPort for InjectedTestClipboard {
    fn copy(&self, text: &str) -> Result<String, String> {
        self.copied.lock().unwrap().push(text.to_string());
        Ok("injected-test".to_string())
    }

    fn read_clipboard(&self) -> Result<String, String> {
        self.clipboard_result.lock().unwrap().clone()
    }

    fn read_primary(&self) -> Result<String, String> {
        self.primary_result.lock().unwrap().clone()
    }
}

#[test]
fn test_app_with_services_injected_clipboard_copy_ctrl_v_and_middle_click_without_env() {
    use std::sync::Arc;
    use grein_tui::TuiServices;

    let clip = Arc::new(InjectedTestClipboard::new(
        Ok("injected\r\nclip"),
        Ok("injected-primary"),
    ));
    let services = TuiServices::default().with_clipboard(clip.clone());
    let mut app = App::with_services(services);
    let default_app = App::new();
    assert_eq!(app, default_app, "TuiServices PartialEq preserves App equality");

    app.active_thread_id = "t1".to_string();
    app.threads = vec![
        ThreadItem {
            id: "t1".to_string(),
            title: "Alpha".to_string(),
            custom_name: None,
            anchors: Vec::new(),
        },
        ThreadItem {
            id: "term1".to_string(),
            title: "Shell".to_string(),
            custom_name: None,
            anchors: Vec::new(),
        },
    ];
    app.thread_meta
        .insert("t1".to_string(), make_meta("t1", None, ThreadKind::Agent));
    app.thread_meta.insert(
        "term1".to_string(),
        make_meta("term1", Some("t1"), ThreadKind::Terminal),
    );
    app.active_transcript_md = "# User: hi\n\n## Assistant\nreply payload\n\n".to_string();
    app.thread_transcripts
        .insert("t1".to_string(), app.active_transcript_md.clone());
    app.sync_anchors();
    app.select_active_thread_in_tree();

    // 1. Copy via /copy routes through injected ClipboardPort
    app.submit_composer_prompt("/copy");
    assert_eq!(clip.copied(), vec!["reply payload".to_string()]);
    assert_eq!(app.last_copied_text.as_deref(), Some("reply payload"));
    assert_eq!(
        app.status_notice.as_deref(),
        Some("Copied last reply (13 chars) via injected-test")
    );

    // 2. Ctrl+V in Composer reads from injected ClipboardPort
    app.set_focus(FocusedPane::Composer);
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)));
    assert_eq!(app.input_buffer, "injected\nclip");

    // 3. Ctrl+V in rename prompt, palette, worktree filter, and search all use injected ClipboardPort
    app.set_focus(FocusedPane::Tree);
    app.rename_prompt = Some(RenamePromptState {
        thread_id: "t1".to_string(),
        buffer: String::new(),
        cursor: 0,
    });
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)));
    assert_eq!(
        app.rename_prompt.as_ref().unwrap().buffer,
        "injectedclip"
    );
    app.rename_prompt = None;

    app.open_command_palette("");
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)));
    assert_eq!(app.palette_input, "injectedclip");
    app.palette_active = false;

    // 4. Middle click reads primary from injected ClipboardPort, then falls back to clipboard
    let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
    draw_app(&mut terminal, &mut app);
    let composer_inner = app.pane_rects.composer_inner.unwrap();
    app.input_buffer.clear();
    app.cursor_position = 0;
    app.set_focus(FocusedPane::Tree);
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Middle),
        composer_inner.x + 1,
        composer_inner.y,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    assert_eq!(app.input_buffer, "injected-primary");

    clip.set_primary(Err("primary unavailable"));
    app.input_buffer.clear();
    app.cursor_position = 0;
    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Middle),
        composer_inner.x + 1,
        composer_inner.y,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.input_buffer, "injected\nclip");
}

#[tokio::test]
async fn test_run_with_engine_events_and_services_injects_clipboard_port() {
    use std::io;
    use std::sync::Arc;
    use crossterm::event::Event;
    use grein_tui::engine_port::{
        HydratedSession, HydratedThread, HydratedTurn, TuiEngineEvent, TuiEnginePort,
    };
    use grein_tui::runner::run_with_engine_events_and_services;
    use grein_tui::TuiServices;
    use tokio::sync::mpsc;

    struct MinimalEngine;
    #[async_trait::async_trait]
    impl TuiEnginePort for MinimalEngine {
        async fn hydrate_session(&self) -> io::Result<HydratedSession> {
            Ok(HydratedSession {
                active_thread_id: "t-1".to_string(),
                threads: vec![HydratedThread {
                    id: "t-1".to_string(),
                    title: "Main".to_string(),
                    custom_name: None,
                    turns: vec![HydratedTurn {
                        id: "turn-1".to_string(),
                        role: "Assistant".to_string(),
                        content: "from-runner".to_string(),
                    }],
                }],
            })
        }

        async fn send_turn(&self, _thread_id: &str, _prompt: &str) -> io::Result<()> {
            Ok(())
        }

        async fn fork_thread(&self, _thread_id: &str, _anchor_id: &str) -> io::Result<String> {
            Ok("t-2".to_string())
        }

        async fn merge_thread(&self, _source_id: &str, _target_id: &str) -> io::Result<()> {
            Ok(())
        }

        async fn resolve_permission(&self, _request_id: &str, _approved: bool) -> io::Result<()> {
            Ok(())
        }

        async fn subscribe_events(&self) -> io::Result<mpsc::UnboundedReceiver<TuiEngineEvent>> {
            let (_tx, rx) = mpsc::unbounded_channel();
            Ok(rx)
        }
    }

    let clip = Arc::new(InjectedTestClipboard::new(
        Ok("/copy"),
        Ok("unused-primary"),
    ));
    let services = TuiServices::default().with_clipboard(clip.clone());
    let engine: Arc<dyn TuiEnginePort> = Arc::new(MinimalEngine);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let (key_tx, key_rx) = mpsc::unbounded_channel();

    // Ctrl+V pastes "/copy", then Enter copies thread transcript (empty or notice), then Ctrl+G + q quits.
    key_tx
        .send(Event::Key(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::CONTROL,
        )))
        .unwrap();
    key_tx
        .send(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)))
        .unwrap();
    key_tx
        .send(Event::Key(KeyEvent::new(
            KeyCode::Char('g'),
            KeyModifiers::CONTROL,
        )))
        .unwrap();
    key_tx
        .send(Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )))
        .unwrap();
    drop(key_tx);

    run_with_engine_events_and_services(&mut terminal, App::new(), engine, services, key_rx)
        .await
        .unwrap();

    assert_eq!(clip.copied(), vec!["from-runner".to_string()]);
}
