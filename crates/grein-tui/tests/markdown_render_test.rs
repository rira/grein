use grein_tui::app::App;
use grein_tui::markdown::{
    parse_inline_spans, render_markdown_lines, render_markdown_lines_with_width,
};
use grein_tui::render::{render_app, render_markdown_lines as render_mod_markdown_lines};
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use ratatui::text::Line;
use ratatui::Terminal;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>()
}

fn lines_text(lines: &[Line<'_>]) -> Vec<String> {
    lines.iter().map(line_text).collect()
}

#[test]
fn test_gfm_table_box_borders_and_alignments() {
    let md = "\
| Name | Align | Score |
| :--- | :---: | ---: |
| alpha | mid | 7 |
| beta | center | 100 |";

    let lines = render_markdown_lines(md);
    let rendered = lines_text(&lines);

    assert_eq!(
        rendered.len(),
        6,
        "compact 1-line-per-row table should have top, header, sep, 2 body rows, bottom; got:\n{}",
        rendered.join("\n")
    );

    assert!(rendered[0].starts_with('┌') && rendered[0].contains('┬') && rendered[0].ends_with('┐'));
    assert!(rendered[2].starts_with('├') && rendered[2].contains('┼') && rendered[2].ends_with('┤'));
    assert!(rendered[5].starts_with('└') && rendered[5].contains('┴') && rendered[5].ends_with('┘'));

    for (idx, line) in lines.iter().enumerate() {
        if idx == 0 || idx == 2 || idx == 5 {
            assert!(
                line.spans
                    .iter()
                    .all(|s| s.style.fg == Some(Color::DarkGray)),
                "table border line {idx} should use DarkGray border style"
            );
        }
    }

    let header_row = &lines[1];
    assert!(
        header_row
            .spans
            .iter()
            .filter(|s| s.content.contains("Name")
                || s.content.contains("Align")
                || s.content.contains("Score"))
            .all(|s| s.style.add_modifier.contains(Modifier::BOLD)),
        "header labels must be bold"
    );

    assert!(rendered[3].contains("│ alpha │"));
    assert!(rendered[3].contains("│  mid   │"));
    assert!(rendered[3].contains("│     7 │"));
    assert!(rendered[4].contains("│ beta  │"));
    assert!(rendered[4].contains("│ center │"));
    assert!(rendered[4].contains("│   100 │"));
}

#[test]
fn test_gfm_table_narrow_width_wrapping_50_and_72() {
    let md = "\
| Feature | **Pins** Approach | `shared_context` Behavior |
| --- | --- | --- |
| Scope | Explicit per-turn **pinning** of key artifacts | Implicit `shared_context` propagation across all nodes |
| Isolation | High isolation per branch | Lower isolation across parallel agents |";

    for max_width in [50usize, 72usize] {
        let lines = render_markdown_lines_with_width(md, max_width);
        let rendered = lines_text(&lines);

        for (i, line) in lines.iter().enumerate() {
            assert!(
                line.width() <= max_width,
                "line {i} width {} exceeds max_width {max_width}: {:?}",
                line.width(),
                rendered[i]
            );
            let txt = &rendered[i];
            let first = txt.chars().next().unwrap();
            let last = txt.chars().last().unwrap();
            assert!(
                matches!(first, '┌' | '│' | '├' | '└'),
                "line {i} unexpected start char in {txt:?}"
            );
            assert!(
                matches!(last, '┐' | '│' | '┤' | '┘'),
                "line {i} unexpected end char in {txt:?}"
            );
        }

        let divider_count = rendered
            .iter()
            .filter(|r| r.starts_with('├') && r.contains('┼') && r.ends_with('┤'))
            .count();
        assert!(
            divider_count >= 2,
            "wrapped table at width {max_width} should emit header sep + body row dividers; got:\n{}",
            rendered.join("\n")
        );

        let full_text = rendered.join("\n");
        assert!(!full_text.contains("**"));
        assert!(!full_text.contains('`'));
        assert!(full_text.contains("shared_context"));
        assert!(full_text.contains("Explicit"));
        assert!(full_text.contains("propagation"));
        assert!(full_text.contains("Isolation"));

        let bold_pin_present = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.content.contains("pinning") && s.style.add_modifier.contains(Modifier::BOLD))
        });
        assert!(bold_pin_present, "inline bold inside wrapped cell must remain bold");

        let yellow_code_present = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.content.contains("shared_context") && s.style.fg == Some(Color::Yellow))
        });
        assert!(
            yellow_code_present,
            "inline code inside wrapped cell must remain yellow"
        );
    }
}

#[test]
fn test_gfm_table_overlong_token_hard_wrap_and_tiny_widths() {
    let md = "\
| Key | Endpoint |
| --- | --- |
| api | https://example.com/very_long_endpoint_path_with_utf8_ééé_end |";

    let lines = render_markdown_lines_with_width(md, 36);
    for (i, line) in lines.iter().enumerate() {
        assert!(
            line.width() <= 36,
            "hard-wrapped line {i} width {} > 36: {:?}",
            line.width(),
            line_text(line)
        );
    }
    let joined: String = lines_text(&lines)
        .into_iter()
        .filter(|l| l.starts_with('│'))
        .map(|l| l.replace(['│', ' '], ""))
        .collect();
    assert!(joined.contains("https://example.com/very_long_endpoint_path_with_utf8_ééé_end"));

    for tiny in [0usize, 1, 2, 5, 10] {
        let out = render_markdown_lines_with_width(md, tiny);
        assert!(!out.is_empty(), "width {tiny} should render without panic");
    }
}

#[test]
fn test_gfm_table_escaped_pipes_mismatched_columns_and_streaming_partial() {
    let md_escaped = "\
| Expr | Meaning |
| --- | --- |
| `a \\| b` | bitwise \\| or |";

    let lines = render_markdown_lines(md_escaped);
    let rendered = lines_text(&lines);
    assert_eq!(rendered.len(), 5);
    assert!(rendered[3].contains("a | b"));
    assert!(rendered[3].contains("bitwise | or"));

    let md_mismatch = "\
| Col1 | Col2 |
| --- | --- |
| only_one |
| one | two | three |";
    let mismatch_lines = render_markdown_lines(md_mismatch);
    let mismatch_text = lines_text(&mismatch_lines);
    assert_eq!(mismatch_text.len(), 6);
    assert!(mismatch_text[3].contains("only_one"));
    assert!(mismatch_text[4].contains("three"));

    for partial in [
        "| Header1 | Header2 |\n| --- |",
        "| Header1 | Header2 |\n| --- | --- |",
        "| Header1 | Header2 |\n| --- | --- |\n| partial | cell",
        "| Header1 | Header2 |\n| --- | --- |\n|",
    ] {
        let out = render_markdown_lines_with_width(partial, 60);
        assert!(!out.is_empty());
        assert!(lines_text(&out)[0].starts_with('┌'));
    }
}

#[test]
fn test_headings_strip_prefix_and_parse_inline_bold_without_literal_stars() {
    let md = "\
# Comparing **Pins** vs. **Shared Context**
## Using `invoke_subagent` with *caution*
### Deep **Heading**
#### Level 4 **Title**";

    let lines = render_markdown_lines(md);
    assert_eq!(lines.len(), 4);

    let texts = lines_text(&lines);
    assert_eq!(texts[0], "Comparing Pins vs. Shared Context");
    assert_eq!(texts[1], "Using invoke_subagent with caution");
    assert_eq!(texts[2], "Deep Heading");
    assert_eq!(texts[3], "Level 4 Title");

    for line in &lines {
        for span in &line.spans {
            assert!(
                span.style.add_modifier.contains(Modifier::BOLD),
                "every heading span must be bold: {:?}",
                span
            );
            assert!(!span.content.contains("**"));
        }
    }

    assert!(lines[1]
        .spans
        .iter()
        .any(|s| s.content == "invoke_subagent" && s.style.fg == Some(Color::Yellow)));
    assert!(lines[1].spans.iter().any(|s| {
        s.content == "caution"
            && s.style.add_modifier.contains(Modifier::BOLD)
            && s.style.add_modifier.contains(Modifier::ITALIC)
    }));
}

#[test]
fn test_snake_case_identifiers_and_inline_styles() {
    let spans = parse_inline_spans(
        "Call invoke_subagent, alt_digits, __init__, foo_bar_baz, and 2 * 3 * 4 with _italic_, *star_em*, **bold**, and `code_**raw**_snake`.",
    );

    let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(
        joined,
        "Call invoke_subagent, alt_digits, __init__, foo_bar_baz, and 2 * 3 * 4 with italic, star_em, bold, and code_**raw**_snake."
    );

    for ident in [
        "invoke_subagent",
        "alt_digits",
        "__init__",
        "foo_bar_baz",
        "2 * 3 * 4",
    ] {
        assert!(
            spans
                .iter()
                .any(|s| s.content.contains(ident) && !s.style.add_modifier.contains(Modifier::ITALIC)),
            "identifier or math '{ident}' must not be italicized: {spans:?}"
        );
    }

    assert!(spans
        .iter()
        .any(|s| s.content == "italic" && s.style.add_modifier.contains(Modifier::ITALIC)));
    assert!(spans
        .iter()
        .any(|s| s.content == "star_em" && s.style.add_modifier.contains(Modifier::ITALIC)));
    assert!(spans
        .iter()
        .any(|s| s.content == "bold" && s.style.add_modifier.contains(Modifier::BOLD)));
    assert!(spans
        .iter()
        .any(|s| s.content == "code_**raw**_snake" && s.style.fg == Some(Color::Yellow)));
}

#[test]
fn test_horizontal_rules_and_not_confused_with_table_delimiter() {
    let md = "\
---
***
___
- - -";

    let constrained_50 = render_markdown_lines_with_width(md, 50);
    assert_eq!(constrained_50.len(), 4);
    for line in &constrained_50 {
        let text = line_text(line);
        assert_eq!(text.chars().count(), 50);
        assert!(text.chars().all(|c| c == '─'));
        assert!(line
            .spans
            .iter()
            .all(|s| s.style.fg == Some(Color::DarkGray)));
    }

    let constrained_100 = render_markdown_lines_with_width("---", 100);
    assert_eq!(line_text(&constrained_100[0]).chars().count(), 60);

    let constrained_2 = render_markdown_lines_with_width("---", 2);
    assert_eq!(line_text(&constrained_2[0]).chars().count(), 3);

    let unconstrained = render_markdown_lines("---");
    let count = line_text(&unconstrained[0]).chars().count();
    assert!((24..=40).contains(&count), "got {count}");

    let table_md = "\
| A | B |
| --- | --- |
| 1 | 2 |";
    let table_lines = render_markdown_lines_with_width(table_md, 40);
    assert_eq!(table_lines.len(), 5);
    assert!(!lines_text(&table_lines)
        .iter()
        .any(|l| l.chars().all(|c| c == '─')));
}

#[test]
fn test_nested_lists_blockquotes_and_fenced_code_blocks_with_pipes() {
    let md = "\
- parent **item**
  - child `code_id`
    * grandchild _em_
> Quoted **text** and `inline_fn`
```markdown
| a | b |
| --- | --- |
| 1 | 2 |
# not a heading
---
```";

    let lines = render_markdown_lines(md);
    let texts = lines_text(&lines);

    assert_eq!(texts[0], "• parent item");
    assert_eq!(texts[1], "  • child code_id");
    assert_eq!(texts[2], "    • grandchild em");

    assert!(texts[3].contains("Quoted text and inline_fn"));
    assert!(lines[3]
        .spans
        .iter()
        .any(|s| s.content.contains("Quoted ") && s.style.fg == Some(Color::DarkGray)));
    assert!(lines[3]
        .spans
        .iter()
        .any(|s| s.content == "text" && s.style.add_modifier.contains(Modifier::BOLD)));
    assert!(lines[3]
        .spans
        .iter()
        .any(|s| s.content == "inline_fn" && s.style.fg == Some(Color::Yellow)));

    assert_eq!(texts[4], "```markdown");
    assert_eq!(texts[5], "| a | b |");
    assert_eq!(texts[6], "| --- | --- |");
    assert_eq!(texts[7], "| 1 | 2 |");
    assert_eq!(texts[8], "# not a heading");
    assert_eq!(texts[9], "---");
    assert_eq!(texts[10], "```");

    assert!(lines[4]
        .spans
        .iter()
        .all(|s| s.style.fg == Some(Color::DarkGray)));
    for line in &lines[5..=9] {
        assert!(line
            .spans
            .iter()
            .all(|s| s.style.fg == Some(Color::Yellow)));
    }
    assert!(!texts.join("\n").contains('┌'));

    let reexport = render_mod_markdown_lines(md);
    assert_eq!(lines_text(&reexport), texts);
}

#[test]
fn test_ratatui_test_backend_transcript_renders_aligned_box_table_without_pipe_wrapping() {
    let backend = TestBackend::new(110, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = App::new();
    app.active_transcript_md = "\
## Comparing **Pins** vs. **Shared Context**

| Dimension | **Pins** | `shared_context` |
| :--- | :--- | :--- |
| Scope | Explicit artifact pinning for focused turn | Shared state visible across all parallel steps |
| Risk | Minimal drift risk | Higher cross-talk risk |

---

- Verified `invoke_subagent` path
  - Nested check passed"
        .to_string();

    terminal.draw(|f| render_app(f, &app)).unwrap();

    let buf = terminal.backend().buffer();
    let mut screen_rows = Vec::new();
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        screen_rows.push(row);
    }
    let screen = screen_rows.join("\n");

    assert!(
        screen.contains("Comparing Pins vs. Shared Context"),
        "heading should render without **:\n{screen}"
    );
    assert!(!screen.contains("**Pins**"));
    assert!(screen.contains("┌") && screen.contains("┐"));
    assert!(screen.contains("└") && screen.contains("┘"));
    assert!(screen.contains("invoke_subagent"));
    assert!(screen.contains("  • Nested check passed"));

    for row in &screen_rows {
        if row.contains('┌') {
            assert!(row.contains('┐'), "top border wrapped on screen row: {row}");
        }
        if row.contains('└') {
            assert!(row.contains('┘'), "bottom border wrapped on screen row: {row}");
        }
    }
}

#[test]
fn test_user_prompts_render_with_chevron_in_leaf_green() {
    let md = "# User: hello world\nsecond line\n\n## Agent (agy)\nreply";
    let rendered = render_markdown_lines(md);
    let texts = lines_text(&rendered);

    assert_eq!(texts[0], "❯ hello world");
    assert_eq!(texts[1], "  second line");
    assert!(
        !texts.join("\n").contains("User:"),
        "User: literal prefix must not appear in rendered lines: {texts:?}"
    );

    assert_eq!(rendered[0].spans[0].style.fg, Some(Color::Green));
    assert!(rendered[0].spans[0]
        .style
        .add_modifier
        .contains(Modifier::BOLD));
    assert!(rendered[0]
        .spans
        .iter()
        .all(|s| s.style.fg == Some(Color::Green)));
    assert!(rendered[1]
        .spans
        .iter()
        .all(|s| s.style.fg == Some(Color::Green)));

    assert_eq!(texts[3], "Agent (agy)");
    assert!(rendered[3]
        .spans
        .iter()
        .all(|s| s.style.fg != Some(Color::Green)));
    assert_eq!(texts[4], "reply");
    assert!(rendered[4]
        .spans
        .iter()
        .all(|s| s.style.fg != Some(Color::Green)));
}
