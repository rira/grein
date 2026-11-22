use grein_tui::search::{
    highlight_lines_with_matches, SearchDirection, SearchMatch, TranscriptSearchState,
};
use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

fn join_line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn test_smart_case_lowercase_query_matches_case_insensitively() {
    let lines = vec![
        Line::from("Hello world"),
        Line::from("HELLO again"),
        Line::from("say hello"),
    ];
    let mut state = TranscriptSearchState::default();
    state.open(SearchDirection::Forward);
    state.query = "hello".to_string();
    state.recompute_matches(&lines, None);

    assert_eq!(
        state.matches,
        vec![
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_end: 5,
            },
            SearchMatch {
                line_index: 1,
                byte_start: 0,
                byte_end: 5,
            },
            SearchMatch {
                line_index: 2,
                byte_start: 4,
                byte_end: 9,
            },
        ]
    );
    assert_eq!(state.current_index, Some(0));
    assert_eq!(state.counter_label(), "[1/3]");
}

#[test]
fn test_smart_case_mixed_case_query_matches_case_sensitively() {
    let lines = vec![
        Line::from("Hello world"),
        Line::from("HELLO again"),
        Line::from("say hello"),
        Line::from("Another Hello here"),
    ];
    let mut state = TranscriptSearchState::default();
    state.open(SearchDirection::Forward);
    state.query = "Hello".to_string();
    state.recompute_matches(&lines, None);

    assert_eq!(
        state.matches,
        vec![
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_end: 5,
            },
            SearchMatch {
                line_index: 3,
                byte_start: 8,
                byte_end: 13,
            },
        ]
    );
    assert_eq!(state.counter_label(), "[1/2]");
}

#[test]
fn test_unicode_expanding_lowercase_preserves_valid_utf8_offsets() {
    let raw_line = "İ Hello Ⱥ world";
    let lines = vec![Line::from(raw_line)];

    let mut state = TranscriptSearchState::default();
    state.open(SearchDirection::Forward);
    state.query = "world".to_string();
    state.recompute_matches(&lines, None);

    assert_eq!(state.matches.len(), 1);
    let m = &state.matches[0];
    assert_eq!(&raw_line[m.byte_start..m.byte_end], "world");

    state.query = "hello".to_string();
    state.recompute_matches(&lines, None);
    assert_eq!(state.matches.len(), 1);
    let m = &state.matches[0];
    assert_eq!(&raw_line[m.byte_start..m.byte_end], "Hello");

    state.query = "ⱥ".to_string();
    state.recompute_matches(&lines, None);
    assert_eq!(state.matches.len(), 1);
    let m = &state.matches[0];
    assert_eq!(&raw_line[m.byte_start..m.byte_end], "Ⱥ");

    state.query = "i".to_string();
    state.recompute_matches(&lines, None);
    for m in &state.matches {
        assert!(raw_line.is_char_boundary(m.byte_start));
        assert!(raw_line.is_char_boundary(m.byte_end));
    }

    let highlighted = highlight_lines_with_matches(lines, &state.matches, state.current_index);
    assert_eq!(join_line_text(&highlighted[0]), raw_line);
}

#[test]
fn test_multi_span_utf8_emoji_highlighting_preserves_styles_and_line_metadata() {
    let cyan_bold = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let gray = Style::default().fg(Color::Gray);
    let line_style = Style::default().bg(Color::Black);

    let line0 = Line::from(vec![
        Span::styled("🚀 ", cyan_bold),
        Span::styled("foo bar", gray),
        Span::styled(" ✦ baz", cyan_bold),
    ])
    .style(line_style)
    .alignment(Alignment::Center);

    let line1 = Line::from(Span::styled("untouched line", gray)).style(line_style);

    let mut state = TranscriptSearchState::default();
    state.open(SearchDirection::Forward);
    state.query = "foo".to_string();
    let lines = vec![line0.clone(), line1.clone()];
    state.recompute_matches(&lines, None);

    let mut matches = state.matches.clone();
    let bar_start = "🚀 foo ".len();
    let bar_end = bar_start + "bar".len();
    matches.push(SearchMatch {
        line_index: 0,
        byte_start: bar_start,
        byte_end: bar_end,
    });

    let highlighted = highlight_lines_with_matches(lines, &matches, Some(1));
    assert_eq!(highlighted.len(), 2);
    assert_eq!(join_line_text(&highlighted[0]), "🚀 foo bar ✦ baz");
    assert_eq!(highlighted[0].style, line_style);
    assert_eq!(highlighted[0].alignment, Some(Alignment::Center));
    assert_eq!(highlighted[1], line1);

    let inactive = Style::default().bg(Color::DarkGray).fg(Color::White);
    let active = Style::default()
        .bg(Color::Yellow)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);

    assert_eq!(
        highlighted[0].spans,
        vec![
            Span::styled("🚀 ", cyan_bold),
            Span::styled("foo", inactive),
            Span::styled(" ", gray),
            Span::styled("bar", active),
            Span::styled(" ✦ baz", cyan_bold),
        ]
    );
}

#[test]
fn test_single_span_emoji_slicing_at_exact_byte_boundaries() {
    let cyan_bold = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let line = Line::from(Span::styled("🚀 foo bar ✦ baz", cyan_bold));

    let foo_start = "🚀 ".len();
    let foo_end = foo_start + "foo".len();
    let bar_start = "🚀 foo ".len();
    let bar_end = bar_start + "bar".len();
    let matches = vec![
        SearchMatch {
            line_index: 0,
            byte_start: foo_start,
            byte_end: foo_end,
        },
        SearchMatch {
            line_index: 0,
            byte_start: bar_start,
            byte_end: bar_end,
        },
    ];

    let highlighted = highlight_lines_with_matches(vec![line], &matches, Some(0));
    let active = Style::default()
        .bg(Color::Yellow)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    let inactive = Style::default().bg(Color::DarkGray).fg(Color::White);

    assert_eq!(
        highlighted[0].spans,
        vec![
            Span::styled("🚀 ", cyan_bold),
            Span::styled("foo", active),
            Span::styled(" ", cyan_bold),
            Span::styled("bar", inactive),
            Span::styled(" ✦ baz", cyan_bold),
        ]
    );
}

#[test]
fn test_multiple_non_overlapping_matches_on_single_line() {
    let lines = vec![Line::from("aaaaa")];
    let mut state = TranscriptSearchState::default();
    state.open(SearchDirection::Forward);
    state.query = "aa".to_string();
    state.recompute_matches(&lines, None);

    assert_eq!(
        state.matches,
        vec![
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_end: 2,
            },
            SearchMatch {
                line_index: 0,
                byte_start: 2,
                byte_end: 4,
            },
        ]
    );

    let highlighted = highlight_lines_with_matches(lines, &state.matches, Some(0));
    let active = Style::default()
        .bg(Color::Yellow)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    let inactive = Style::default().bg(Color::DarkGray).fg(Color::White);

    assert_eq!(
        highlighted[0].spans,
        vec![
            Span::styled("aa", active),
            Span::styled("aa", inactive),
            Span::styled("a", Style::default()),
        ]
    );
}

#[test]
fn test_anchor_selection_and_direction_wrapping() {
    let lines = vec![
        Line::from("blank"),
        Line::from("hit one"),
        Line::from("blank"),
        Line::from("hit three-a and hit three-b"),
        Line::from("blank"),
        Line::from("hit five"),
    ];

    let mut fwd = TranscriptSearchState::default();
    fwd.open(SearchDirection::Forward);
    fwd.query = "hit".to_string();
    fwd.recompute_matches(&lines, Some(3));
    assert_eq!(fwd.current_index, Some(1));
    assert_eq!(fwd.active_line_index(), Some(3));

    fwd.recompute_matches(&lines, Some(4));
    assert_eq!(fwd.current_index, Some(3));
    assert_eq!(fwd.active_line_index(), Some(5));

    fwd.recompute_matches(&lines, Some(6));
    assert_eq!(fwd.current_index, Some(0));
    assert_eq!(fwd.active_line_index(), Some(1));

    let mut rev = TranscriptSearchState::default();
    rev.open(SearchDirection::Reverse);
    rev.query = "hit".to_string();
    rev.recompute_matches(&lines, Some(3));
    assert_eq!(rev.current_index, Some(2));
    assert_eq!(rev.active_line_index(), Some(3));

    rev.recompute_matches(&lines, Some(2));
    assert_eq!(rev.current_index, Some(0));
    assert_eq!(rev.active_line_index(), Some(1));

    rev.recompute_matches(&lines, Some(0));
    assert_eq!(rev.current_index, Some(3));
    assert_eq!(rev.active_line_index(), Some(5));
}

#[test]
fn test_next_and_prev_match_navigation_and_counter_label() {
    let lines = vec![
        Line::from("match 1"),
        Line::from("match 2"),
        Line::from("match 3"),
    ];

    let mut fwd = TranscriptSearchState::default();
    assert_eq!(fwd.counter_label(), "[0/0]");
    assert_eq!(fwd.next_match(), None);
    assert_eq!(fwd.prev_match(), None);

    fwd.open(SearchDirection::Forward);
    fwd.query = "match".to_string();
    fwd.recompute_matches(&lines, Some(0));
    assert_eq!(fwd.counter_label(), "[1/3]");

    assert_eq!(fwd.next_match().map(|m| m.line_index), Some(1));
    assert_eq!(fwd.counter_label(), "[2/3]");
    assert_eq!(fwd.next_match().map(|m| m.line_index), Some(2));
    assert_eq!(fwd.counter_label(), "[3/3]");
    assert_eq!(fwd.next_match().map(|m| m.line_index), Some(0));
    assert_eq!(fwd.counter_label(), "[1/3]");

    assert_eq!(fwd.prev_match().map(|m| m.line_index), Some(2));
    assert_eq!(fwd.counter_label(), "[3/3]");
    assert_eq!(fwd.prev_match().map(|m| m.line_index), Some(1));
    assert_eq!(fwd.counter_label(), "[2/3]");

    let mut rev = TranscriptSearchState::default();
    rev.open(SearchDirection::Reverse);
    rev.query = "match".to_string();
    rev.recompute_matches(&lines, Some(2));
    assert_eq!(rev.counter_label(), "[3/3]");

    assert_eq!(rev.next_match().map(|m| m.line_index), Some(1));
    assert_eq!(rev.counter_label(), "[2/3]");
    assert_eq!(rev.next_match().map(|m| m.line_index), Some(0));
    assert_eq!(rev.counter_label(), "[1/3]");
    assert_eq!(rev.next_match().map(|m| m.line_index), Some(2));
    assert_eq!(rev.counter_label(), "[3/3]");

    assert_eq!(rev.prev_match().map(|m| m.line_index), Some(0));
    assert_eq!(rev.counter_label(), "[1/3]");
}

#[test]
fn test_lifecycle_open_confirm_clear_is_visible_and_empty_query() {
    let lines = vec![Line::from("alpha beta"), Line::from("gamma")];
    let mut state = TranscriptSearchState::default();

    assert!(!state.editing);
    assert!(!state.is_visible());
    assert_eq!(state.active_line_index(), None);

    state.open(SearchDirection::Forward);
    assert!(state.editing);
    assert!(state.is_visible());
    assert_eq!(state.direction, SearchDirection::Forward);

    state.query = "beta".to_string();
    state.recompute_matches(&lines, None);
    assert_eq!(state.active_line_index(), Some(0));

    state.confirm();
    assert!(!state.editing);
    assert!(state.is_visible());
    assert_eq!(state.active_line_index(), Some(0));

    state.query.clear();
    state.recompute_matches(&lines, None);
    assert!(state.matches.is_empty());
    assert_eq!(state.current_index, None);
    assert_eq!(state.active_line_index(), None);
    assert!(!state.is_visible());
    assert_eq!(state.counter_label(), "[0/0]");

    state.open(SearchDirection::Reverse);
    assert!(state.editing);
    assert!(state.is_visible());
    state.query = "gamma".to_string();
    state.recompute_matches(&lines, None);
    assert_eq!(state.active_line_index(), Some(1));

    state.clear();
    assert!(!state.editing);
    assert!(!state.is_visible());
    assert!(state.query.is_empty());
    assert!(state.matches.is_empty());
    assert_eq!(state.current_index, None);
    assert_eq!(state.active_line_index(), None);
}

#[test]
fn test_cross_span_match_and_defensive_highlight_inputs() {
    let cyan = Style::default().fg(Color::Cyan);
    let red = Style::default().fg(Color::Red);
    let lines = vec![Line::from(vec![
        Span::styled("fo", cyan),
        Span::styled("o!", red),
    ])];

    let mut state = TranscriptSearchState::default();
    state.open(SearchDirection::Forward);
    state.query = "foo".to_string();
    state.recompute_matches(&lines, None);
    assert_eq!(
        state.matches,
        vec![SearchMatch {
            line_index: 0,
            byte_start: 0,
            byte_end: 3,
        }]
    );

    let highlighted = highlight_lines_with_matches(lines.clone(), &state.matches, Some(0));
    let active = Style::default()
        .bg(Color::Yellow)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    assert_eq!(
        highlighted[0].spans,
        vec![
            Span::styled("fo", active),
            Span::styled("o", active),
            Span::styled("!", red),
        ]
    );

    let stale_matches = vec![
        SearchMatch {
            line_index: 99,
            byte_start: 0,
            byte_end: 2,
        },
        SearchMatch {
            line_index: 0,
            byte_start: 2,
            byte_end: 2,
        },
    ];
    let untouched = highlight_lines_with_matches(lines.clone(), &stale_matches, Some(0));
    assert_eq!(untouched, lines);
}

#[test]
fn test_reverse_search_without_anchor_and_reopen_resets_query() {
    let lines = vec![Line::from("one"), Line::from("two"), Line::from("one again")];
    let mut state = TranscriptSearchState::default();
    state.open(SearchDirection::Reverse);
    state.query = "one".to_string();
    state.recompute_matches(&lines, None);

    assert_eq!(state.current_index, Some(1));
    assert_eq!(state.active_line_index(), Some(2));

    state.confirm();
    assert!(!state.editing);
    assert!(state.is_visible());

    state.open(SearchDirection::Forward);
    assert!(state.editing);
    assert_eq!(state.direction, SearchDirection::Forward);
    assert!(state.query.is_empty());
    assert!(state.matches.is_empty());
    assert_eq!(state.current_index, None);
}

