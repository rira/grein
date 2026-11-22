use grein_tui::markdown::{
    parse_inline_spans, render_ansi_text, render_markdown_lines, wrap_styled_line,
    wrap_styled_lines,
};
use grein_tui::syntax::{highlight_code_line, highlight_code_lines, safe_fence_for};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>()
}

fn lines_text(lines: &[Line<'_>]) -> Vec<String> {
    lines.iter().map(line_text).collect()
}

fn find_span<'a>(line: &'a Line<'a>, needle: &str) -> &'a Span<'a> {
    line.spans
        .iter()
        .find(|s| s.content.contains(needle))
        .unwrap_or_else(|| panic!("expected span containing {needle:?} in {:?}", line.spans))
}

#[test]
fn test_safe_fence_for_backtick_runs() {
    assert_eq!(safe_fence_for(""), "```");
    assert_eq!(safe_fence_for("no backticks here"), "```");
    assert_eq!(safe_fence_for("single ` and double `` ticks"), "```");
    assert_eq!(safe_fence_for("embedded ```rust\ncode\n```"), "````");
    assert_eq!(safe_fence_for("five ````` ticks inside"), "``````");
}

#[test]
fn test_syntax_highlight_rust_tokens_and_multiline_block_comments() {
    let mut in_comment = false;

    let attr_line = highlight_code_line("  RS  ", "#[derive(Debug)]", &mut in_comment);
    assert!(!in_comment);
    assert_eq!(line_text(&attr_line), "#[derive(Debug)]");
    assert_eq!(
        find_span(&attr_line, "#[derive(Debug)]").style.fg,
        Some(Color::LightBlue)
    );

    let sig_line = highlight_code_line(
        "rust",
        "pub fn main() -> Result<usize, String> {",
        &mut in_comment,
    );
    let pub_span = find_span(&sig_line, "pub");
    assert_eq!(pub_span.style.fg, Some(Color::Magenta));
    assert!(pub_span.style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(find_span(&sig_line, "Result").style.fg, Some(Color::Cyan));
    assert_eq!(find_span(&sig_line, "usize").style.fg, Some(Color::Cyan));
    assert_eq!(find_span(&sig_line, "String").style.fg, Some(Color::Cyan));

    let body_line = highlight_code_line(
        "rust",
        "println!(\"hi\", 42); let r = 0..10; // tail",
        &mut in_comment,
    );
    assert_eq!(
        find_span(&body_line, "println").style.fg,
        Some(Color::LightBlue)
    );
    assert_eq!(
        find_span(&body_line, "\"hi\"").style.fg,
        Some(Color::Green)
    );
    assert_eq!(
        find_span(&body_line, "42").style.fg,
        Some(Color::LightYellow)
    );
    assert_eq!(
        find_span(&body_line, "0").style.fg,
        Some(Color::LightYellow)
    );
    assert_eq!(
        find_span(&body_line, "10").style.fg,
        Some(Color::LightYellow)
    );
    let comment_span = find_span(&body_line, "// tail");
    assert_eq!(comment_span.style.fg, Some(Color::DarkGray));
    assert!(comment_span.style.add_modifier.contains(Modifier::ITALIC));

    let lifetime_line = highlight_code_line(
        "rust",
        "fn borrow<'a, 'b>(x: &'a str, c: 'z') {}",
        &mut in_comment,
    );
    assert_ne!(find_span(&lifetime_line, "'a").style.fg, Some(Color::Green));
    assert_eq!(
        find_span(&lifetime_line, "'z'").style.fg,
        Some(Color::Green)
    );

    let block = "/* open\nstill comment\nclose */ let x = 7;\nlet /* inline */ y = 1;";
    let lines = highlight_code_lines("rust", block);
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0].spans[0].style.fg, Some(Color::DarkGray));
    assert!(lines[0].spans[0]
        .style
        .add_modifier
        .contains(Modifier::ITALIC));
    assert_eq!(lines[1].spans[0].style.fg, Some(Color::DarkGray));
    assert!(lines[1].spans[0]
        .style
        .add_modifier
        .contains(Modifier::ITALIC));
    assert_eq!(find_span(&lines[2], "close */").style.fg, Some(Color::DarkGray));
    assert_eq!(find_span(&lines[2], "let").style.fg, Some(Color::Magenta));
    assert_eq!(find_span(&lines[2], "7").style.fg, Some(Color::LightYellow));
    assert_eq!(
        find_span(&lines[3], "/* inline */").style.fg,
        Some(Color::DarkGray)
    );
    assert_eq!(find_span(&lines[3], "1").style.fg, Some(Color::LightYellow));
}

#[test]
fn test_syntax_highlight_all_languages_and_neutral_plain() {
    let ts_lines = highlight_code_lines(
        "tsx",
        "@Injectable()\nexport async function get(id: string): Promise<number> { return 1; }",
    );
    assert_eq!(
        find_span(&ts_lines[0], "@Injectable").style.fg,
        Some(Color::LightBlue)
    );
    assert_eq!(
        find_span(&ts_lines[1], "export").style.fg,
        Some(Color::Magenta)
    );
    assert_eq!(
        find_span(&ts_lines[1], "string").style.fg,
        Some(Color::Cyan)
    );
    assert_eq!(
        find_span(&ts_lines[1], "1").style.fg,
        Some(Color::LightYellow)
    );

    let py_lines = highlight_code_lines(
        "python",
        "@dataclass\ndef greet(name: str) -> bool: # hi\n    return True",
    );
    assert_eq!(
        find_span(&py_lines[0], "@dataclass").style.fg,
        Some(Color::LightBlue)
    );
    assert_eq!(find_span(&py_lines[1], "def").style.fg, Some(Color::Magenta));
    assert_eq!(find_span(&py_lines[1], "str").style.fg, Some(Color::Cyan));
    assert_eq!(
        find_span(&py_lines[1], "# hi").style.fg,
        Some(Color::DarkGray)
    );
    assert_eq!(
        find_span(&py_lines[2], "True").style.fg,
        Some(Color::Magenta)
    );

    let sh_lines = highlight_code_lines(
        "bash",
        "if [ -n \"$HOME\" ]; then export N=3; fi # done",
    );
    assert_eq!(find_span(&sh_lines[0], "if").style.fg, Some(Color::Magenta));
    assert_eq!(
        find_span(&sh_lines[0], "\"$HOME\"").style.fg,
        Some(Color::Green)
    );
    assert_eq!(
        find_span(&sh_lines[0], "# done").style.fg,
        Some(Color::DarkGray)
    );

    let json_lines = highlight_code_lines(
        "jsonc",
        "{\n  \"name\": \"grein\",\n  \"count\": 10,\n  \"ok\": true\n}",
    );
    assert_eq!(
        find_span(&json_lines[1], "\"name\"").style.fg,
        Some(Color::LightBlue)
    );
    assert_eq!(
        find_span(&json_lines[1], "\"grein\"").style.fg,
        Some(Color::Green)
    );
    assert_eq!(
        find_span(&json_lines[2], "10").style.fg,
        Some(Color::LightYellow)
    );
    assert_eq!(
        find_span(&json_lines[3], "true").style.fg,
        Some(Color::Magenta)
    );

    let toml_lines = highlight_code_lines(
        "toml",
        "[package]\nname = \"grein\"\nport = 8080 # dev",
    );
    assert_eq!(
        find_span(&toml_lines[0], "[package]").style.fg,
        Some(Color::LightBlue)
    );
    assert_eq!(
        find_span(&toml_lines[1], "name").style.fg,
        Some(Color::LightBlue)
    );
    assert_eq!(
        find_span(&toml_lines[1], "\"grein\"").style.fg,
        Some(Color::Green)
    );
    assert_eq!(
        find_span(&toml_lines[2], "8080").style.fg,
        Some(Color::LightYellow)
    );
    assert_eq!(
        find_span(&toml_lines[2], "# dev").style.fg,
        Some(Color::DarkGray)
    );

    let yaml_lines = highlight_code_lines("yml", "service: \"api\"\nenabled: true\nport: 9000");
    assert_eq!(
        find_span(&yaml_lines[0], "service").style.fg,
        Some(Color::LightBlue)
    );
    assert_eq!(
        find_span(&yaml_lines[0], "\"api\"").style.fg,
        Some(Color::Green)
    );
    assert_eq!(
        find_span(&yaml_lines[1], "true").style.fg,
        Some(Color::Magenta)
    );
    assert_eq!(
        find_span(&yaml_lines[2], "9000").style.fg,
        Some(Color::LightYellow)
    );

    let nix_lines = highlight_code_lines(
        "nix",
        "let x = \"ok\"; in { inherit x; } # flake",
    );
    assert_eq!(
        find_span(&nix_lines[0], "let").style.fg,
        Some(Color::Magenta)
    );
    assert_eq!(
        find_span(&nix_lines[0], "\"ok\"").style.fg,
        Some(Color::Green)
    );
    assert_eq!(
        find_span(&nix_lines[0], "# flake").style.fg,
        Some(Color::DarkGray)
    );

    let sql_lines = highlight_code_lines(
        "sql",
        "select id from users where age > 18 -- adult",
    );
    assert_eq!(
        find_span(&sql_lines[0], "select").style.fg,
        Some(Color::Magenta)
    );
    assert_eq!(
        find_span(&sql_lines[0], "18").style.fg,
        Some(Color::LightYellow)
    );
    assert_eq!(
        find_span(&sql_lines[0], "-- adult").style.fg,
        Some(Color::DarkGray)
    );

    let go_lines = highlight_code_lines("go", "func main() int { return 0 } // go");
    assert_eq!(
        find_span(&go_lines[0], "func").style.fg,
        Some(Color::Magenta)
    );
    assert_eq!(find_span(&go_lines[0], "int").style.fg, Some(Color::Cyan));
    assert_eq!(
        find_span(&go_lines[0], "0").style.fg,
        Some(Color::LightYellow)
    );

    let cpp_lines = highlight_code_lines("cpp", "#include <vector>\nint main() { return 0; }");
    assert_eq!(
        find_span(&cpp_lines[0], "#include").style.fg,
        Some(Color::LightBlue)
    );
    assert_eq!(find_span(&cpp_lines[1], "int").style.fg, Some(Color::Cyan));
    assert_eq!(
        find_span(&cpp_lines[1], "return").style.fg,
        Some(Color::Magenta)
    );

    let diff_lines = highlight_code_lines(
        "diff",
        "diff --git a/a.rs b/a.rs\nindex 111..222\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n-old\n+new\n ctx",
    );
    for line in diff_lines.iter().take(4) {
        assert_eq!(line.spans[0].style.fg, Some(Color::Yellow));
        assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
    }
    assert_eq!(diff_lines[4].spans[0].style.fg, Some(Color::Cyan));
    assert!(diff_lines[4].spans[0]
        .style
        .add_modifier
        .contains(Modifier::BOLD));
    assert_eq!(diff_lines[5].spans[0].style.fg, Some(Color::Red));
    assert_eq!(diff_lines[6].spans[0].style.fg, Some(Color::Green));
    assert_eq!(diff_lines[7].spans[0].style.fg, Some(Color::Reset));

    for lang in ["text", "", "unknown_lang"] {
        let plain = highlight_code_lines(lang, "just plain text 123");
        assert_eq!(plain.len(), 1);
        assert!(plain[0]
            .spans
            .iter()
            .all(|s| s.style.fg == Some(Color::Reset)));
    }
}

#[test]
fn test_commonmark_fences_backticks_tildes_embedded_and_headers() {
    let md = "\
````markdown
```rust
let inside = 1;
```
````

~~~python
```
def hi():
    return 42
~~~

```
plain block
```

Inline ```ticks``` not fence.";

    let lines = render_markdown_lines(md);
    let texts = lines_text(&lines);

    assert_eq!(texts[0], "╭─ markdown ─");
    assert_eq!(lines[0].spans[0].style.fg, Some(Color::DarkGray));
    assert_eq!(texts[1], "```rust");
    assert_eq!(texts[2], "let inside = 1;");
    assert_eq!(texts[3], "```");
    assert_eq!(texts[4], "╰───");
    assert_eq!(lines[4].spans[0].style.fg, Some(Color::DarkGray));

    assert_eq!(texts[6], "╭─ python ─");
    assert_eq!(texts[7], "```");
    assert_eq!(texts[8], "def hi():");
    assert_eq!(find_span(&lines[8], "def").style.fg, Some(Color::Magenta));
    assert_eq!(texts[9], "    return 42");
    assert_eq!(texts[10], "╰───");

    assert_eq!(texts[12], "╭───");
    assert_eq!(lines[12].spans[0].style.fg, Some(Color::DarkGray));
    assert_eq!(texts[13], "plain block");
    assert_eq!(lines[13].spans[0].style.fg, Some(Color::Reset));
    assert_eq!(texts[14], "╰───");

    let last = lines.last().unwrap();
    assert_eq!(line_text(last), "Inline ticks not fence.");
    assert!(last
        .spans
        .iter()
        .any(|s| s.content == "ticks" && s.style.fg == Some(Color::Yellow)));
}

#[test]
fn test_render_ansi_text_persists_style_across_lines() {
    let ansi = "\x1b[32mgreen_1\ngreen_2\x1b[0m\n[stderr]\n\x1b[31merr\x1b[0m";
    let lines = render_ansi_text(ansi);
    assert_eq!(lines_text(&lines), vec!["green_1", "green_2", "[stderr]", "err"]);
    assert_eq!(lines[0].spans[0].style.fg, Some(Color::Green));
    assert_eq!(lines[1].spans[0].style.fg, Some(Color::Green));
    assert_eq!(lines[2].spans[0].style.fg, Some(Color::Red));
    assert_eq!(lines[3].spans[0].style.fg, Some(Color::Red));
}

#[test]
fn test_multi_backtick_inline_code_spans() {
    let spans = parse_inline_spans("Use `` `foo` `` and **bold `` `bar` `` end**.");
    let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(joined, "Use `foo` and bold `bar` end.");

    assert!(spans
        .iter()
        .any(|s| s.content == "`foo`" && s.style.fg == Some(Color::Yellow)));
    assert!(spans.iter().any(|s| {
        s.content == "`bar`"
            && s.style.fg == Some(Color::Yellow)
            && s.style.add_modifier.contains(Modifier::BOLD)
    }));
}

#[test]
fn test_wrap_styled_line_and_lines_unicode_and_styles() {
    let s1 = Style::default().fg(Color::Cyan);
    let s2 = Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::BOLD);

    let line = Line::from(vec![
        Span::styled("hello ", s1),
        Span::styled("world_token_long", s2),
    ]);
    let wrapped = wrap_styled_line(line, 10);
    for l in &wrapped {
        assert!(l.width() <= 10, "line exceeded width 10: {:?}", l);
    }
    let joined_no_ws: String = lines_text(&wrapped)
        .concat()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert_eq!(joined_no_ws, "helloworld_token_long");
    assert!(wrapped
        .iter()
        .any(|l| l.spans.iter().any(|s| s.style == s1 && s.content.contains("hello"))));
    assert!(wrapped
        .iter()
        .any(|l| l.spans.iter().any(|s| s.style == s2 && s.content.contains("world"))));

    let wide = Line::from(Span::styled("🦀🦀🦀🦀 日本語テスト", s1));
    let wrapped_wide = wrap_styled_line(wide, 5);
    for l in &wrapped_wide {
        assert!(l.width() <= 5, "wide line width {} > 5: {:?}", l.width(), l);
    }
    let wide_rejoined: String = lines_text(&wrapped_wide)
        .concat()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert_eq!(wide_rejoined, "🦀🦀🦀🦀日本語テスト");

    let indented = Line::from(Span::raw("    alpha beta"));
    let wrapped_indent = wrap_styled_line(indented, 10);
    assert_eq!(lines_text(&wrapped_indent), vec!["    alpha", "beta"]);

    let empty = wrap_styled_line(Line::raw(""), 10);
    assert_eq!(empty.len(), 1);
    assert_eq!(line_text(&empty[0]), "");

    let batch = wrap_styled_lines(
        vec![Line::raw("one two three"), Line::raw("four five")],
        7,
    );
    assert!(batch.iter().all(|l| l.width() <= 7));
    assert_eq!(
        lines_text(&batch),
        vec!["one two", "three", "four", "five"]
    );
}
