use grein_tui::engine_port::PtyCell;
use grein_tui::markdown::{wrap_styled_lines_with_meta, LineMeta};
use grein_tui::selection::{
    apply_selection_highlight, extract_lines_selection, extract_pty_selection, word_bounds_at,
    PaneRects, PaneSelection, SelectionPane,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

fn pty_cell(s: &str) -> PtyCell {
    PtyCell {
        contents: s.to_string(),
        ..PtyCell::default()
    }
}

fn pty_wide_pair(ch: &str) -> [PtyCell; 2] {
    [
        PtyCell {
            contents: ch.to_string(),
            ..PtyCell::default()
        },
        PtyCell {
            contents: String::new(),
            is_wide_continuation: true,
            ..PtyCell::default()
        },
    ]
}

fn pty_row_from_str(text: &str, total_cols: usize) -> Vec<PtyCell> {
    let mut row = Vec::with_capacity(total_cols);
    for ch in text.chars() {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        let w = Span::raw(&*s).width();
        if w == 2 {
            row.extend(pty_wide_pair(s));
        } else {
            row.push(pty_cell(s));
        }
    }
    while row.len() < total_cols {
        row.push(PtyCell::default());
    }
    row
}

#[test]
fn test_pane_selection_ordered_and_pane_rects_default() {
    let forward = PaneSelection {
        pane: SelectionPane::Transcript,
        anchor: (1, 3),
        focus: (4, 2),
        dragging: true,
    };
    assert_eq!(forward.ordered(), ((1, 3), (4, 2)));

    let backward_multi = PaneSelection {
        pane: SelectionPane::Terminal,
        anchor: (5, 1),
        focus: (2, 8),
        dragging: false,
    };
    assert_eq!(backward_multi.ordered(), ((2, 8), (5, 1)));

    let backward_same_row = PaneSelection {
        pane: SelectionPane::WorktreeDiff,
        anchor: (3, 9),
        focus: (3, 2),
        dragging: false,
    };
    assert_eq!(backward_same_row.ordered(), ((3, 2), (3, 9)));

    let rects = PaneRects::default();
    assert_eq!(rects.tree_inner, None);
    assert_eq!(rects.right_inner, None);
    assert_eq!(rects.composer_inner, None);
}

#[test]
fn test_extract_lines_selection_single_multi_and_trailing_spaces() {
    let lines = vec![
        Line::from(vec![Span::raw("hello "), Span::raw("world   ")]),
        Line::raw("second line   "),
        Line::raw("   "),
        Line::raw("last"),
    ];
    let meta = vec![LineMeta::default(); lines.len()];

    assert_eq!(
        extract_lines_selection(&lines, &meta, (0, 1), (0, 4)),
        "ello"
    );
    assert_eq!(
        extract_lines_selection(&lines, &meta, (0, 2), (0, 2)),
        "l"
    );
    assert_eq!(
        extract_lines_selection(&lines, &meta, (0, 0), (0, 5)),
        "hello "
    );
    assert_eq!(
        extract_lines_selection(&lines, &meta, (0, 6), (0, 99)),
        "world"
    );
    assert_eq!(
        extract_lines_selection(&lines, &meta, (0, 12), (0, 20)),
        ""
    );
    assert_eq!(
        extract_lines_selection(&lines, &meta, (0, 6), (3, 2)),
        "world\nsecond line\n\nlas"
    );
    // Reverse endpoint normalization.
    assert_eq!(
        extract_lines_selection(&lines, &meta, (1, 5), (0, 6)),
        "world\nsecond"
    );
}

#[test]
fn test_extract_lines_selection_wide_emojis_and_combining_marks() {
    let lines = vec![
        Line::from(vec![Span::raw("a"), Span::raw("🦀b"), Span::raw("日本語")]),
        Line::raw("e\u{0301}x"),
    ];
    let meta = vec![LineMeta::default(); lines.len()];

    // "a" [0], "🦀" [1..=2], "b" [3], "日" [4..=5], "本" [6..=7], "語" [8..=9]
    assert_eq!(
        extract_lines_selection(&lines, &meta, (0, 1), (0, 2)),
        "🦀"
    );
    // Starting on second half of emoji snaps cleanly to include the whole glyph.
    assert_eq!(
        extract_lines_selection(&lines, &meta, (0, 2), (0, 3)),
        "🦀b"
    );
    // Ending on first half of emoji expands cleanly to include the whole glyph.
    assert_eq!(
        extract_lines_selection(&lines, &meta, (0, 0), (0, 1)),
        "a🦀"
    );
    assert_eq!(
        extract_lines_selection(&lines, &meta, (0, 4), (0, 7)),
        "日本"
    );
    // Combining accent stays attached to 'e' (col 0), never leaks into 'x' (col 1).
    assert_eq!(
        extract_lines_selection(&lines, &meta, (1, 0), (1, 0)),
        "e\u{0301}"
    );
    assert_eq!(
        extract_lines_selection(&lines, &meta, (1, 1), (1, 1)),
        "x"
    );
}

#[test]
fn test_extract_lines_selection_soft_wrapped_prose_and_url() {
    let raw = vec![
        Line::raw("alpha beta gamma delta"),
        Line::raw("See https://example.com/very/long/path/token"),
        Line::raw("      indented"),
    ];
    let (wrapped, meta) = wrap_styled_lines_with_meta(raw, 12);

    // Select full wrapped prose across soft wrap: joined with single space.
    assert_eq!(
        extract_lines_selection(&wrapped, &meta, (0, 0), (1, 20)),
        "alpha beta gamma delta"
    );
    // Partial selection across soft wrap boundary.
    assert_eq!(
        extract_lines_selection(&wrapped, &meta, (0, 6), (1, 4)),
        "beta gamma"
    );

    // Wrapped URL rows: row 2 ("See"), rows 3..=6 (URL fragments).
    let url_full = extract_lines_selection(&wrapped, &meta, (2, 0), (6, 20));
    assert_eq!(url_full, "See https://example.com/very/long/path/token");

    // Leading-indent soft-wrap preserves indentation spaces without inserting extra spaces.
    assert_eq!(
        extract_lines_selection(&wrapped, &meta, (7, 0), (8, 20)),
        "      indented"
    );

    // Drag starting past EOL on "alpha beta" into soft-wrapped "gamma delta" must not prepend a space.
    assert_eq!(
        extract_lines_selection(&wrapped, &meta, (0, 20), (1, 11)),
        "gamma delta"
    );
}

#[test]
fn test_extract_pty_selection_wide_padding_and_row_wrapped() {
    let row0 = pty_row_from_str("hi 🦀!", 10);
    let mut row1 = pty_row_from_str("hello ", 6);
    row1.truncate(6);
    let row2 = pty_row_from_str("world", 10);
    let mut row3 = vec![pty_cell("a"), PtyCell::default(), pty_cell("b")];
    row3.resize_with(10, PtyCell::default);

    let cells = vec![row0, row1, row2, row3];
    // Row 0 not wrapped, Row 1 wrapped onto Row 2, Row 2 not wrapped, Row 3 not wrapped.
    let row_wrapped = vec![false, true, false, false];

    // Row 0: 'h'(0) 'i'(1) ' '(2) '🦀'(3) cont(4) '!'(5) [6..9 blank padding]
    assert_eq!(
        extract_pty_selection(&cells, &row_wrapped, (0, 4), (0, 9)),
        "🦀!"
    );
    assert_eq!(
        extract_pty_selection(&cells, &row_wrapped, (0, 0), (0, 3)),
        "hi 🦀"
    );
    // Clicking inside trailing blank padding yields empty string.
    assert_eq!(
        extract_pty_selection(&cells, &row_wrapped, (0, 6), (0, 9)),
        ""
    );

    // Wrapped Row 1 ("hello ") + Row 2 ("world     ") joins without newline and preserves row 1's space.
    assert_eq!(
        extract_pty_selection(&cells, &row_wrapped, (1, 0), (2, 9)),
        "hello world"
    );

    // Interior empty cell becomes ' ', trailing blank cells trimmed.
    assert_eq!(
        extract_pty_selection(&cells, &row_wrapped, (3, 0), (3, 9)),
        "a b"
    );

    // Multi-row across non-wrapped and wrapped boundaries.
    assert_eq!(
        extract_pty_selection(&cells, &row_wrapped, (0, 0), (3, 2)),
        "hi 🦀!\nhello world\na b"
    );
}

#[test]
fn test_word_bounds_at_words_whitespace_symbols_and_wide_chars() {
    let line = "hello_world  foo::bar 🦀🦀 日本語";
    assert_eq!(word_bounds_at(line, 0), Some((0, 10)));
    assert_eq!(word_bounds_at(line, 5), Some((0, 10)));
    assert_eq!(word_bounds_at(line, 11), Some((11, 12)));
    assert_eq!(word_bounds_at(line, 12), Some((11, 12)));
    assert_eq!(word_bounds_at(line, 13), Some((13, 15)));
    assert_eq!(word_bounds_at(line, 16), Some((16, 17)));
    assert_eq!(word_bounds_at(line, 17), Some((16, 17)));
    assert_eq!(word_bounds_at(line, 18), Some((18, 20)));
    // Emoji symbol run "🦀🦀" at cols 22..=25.
    assert_eq!(word_bounds_at(line, 22), Some((22, 25)));
    assert_eq!(word_bounds_at(line, 25), Some((22, 25)));
    // CJK word run "日本語" at cols 27..=32.
    assert_eq!(word_bounds_at(line, 27), Some((27, 32)));
    assert_eq!(word_bounds_at(line, 32), Some((27, 32)));
    assert_eq!(word_bounds_at(line, 33), None);
    assert_eq!(word_bounds_at("", 0), None);
    assert_eq!(word_bounds_at("re\u{0301}sume", 1), Some((0, 5)));
    assert_eq!(word_bounds_at("re\u{0301}sume", 3), Some((0, 5)));

    // Round-trip double-click bounds into extract_lines_selection.
    let lines = vec![Line::raw(line)];
    let meta = vec![LineMeta::default()];
    let (ws, we) = word_bounds_at(line, 23).unwrap();
    assert_eq!(
        extract_lines_selection(&lines, &meta, (0, ws), (0, we)),
        "🦀🦀"
    );
}

#[test]
fn test_apply_selection_highlight_isolated_from_sidebar_and_borders() {
    let total = Rect::new(0, 0, 24, 6);
    let mut buf = Buffer::empty(total);

    let sidebar_area = Rect::new(0, 0, 8, 6);
    let right_area = Rect::new(8, 0, 16, 6);
    let sidebar_block = Block::default().borders(Borders::ALL);
    let right_block = Block::default().borders(Borders::ALL);

    let rects = PaneRects {
        tree_inner: Some(sidebar_block.inner(sidebar_area)),
        right_inner: Some(right_block.inner(right_area)),
        composer_inner: None,
    };
    let right_inner = rects.right_inner.unwrap();
    let tree_inner = rects.tree_inner.unwrap();

    sidebar_block.render(sidebar_area, &mut buf);
    right_block.render(right_area, &mut buf);

    Paragraph::new(vec![Line::raw("thread")]).render(tree_inner, &mut buf);

    let transcript_lines = vec![
        Line::raw("line zero"),
        Line::raw("line one"),
        Line::from(Span::styled(
            "selected row",
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )),
        Line::raw("line three"),
    ];
    let meta = vec![LineMeta::default(); transcript_lines.len()];
    Paragraph::new(transcript_lines.clone())
        .scroll((1, 0))
        .render(right_inner, &mut buf);

    // Select from row 1 col 5 ("one") through row 2 col 7 ("selected").
    let sel = PaneSelection {
        pane: SelectionPane::Transcript,
        anchor: (2, 7),
        focus: (1, 5),
        dragging: false,
    };
    apply_selection_highlight(&mut buf, right_inner, 1, &sel, Color::Blue, Color::White);

    // Sidebar cells and borders must remain untouched.
    for y in 0..total.height {
        for x in 0..right_inner.left() {
            assert_ne!(buf[(x, y)].bg, Color::Blue, "bled into ({x}, {y})");
        }
        assert_ne!(
            buf[(right_area.right() - 1, y)].bg,
            Color::Blue,
            "bled into right border at y={y}"
        );
    }
    for x in right_area.left()..right_area.right() {
        assert_ne!(buf[(x, right_area.top())].bg, Color::Blue);
        assert_ne!(buf[(x, right_area.bottom() - 1)].bg, Color::Blue);
    }

    // Visual row 0 (content row 1, scroll_y=1): cols 5..right_inner.width highlighted.
    assert_ne!(buf[(right_inner.x + 4, right_inner.y)].bg, Color::Blue);
    assert_eq!(buf[(right_inner.x + 5, right_inner.y)].bg, Color::Blue);
    assert_eq!(buf[(right_inner.x + 5, right_inner.y)].fg, Color::White);

    // Visual row 1 (content row 2): cols 0..=7 highlighted, BOLD kept, REVERSED stripped.
    assert_eq!(buf[(right_inner.x, right_inner.y + 1)].bg, Color::Blue);
    assert!(buf[(right_inner.x, right_inner.y + 1)]
        .modifier
        .contains(Modifier::BOLD));
    assert!(!buf[(right_inner.x, right_inner.y + 1)]
        .modifier
        .contains(Modifier::REVERSED));
    assert_eq!(buf[(right_inner.x + 7, right_inner.y + 1)].bg, Color::Blue);
    assert_ne!(buf[(right_inner.x + 8, right_inner.y + 1)].bg, Color::Blue);

    // Extracted text contains only transcript content, never sidebar or border characters.
    let (start, end) = sel.ordered();
    let extracted = extract_lines_selection(&transcript_lines, &meta, start, end);
    assert_eq!(extracted, "one\nselected");
    assert!(!extracted.contains('│'));
    assert!(!extracted.contains("thread"));
}
