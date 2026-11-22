use grein_tui::app::App;
use grein_tui::engine_port::TuiEngineEvent;
use grein_tui::markdown::{
    parse_inline_spans, render_markdown_lines, render_markdown_lines_with_width, wrap_styled_lines,
};
use grein_tui::render::{render_app, render_markdown_lines as render_mod_markdown_lines};
use grein_tui::runner::apply_engine_event;
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

    assert_eq!(texts[4], "╭─ markdown ─");
    assert_eq!(texts[5], "| a | b |");
    assert_eq!(texts[6], "| --- | --- |");
    assert_eq!(texts[7], "| 1 | 2 |");
    assert_eq!(texts[8], "# not a heading");
    assert_eq!(texts[9], "---");
    assert_eq!(texts[10], "╰───");

    assert!(lines[4]
        .spans
        .iter()
        .all(|s| s.style.fg == Some(Color::DarkGray)));
    for line in &lines[5..=9] {
        assert!(line
            .spans
            .iter()
            .all(|s| s.style.fg == Some(Color::Reset)));
    }
    assert!(lines[10]
        .spans
        .iter()
        .all(|s| s.style.fg == Some(Color::DarkGray)));
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

    assert_eq!(texts[3], "✦ reply");
    assert_eq!(texts.len(), 4);
    assert!(
        !texts.join("\n").contains("Agent (agy)"),
        "Agent (agy) literal header must not appear in rendered lines: {texts:?}"
    );
    assert_eq!(rendered[3].spans[0].content, "✦ ");
    assert_eq!(rendered[3].spans[0].style.fg, Some(Color::Cyan));
    assert!(rendered[3].spans[0]
        .style
        .add_modifier
        .contains(Modifier::BOLD));
    assert_eq!(rendered[3].spans[1].content, "reply");
    assert!(rendered[3]
        .spans
        .iter()
        .all(|s| s.style.fg != Some(Color::Green)));
}

#[test]
fn test_assistant_and_agent_headers_render_minimal_cyan_accent() {
    for header in ["## Assistant", "## Agent (agy)", "  ## Assistant  "] {
        let md = format!("{header}\nHello world");
        let lines = render_markdown_lines(&md);
        let texts = lines_text(&lines);
        assert_eq!(texts, vec!["✦ Hello world"], "for header {header:?}");
        assert!(!texts.join("\n").contains("Assistant"));
        assert!(!texts.join("\n").contains("Agent (agy)"));
        assert_eq!(lines[0].spans[0].content, "✦ ");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Cyan));
        assert!(lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(lines[0].spans[1].content, "Hello world");
        assert_eq!(lines[0].spans[1].style.fg, None);
    }

    let spaced = render_markdown_lines("## Assistant\n\n\nHello\n\nWorld");
    assert_eq!(lines_text(&spaced), vec!["✦ Hello", "", "World"]);

    for eof_only in ["## Assistant\n", "## Assistant", "## Agent (agy)\n\n"] {
        let lines = render_markdown_lines(eof_only);
        assert_eq!(lines_text(&lines), vec!["✦"], "for {eof_only:?}");
        assert_eq!(lines[0].spans[0].content, "✦");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Cyan));
        assert!(lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
    }

    let user_to_asst = render_markdown_lines("# User: hi\n## Assistant\nhello");
    assert_eq!(lines_text(&user_to_asst), vec!["❯ hi", "✦ hello"]);
    assert!(user_to_asst[0]
        .spans
        .iter()
        .all(|s| s.style.fg == Some(Color::Green)));
    assert_eq!(user_to_asst[1].spans[0].style.fg, Some(Color::Cyan));
    assert!(user_to_asst[1]
        .spans
        .iter()
        .all(|s| s.style.fg != Some(Color::Green)));

    let asst_to_user = render_markdown_lines("## Assistant\n# User: next");
    assert_eq!(lines_text(&asst_to_user), vec!["✦", "❯ next"]);
    assert_eq!(asst_to_user[0].spans[0].style.fg, Some(Color::Cyan));

    let back_to_back = render_markdown_lines("## Assistant\n## Agent (agy)\nhello");
    assert_eq!(lines_text(&back_to_back), vec!["✦", "✦ hello"]);

    let heading_first = render_markdown_lines("## Assistant\n### Summary");
    assert_eq!(lines_text(&heading_first), vec!["✦ Summary"]);
    assert_eq!(heading_first[0].spans[0].style.fg, Some(Color::Cyan));
    assert!(heading_first[0].spans[1]
        .style
        .add_modifier
        .contains(Modifier::BOLD));

    let list_first = render_markdown_lines("## Assistant\n- item one");
    assert_eq!(lines_text(&list_first), vec!["✦ • item one"]);

    let quote_first = render_markdown_lines("## Assistant\n> quoted line");
    assert_eq!(lines_text(&quote_first), vec!["✦ │ quoted line"]);

    let non_marker_h2 = render_markdown_lines("## Assistant Note\nbody");
    assert_eq!(lines_text(&non_marker_h2), vec!["Assistant Note", "body"]);
    let non_marker_h3 = render_markdown_lines("### Assistant\nbody");
    assert_eq!(lines_text(&non_marker_h3), vec!["Assistant", "body"]);
}

#[test]
fn test_assistant_accent_code_fence_table_and_literal_markdown_fence() {
    let md_fence = "## Assistant\n\n```rust\nlet x = 1;\n```";
    let fence_lines = render_markdown_lines(md_fence);
    let fence_texts = lines_text(&fence_lines);
    assert_eq!(
        fence_texts,
        vec!["✦", "╭─ rust ─", "let x = 1;", "╰───"]
    );
    assert_eq!(fence_lines[0].spans[0].style.fg, Some(Color::Cyan));
    assert!(fence_lines[0].spans[0]
        .style
        .add_modifier
        .contains(Modifier::BOLD));
    assert!(fence_texts[2].starts_with("let x = 1;"));

    let tilde_fence = "## Agent (agy)\n~~~python\nprint(1)\n~~~";
    assert_eq!(
        lines_text(&render_markdown_lines(tilde_fence)),
        vec!["✦", "╭─ python ─", "print(1)", "╰───"]
    );

    let table_first = "## Assistant\n| A | B |\n| --- | --- |\n| 1 | 2 |";
    let table_lines = render_markdown_lines(table_first);
    let table_texts = lines_text(&table_lines);
    assert_eq!(table_texts[0], "✦");
    assert!(table_texts[1].starts_with('┌'));

    let literal_in_fence = "```markdown\n## Assistant\n## Agent (agy)\n```";
    assert_eq!(
        lines_text(&render_markdown_lines(literal_in_fence)),
        vec!["╭─ markdown ─", "## Assistant", "## Agent (agy)", "╰───"]
    );
}

#[test]
fn test_assistant_accent_wrapping_and_ratatui_screen() {
    let md = "## Assistant\nalpha beta gamma";
    let wrapped_12 = wrap_styled_lines(render_markdown_lines_with_width(md, 12), 12);
    assert_eq!(lines_text(&wrapped_12), vec!["✦ alpha beta", "gamma"]);
    assert!(wrapped_12.iter().all(|l| l.width() <= 12));
    assert_eq!(wrapped_12[0].spans[0].style.fg, Some(Color::Cyan));
    assert!(wrapped_12[0].spans[0]
        .style
        .add_modifier
        .contains(Modifier::BOLD));

    let wrapped_11 = wrap_styled_lines(render_markdown_lines_with_width(md, 11), 11);
    assert_eq!(lines_text(&wrapped_11), vec!["✦ alpha", "beta gamma"]);
    assert!(wrapped_11.iter().all(|l| l.width() <= 11));

    let backend = TestBackend::new(80, 18);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = App::new();
    app.active_transcript_md =
        "# User: hi\n\n## Assistant\nHello world\n\n## Agent (agy)\n\n```rust\nlet x = 1;\n```"
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

    assert!(screen.contains("❯ hi"), "{screen}");
    assert!(screen.contains("✦ Hello world"), "{screen}");
    assert!(screen.contains("let x = 1;"), "{screen}");
    assert!(!screen.contains("## Assistant"), "{screen}");
    assert!(!screen.contains("Agent (agy)"), "{screen}");

    let mut found_cyan_bold_star = false;
    for y in 0..buf.area.height {
        let row = &screen_rows[y as usize];
        if let Some(byte_idx) = row.find("✦ Hello world") {
            let col = row[..byte_idx].chars().count() as u16;
            let cell = &buf[(col, y)];
            assert_eq!(cell.fg, Color::Cyan);
            assert!(cell.modifier.contains(Modifier::BOLD));
            found_cyan_bold_star = true;
            break;
        }
    }
    assert!(found_cyan_bold_star, "expected cyan bold ✦ on screen:\n{screen}");
}

#[test]
fn test_shell_header_lines_style_command_bold_and_exit_status_colored() {
    let md = "\
$ ls -latr (exit 0)
$ false (exit 2)
$ sleep 99 (exit -1)
$ echo \" (exit 1)\" (exit 0)";

    let lines = render_markdown_lines(md);
    assert_eq!(lines.len(), 4);
    assert_eq!(
        lines_text(&lines),
        vec![
            "$ ls -latr (exit 0)",
            "$ false (exit 2)",
            "$ sleep 99 (exit -1)",
            "$ echo \" (exit 1)\" (exit 0)",
        ]
    );

    assert_eq!(lines[0].spans[0].content, "$ ls -latr ");
    assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(lines[0].spans[1].content, "(exit 0)");
    assert_eq!(lines[0].spans[1].style.fg, Some(Color::Green));

    assert_eq!(lines[1].spans[0].content, "$ false ");
    assert!(lines[1].spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(lines[1].spans[1].content, "(exit 2)");
    assert_eq!(lines[1].spans[1].style.fg, Some(Color::Red));

    assert_eq!(lines[2].spans[0].content, "$ sleep 99 ");
    assert_eq!(lines[2].spans[1].content, "(exit -1)");
    assert_eq!(lines[2].spans[1].style.fg, Some(Color::Red));

    assert_eq!(lines[3].spans[0].content, "$ echo \" (exit 1)\" ");
    assert_eq!(lines[3].spans[1].content, "(exit 0)");
    assert_eq!(lines[3].spans[1].style.fg, Some(Color::Green));
}

#[test]
fn test_ansi_fenced_block_parses_sgr_styles_persists_across_lines_and_hides_fences() {
    let md = concat!(
        "$ ls -latr (exit 0)\n",
        "```ansi\n",
        "plain default\n",
        "\x1b[1;34mdir\x1b[0m \x1b[2mdim\x1b[22m \x1b[3mit\x1b[23m \x1b[4mul\x1b[24m \x1b[7mrev\x1b[27m\n",
        "\x1b[91mbright_red\x1b[39m \x1b[42mbg_green\x1b[49m \x1b[104mbright_bg\x1b[0m\n",
        "\x1b[38;5;208morange256\x1b[0m \x1b[48;5;17mbg256\x1b[0m \x1b[38;2;12;34;56;48;2;78;90;12mrgb\x1b[0m\n",
        "\x1b[32mpersist_line1\n",
        "persist_line2\x1b[m_after_empty_reset\n",
        "[stderr]\n",
        "\x1b[31merr msg\x1b[0m\n",
        "```\n",
        "```rust\n",
        "let x = 1;\n",
        "```"
    );

    let lines = render_markdown_lines(md);
    let texts = lines_text(&lines);
    assert!(
        !texts.iter().any(|t| t == "```ansi" || t == "```" && t.is_empty()),
        "ansi fences must be hidden: {texts:?}"
    );
    assert_eq!(texts[0], "$ ls -latr (exit 0)");
    assert_eq!(texts[1], "plain default");
    assert_eq!(texts[2], "dir dim it ul rev");
    assert_eq!(texts[3], "bright_red bg_green bright_bg");
    assert_eq!(texts[4], "orange256 bg256 rgb");
    assert_eq!(texts[5], "persist_line1");
    assert_eq!(texts[6], "persist_line2_after_empty_reset");
    assert_eq!(texts[7], "[stderr]");
    assert_eq!(texts[8], "err msg");
    assert_eq!(texts[9], "╭─ rust ─");
    assert_eq!(texts[10], "let x = 1;");
    assert_eq!(texts[11], "╰───");

    assert_eq!(lines[1].spans[0].style.fg, None);
    assert_eq!(lines[1].spans[0].style.bg, None);

    assert!(lines[2]
        .spans
        .iter()
        .any(|s| s.content == "dir"
            && s.style.fg == Some(Color::Blue)
            && s.style.add_modifier.contains(Modifier::BOLD)));
    assert!(lines[2]
        .spans
        .iter()
        .any(|s| s.content == "dim" && s.style.add_modifier.contains(Modifier::DIM)));
    assert!(lines[2].spans.iter().any(|s| {
        s.content == "it"
            && s.style.add_modifier.contains(Modifier::ITALIC)
            && !s.style.add_modifier.contains(Modifier::DIM)
    }));
    assert!(lines[2].spans.iter().any(|s| {
        s.content == "ul"
            && s.style.add_modifier.contains(Modifier::UNDERLINED)
            && !s.style.add_modifier.contains(Modifier::ITALIC)
    }));
    assert!(lines[2].spans.iter().any(|s| {
        s.content == "rev"
            && s.style.add_modifier.contains(Modifier::REVERSED)
            && !s.style.add_modifier.contains(Modifier::UNDERLINED)
    }));

    assert!(lines[3]
        .spans
        .iter()
        .any(|s| s.content == "bright_red" && s.style.fg == Some(Color::LightRed)));
    assert!(lines[3]
        .spans
        .iter()
        .any(|s| s.content == "bg_green" && s.style.fg.is_none() && s.style.bg == Some(Color::Green)));
    assert!(lines[3]
        .spans
        .iter()
        .any(|s| s.content == "bright_bg" && s.style.bg == Some(Color::LightBlue)));

    assert!(lines[4]
        .spans
        .iter()
        .any(|s| s.content == "orange256" && s.style.fg == Some(Color::Indexed(208))));
    assert!(lines[4]
        .spans
        .iter()
        .any(|s| s.content == "bg256" && s.style.bg == Some(Color::Indexed(17))));
    assert!(lines[4].spans.iter().any(|s| {
        s.content == "rgb"
            && s.style.fg == Some(Color::Rgb(12, 34, 56))
            && s.style.bg == Some(Color::Rgb(78, 90, 12))
    }));

    assert_eq!(lines[5].spans[0].style.fg, Some(Color::Green));
    assert!(lines[6]
        .spans
        .iter()
        .any(|s| s.content == "persist_line2" && s.style.fg == Some(Color::Green)));
    assert!(lines[6]
        .spans
        .iter()
        .any(|s| s.content == "_after_empty_reset" && s.style.fg.is_none()));

    assert_eq!(lines[7].spans[0].content, "[stderr]");
    assert_eq!(lines[7].spans[0].style.fg, Some(Color::Red));
    assert_eq!(lines[8].spans[0].style.fg, Some(Color::Red));

    assert_eq!(lines[9].spans[0].style.fg, Some(Color::DarkGray));
    assert_eq!(lines[10].spans[0].content, "let");
    assert_eq!(lines[10].spans[0].style.fg, Some(Color::Magenta));
    assert!(lines[10].spans[0]
        .style
        .add_modifier
        .contains(Modifier::BOLD));
    assert!(lines[10]
        .spans
        .iter()
        .any(|s| s.content == "1" && s.style.fg == Some(Color::LightYellow)));
    assert_eq!(lines[11].spans[0].style.fg, Some(Color::DarkGray));
}

#[test]
fn test_ansi_strips_non_sgr_osc_cr_and_survives_malformed_sequences() {
    let md = concat!(
        "```ansi\n",
        "\x1b[01;31m\x1b[Khit\x1b[m\x1b[K\r\n",
        "\x1b[2J\x1b[?25h\x1b]8;;https://example.com\x07link\x1b]8;;\x07 \x1b]0;title\x1b\\ok\n",
        "trailing_esc\x1b\n",
        "trunc_csi\x1b[38;2;10;20\n",
        "utf8_csi\x1b[31🦀safe\n",
        "trunc_osc\x1b]0;🦀title\n",
        "over_256\x1b[38;5;999;32mok\x1b[0m\n",
        "```"
    );

    let lines = render_markdown_lines(md);
    let texts = lines_text(&lines);
    assert_eq!(
        texts,
        vec![
            "hit",
            "link ok",
            "trailing_esc",
            "trunc_csi",
            "utf8_csi🦀safe",
            "trunc_osc",
            "over_256ok",
        ]
    );
    assert!(lines[0]
        .spans
        .iter()
        .any(|s| s.content == "hit" && s.style.fg == Some(Color::Red)));
    assert!(lines[6]
        .spans
        .iter()
        .any(|s| s.content == "ok" && s.style.fg == Some(Color::Green)));
}

#[test]
fn test_ratatui_test_backend_shell_ansi_colors_and_runner_dedup() {
    let mut app = App::new();
    app.submit_composer_prompt("!ls -latr");
    assert!(app.active_transcript_md.contains("> $ ls -latr"));

    let tid = app.active_thread_id.clone();
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ShellOutput {
            thread_id: tid.clone(),
            command: "ls -latr".to_string(),
            stdout: "\x1b[01;34msrc\x1b[0m \x1b[38;5;208mCargo.toml\x1b[0m \x1b[38;2;10;200;30mrgb\x1b[0m plain\n".to_string(),
            stderr: "warn\n".to_string(),
            exit_code: 0,
        },
    );

    assert!(
        !app.active_transcript_md.contains("> $ ls -latr"),
        "pending stub must be deduplicated: {:?}",
        app.active_transcript_md
    );
    assert_eq!(app.active_transcript_md.matches("$ ls -latr").count(), 1);
    assert!(app
        .active_transcript_md
        .contains("$ ls -latr (exit 0)\n```ansi\n"));

    app.submit_composer_prompt("!false");
    let tid2 = app.active_thread_id.clone();
    apply_engine_event(
        &mut app,
        TuiEngineEvent::ShellOutput {
            thread_id: tid2,
            command: "false".to_string(),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 2,
        },
    );
    assert!(!app.active_transcript_md.contains("> $ false"));
    assert!(app.active_transcript_md.contains("$ false (exit 2)\n\n"));
    assert!(!app
        .active_transcript_md
        .contains("$ false (exit 2)\n```ansi"));

    let backend = TestBackend::new(110, 24);
    let mut terminal = Terminal::new(backend).unwrap();
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

    assert!(screen.contains("$ ls -latr (exit 0)"), "{screen}");
    assert!(screen.contains("$ false (exit 2)"), "{screen}");
    assert!(!screen.contains("```ansi"), "{screen}");
    assert!(!screen.contains("> $ ls -latr"), "{screen}");

    let find_cell = |needle: &str| -> (Color, Modifier) {
        for y in 0..buf.area.height {
            let row = &screen_rows[y as usize];
            if let Some(byte_idx) = row.find(needle) {
                let col = row[..byte_idx].chars().count() as u16;
                let cell = &buf[(col, y)];
                return (cell.fg, cell.modifier);
            }
        }
        panic!("needle {needle:?} not found on screen:\n{screen}");
    };

    let (exit0_fg, _) = find_cell("(exit 0)");
    assert_eq!(exit0_fg, Color::Green);
    let (exit2_fg, _) = find_cell("(exit 2)");
    assert_eq!(exit2_fg, Color::Red);
    let (dir_fg, dir_mod) = find_cell("src");
    assert_eq!(dir_fg, Color::Blue);
    assert!(dir_mod.contains(Modifier::BOLD));
    let (idx_fg, _) = find_cell("Cargo.toml");
    assert_eq!(idx_fg, Color::Indexed(208));
    let (rgb_fg, _) = find_cell("rgb");
    assert_eq!(rgb_fg, Color::Rgb(10, 200, 30));
    let (plain_fg, _) = find_cell("plain");
    assert_eq!(plain_fg, Color::Reset);
    let (stderr_fg, _) = find_cell("[stderr]");
    assert_eq!(stderr_fg, Color::Red);
}
