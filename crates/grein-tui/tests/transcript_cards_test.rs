use grein_tui::clipboard::{copy_to_clipboard, encode_osc52};
use grein_tui::transcript::{
    MessageRole, ThreadTranscript, TranscriptBlock, TranscriptMessage, TranscriptPane,
    TranscriptViewport,
};
use ratatui::style::{Color, Modifier};
use ratatui::text::Line;

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn lines_text(lines: &[Line<'_>]) -> Vec<String> {
    lines.iter().map(line_text).collect()
}

#[test]
fn test_viewport_keeps_auto_scroll_when_max_scroll_is_zero() {
    let mut vp = TranscriptViewport::new();
    assert!(vp.auto_scroll());
    assert_eq!(vp.max_scroll(), 0);

    vp.scroll_up(5);
    assert!(
        vp.auto_scroll(),
        "scroll_up with max_scroll == 0 must preserve auto_scroll"
    );
    assert_eq!(vp.scroll_offset(), 0);

    vp.scroll_to_top();
    assert!(
        vp.auto_scroll(),
        "scroll_to_top with max_scroll == 0 must preserve auto_scroll"
    );
    assert_eq!(vp.scroll_offset(), 0);

    // Subsequent growth should still auto-scroll to bottom
    vp.set_max_scroll(25);
    assert!(vp.auto_scroll());
    assert_eq!(vp.scroll_offset(), 25);

    // Once max_scroll > 0, scrolling up or to top disables auto_scroll
    vp.scroll_up(3);
    assert!(!vp.auto_scroll());
    assert_eq!(vp.scroll_offset(), 22);

    vp.scroll_to_top();
    assert!(!vp.auto_scroll());
    assert_eq!(vp.scroll_offset(), 0);

    // Backward-compat types remain usable
    let mut pane = TranscriptPane::new();
    let msg = TranscriptMessage::system("sys");
    assert_eq!(*msg.role(), MessageRole::System);
    pane.add_message(msg);
    assert_eq!(pane.message_count(), 1);
}

#[test]
fn test_thread_transcript_lifecycle_matching_and_auto_collapse() {
    let mut t = ThreadTranscript::new();
    assert!(t.is_empty());

    t.push_user("Explain\nthe diff");
    assert!(!t.is_empty());

    t.append_assistant_chunk("Checking ");
    t.append_assistant_chunk("files now.");
    assert_eq!(t.blocks.len(), 2);
    assert_eq!(
        t.blocks[1],
        TranscriptBlock::AssistantMarkdown {
            content: "Checking files now.".to_string(),
        }
    );

    // Two concurrent tool calls with same tool_name distinguished by id
    t.start_tool_call(Some("call-1".into()), "read_file", Some("a.rs".into()));
    t.start_tool_call(Some("call-2".into()), "read_file", Some("b.rs".into()));
    t.start_tool_call(None, "grep", Some("TODO".into()));

    // Complete call-1 (short output <= 4 lines & <= 320 bytes -> expanded)
    t.complete_tool_call(Some("call-1"), "read_file", "line1\nline2\nline3\nline4", false);
    match &t.blocks[2] {
        TranscriptBlock::ToolCall {
            id,
            summary,
            completed,
            is_error,
            collapsed,
            ..
        } => {
            assert_eq!(id.as_deref(), Some("call-1"));
            assert_eq!(summary.as_deref(), Some("a.rs"));
            assert!(*completed);
            assert!(!*is_error);
            assert!(!*collapsed);
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }

    // Complete call-2 (> 4 lines -> collapsed)
    t.complete_tool_call(Some("call-2"), "read_file", "1\n2\n3\n4\n5", true);
    match &t.blocks[3] {
        TranscriptBlock::ToolCall {
            completed,
            is_error,
            collapsed,
            ..
        } => {
            assert!(*completed);
            assert!(*is_error);
            assert!(*collapsed);
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }

    // Complete grep by tool_name (1 long line > 320 bytes -> collapsed)
    let long_line = "x".repeat(321);
    t.complete_tool_call(None, "grep", long_line, false);
    match &t.blocks[4] {
        TranscriptBlock::ToolCall {
            tool_name,
            completed,
            collapsed,
            ..
        } => {
            assert_eq!(tool_name, "grep");
            assert!(*completed);
            assert!(*collapsed);
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }

    // Unmatched completion pushes new completed ToolCall
    t.complete_tool_call(Some("missing"), "stat", "ok", false);
    assert_eq!(t.blocks.len(), 6);

    // Shell pending + complete
    t.push_shell_pending("echo hi");
    let twenty_four = (1..=24).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n");
    t.complete_shell("echo hi", twenty_four, String::new(), 0);
    match &t.blocks[6] {
        TranscriptBlock::ShellExecution {
            exit_code,
            collapsed,
            ..
        } => {
            assert_eq!(*exit_code, Some(0));
            assert!(!*collapsed, "24 lines should not auto-collapse");
        }
        other => panic!("expected ShellExecution, got {other:?}"),
    }

    // Unmatched shell > 24 lines auto-collapses
    let twenty_five = (1..=25).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n");
    t.complete_shell("ls -la", twenty_five, "warn".into(), 1);
    match &t.blocks[7] {
        TranscriptBlock::ShellExecution {
            exit_code,
            collapsed,
            ..
        } => {
            assert_eq!(*exit_code, Some(1));
            assert!(*collapsed, "> 24 lines should auto-collapse");
        }
        other => panic!("expected ShellExecution, got {other:?}"),
    }

    t.push_join_note("Merged branch foo");
    t.push_error("Stream disconnected");
    assert_eq!(t.blocks.len(), 10);

    t.clear();
    assert!(t.is_empty());
}

#[test]
fn test_toggle_all_collapsible_behavior() {
    let mut t = ThreadTranscript::new();
    // 1-line tool call (expanded)
    t.complete_tool_call(None, "ping", "pong", false);
    // Multi-line tool call (6 lines -> collapsed)
    t.complete_tool_call(None, "read", "a\nb\nc\nd\ne\nf", false);
    // 2-line shell (initially expanded since <= 24)
    t.complete_shell("pwd", "/tmp\n/var".into(), String::new(), 0);
    // Empty completed tool call (even if marked collapsed, has empty output)
    t.blocks.push(TranscriptBlock::ToolCall {
        id: None,
        tool_name: "noop".into(),
        summary: None,
        output: String::new(),
        completed: true,
        is_error: false,
        collapsed: true,
    });

    // Since "read" (non-empty) is collapsed, toggle_all_collapsible expands all & returns true
    assert!(t.toggle_all_collapsible());
    for block in &t.blocks {
        match block {
            TranscriptBlock::ToolCall { collapsed, .. }
            | TranscriptBlock::ShellExecution { collapsed, .. } => assert!(!*collapsed),
            _ => {}
        }
    }

    // Now none with non-empty output is collapsed -> collapses multi-line ones & returns false
    assert!(!t.toggle_all_collapsible());
    match &t.blocks[0] {
        TranscriptBlock::ToolCall { collapsed, .. } => {
            assert!(!*collapsed, "single-line short tool output stays expanded");
        }
        _ => unreachable!(),
    }
    match &t.blocks[1] {
        TranscriptBlock::ToolCall { collapsed, .. } => {
            assert!(*collapsed, "multi-line tool output collapses");
        }
        _ => unreachable!(),
    }
    match &t.blocks[2] {
        TranscriptBlock::ShellExecution { collapsed, .. } => {
            assert!(*collapsed, "multi-line shell output collapses");
        }
        _ => unreachable!(),
    }

    // With only empty-output blocks, toggle should not report collapsed non-empty blocks
    let mut empty_only = ThreadTranscript::new();
    empty_only.blocks.push(TranscriptBlock::ToolCall {
        id: None,
        tool_name: "noop".into(),
        summary: None,
        output: String::new(),
        completed: true,
        is_error: false,
        collapsed: true,
    });
    assert!(!empty_only.toggle_all_collapsible());
}

#[test]
fn test_latest_assistant_text_latest_code_block_and_plain_text() {
    let mut t = ThreadTranscript::new();
    assert_eq!(t.latest_assistant_text(), None);
    assert_eq!(t.latest_code_block(), None);

    t.push_user("Show examples");
    t.append_assistant_chunk("Here is Python:\n~~~python\nprint('hi')\n~~~\n");
    t.complete_tool_call(None, "bash", "ok", false);
    t.append_assistant_chunk(
        "And nested markdown:\n````markdown\n```rust\nfn main() {}\n```\n````\nDone.",
    );

    assert_eq!(
        t.latest_assistant_text().as_deref(),
        Some("And nested markdown:\n````markdown\n```rust\nfn main() {}\n```\n````\nDone.")
    );
    assert_eq!(
        t.latest_code_block().as_deref(),
        Some("```rust\nfn main() {}\n```")
    );

    t.complete_shell(
        "echo color",
        "\x1b[32mgreen_out\x1b[0m\n".into(),
        "err_line\n".into(),
        0,
    );
    t.push_join_note("Joined branch alpha");
    t.push_error("Something failed");

    let plain = t.full_plain_text();
    assert!(plain.contains("Show examples"));
    assert!(plain.contains("And nested markdown:"));
    assert!(plain.contains("green_out"));
    assert!(!plain.contains("\x1b[32m"));
    assert!(plain.contains("err_line"));
    assert!(plain.contains("Joined branch alpha"));
    assert!(plain.contains("Something failed"));
}

#[test]
fn test_render_lines_user_assistant_join_error_and_tool_cards() {
    let mut t = ThreadTranscript::new();
    t.push_user("First line\nSecond line");
    t.append_assistant_chunk("Hello **world**");
    t.start_tool_call(Some("1".into()), "grep", Some("pattern".into()));
    t.complete_tool_call(
        Some("2"),
        "read_file",
        "1\n2\n3\n4\n5\n6",
        false,
    );
    if let TranscriptBlock::ToolCall { summary, .. } = &mut t.blocks[3] {
        *summary = Some("src/lib.rs".into());
    }
    t.complete_tool_call(Some("3"), "write_file", "", true);
    t.push_join_note("Joined worker-1");
    t.push_error("Disk full");

    let rendered = t.render_lines(80);
    let texts = lines_text(&rendered);

    // UserTurn: green bold ❯ + green continuation
    let user_line0 = rendered
        .iter()
        .find(|l| line_text(l) == "❯ First line")
        .expect("user first line missing");
    assert_eq!(user_line0.spans[0].content, "❯ ");
    assert_eq!(user_line0.spans[0].style.fg, Some(Color::Green));
    assert!(user_line0.spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(user_line0.spans[1].style.fg, Some(Color::Green));

    let user_line1 = rendered
        .iter()
        .find(|l| line_text(l) == "  Second line")
        .expect("user continuation line missing");
    assert_eq!(user_line1.spans[0].content, "  ");
    assert_eq!(user_line1.spans[0].style.fg, Some(Color::Green));
    assert_eq!(user_line1.spans[1].style.fg, Some(Color::Green));

    // AssistantMarkdown: bold Assistant heading + rendered markdown
    let asst_heading = rendered
        .iter()
        .find(|l| line_text(l) == "Assistant")
        .expect("Assistant heading missing");
    assert!(asst_heading
        .spans
        .iter()
        .all(|s| s.style.add_modifier.contains(Modifier::BOLD)));
    assert!(texts.iter().any(|l| l == "Hello world"));

    // Running ToolCall: yellow ⚙ grep + DarkGray (pattern)
    let running_line = rendered
        .iter()
        .find(|l| line_text(l) == "⚙ grep (pattern)")
        .expect("running tool call missing");
    assert_eq!(running_line.spans[0].style.fg, Some(Color::Yellow));
    assert_eq!(running_line.spans[1].style.fg, Some(Color::DarkGray));

    // Collapsed completed ToolCall: ▶ ✓ read_file (src/lib.rs) · 6 lines  [Ctrl+O]
    let collapsed_line = rendered
        .iter()
        .find(|l| line_text(l) == "▶ ✓ read_file (src/lib.rs) · 6 lines  [Ctrl+O]")
        .expect("collapsed tool call card missing");
    assert!(collapsed_line
        .spans
        .iter()
        .any(|s| s.content == "✓" && s.style.fg == Some(Color::Green)));

    // Empty error tool call omits 0 lines and empty summary
    let empty_err_line = rendered
        .iter()
        .find(|l| line_text(l).contains("write_file"))
        .expect("empty error tool call missing");
    let empty_err_txt = line_text(empty_err_line);
    assert!(!empty_err_txt.contains("0 lines"));
    assert!(!empty_err_txt.contains("()"));
    assert!(empty_err_line
        .spans
        .iter()
        .any(|s| s.content == "✖" && s.style.fg == Some(Color::Red)));

    // JoinNote: DarkGray italic ↳ ...
    let join_line = rendered
        .iter()
        .find(|l| line_text(l) == "↳ Joined worker-1")
        .expect("join note missing");
    assert_eq!(join_line.spans[0].style.fg, Some(Color::DarkGray));
    assert!(join_line.spans[0]
        .style
        .add_modifier
        .contains(Modifier::ITALIC));

    // ErrorNotice: Red bold ✖ Error: ...
    let err_line = rendered
        .iter()
        .find(|l| line_text(l) == "✖ Error: Disk full")
        .expect("error notice missing");
    assert_eq!(err_line.spans[0].style.fg, Some(Color::Red));
    assert!(err_line.spans[0].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn test_render_lines_expanded_tool_diff_and_backtick_safety_and_shell_cards() {
    let mut t = ThreadTranscript::new();

    // Expanded tool call with backticks in output (must not be wrapped in 3-backtick markdown fences)
    t.complete_tool_call(
        None,
        "read_file",
        "```rust\nlet x = 1;\n```",
        false,
    );

    // Expanded tool call with unified diff output
    t.blocks.push(TranscriptBlock::ToolCall {
        id: None,
        tool_name: "apply_patch".into(),
        summary: Some("src/main.rs".into()),
        output: "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,2 +1,2 @@\n-old\n+new\n ctx".into(),
        completed: true,
        is_error: false,
        collapsed: false,
    });

    // Shell pending, collapsed, and expanded with backticks + ansi + stderr
    t.push_shell_pending("cargo build");
    t.blocks.push(TranscriptBlock::ShellExecution {
        command: "seq 30".into(),
        stdout: (1..=30).map(|i| format!("{i}")).collect::<Vec<_>>().join("\n"),
        stderr: String::new(),
        exit_code: Some(0),
        collapsed: true,
    });
    t.complete_shell(
        "cat snippet.md",
        "\x1b[32mheader\x1b[0m\n```\ncode\n```".into(),
        "warn\n".into(),
        2,
    );

    let rendered = t.render_lines(80);
    let texts = lines_text(&rendered);

    // Expanded read_file shows ▼ ✓ read_file and gutter lines preserving literal ```rust
    assert!(texts.iter().any(|l| l == "▼ ✓ read_file"));
    assert!(texts.iter().any(|l| l == "│ ```rust"));
    assert!(texts.iter().any(|l| l == "│ let x = 1;"));
    assert!(texts.iter().any(|l| l == "│ ```"));

    // Diff lines get colorized inside │ gutter
    assert!(texts.iter().any(|l| l == "▼ ✓ apply_patch (src/main.rs)"));
    let hunk_line = rendered
        .iter()
        .find(|l| line_text(l) == "│ @@ -1,2 +1,2 @@")
        .expect("diff hunk missing");
    assert!(hunk_line
        .spans
        .iter()
        .any(|s| s.content.contains("@@") && s.style.fg == Some(Color::Cyan)));

    let del_line = rendered
        .iter()
        .find(|l| line_text(l) == "│ -old")
        .expect("diff deletion missing");
    assert!(del_line
        .spans
        .iter()
        .any(|s| s.content == "-old" && s.style.fg == Some(Color::Red)));

    let add_line = rendered
        .iter()
        .find(|l| line_text(l) == "│ +new")
        .expect("diff addition missing");
    assert!(add_line
        .spans
        .iter()
        .any(|s| s.content == "+new" && s.style.fg == Some(Color::Green)));

    // Shell pending
    assert!(texts.iter().any(|l| l == "$ cargo build"));

    // Shell collapsed
    let seq_hdr = rendered
        .iter()
        .find(|l| line_text(l) == "$ seq 30 (exit 0)")
        .expect("seq header missing");
    assert!(seq_hdr
        .spans
        .iter()
        .any(|s| s.content == "(exit 0)" && s.style.fg == Some(Color::Green)));
    assert!(texts.iter().any(|l| l == "▶ 30 lines  [Ctrl+O]"));

    // Shell expanded with backticks + ANSI + stderr
    let cat_hdr = rendered
        .iter()
        .find(|l| line_text(l) == "$ cat snippet.md (exit 2)")
        .expect("cat header missing");
    assert!(cat_hdr
        .spans
        .iter()
        .any(|s| s.content == "(exit 2)" && s.style.fg == Some(Color::Red)));
    let green_header = rendered
        .iter()
        .find(|l| line_text(l) == "header")
        .expect("shell ansi green line missing");
    assert_eq!(green_header.spans[0].style.fg, Some(Color::Green));
    assert!(texts.iter().any(|l| l == "code"));
    let stderr_hdr = rendered
        .iter()
        .find(|l| line_text(l) == "[stderr]")
        .expect("stderr header missing");
    assert_eq!(stderr_hdr.spans[0].style.fg, Some(Color::Red));
    assert!(texts.iter().any(|l| l == "warn"));
}

#[test]
fn test_clipboard_osc52_and_tmux_passthrough_and_headless_safety() {
    assert_eq!(encode_osc52("", false), "\x1b]52;c;\x07");
    assert_eq!(encode_osc52("f", false), "\x1b]52;c;Zg==\x07");
    assert_eq!(encode_osc52("fo", false), "\x1b]52;c;Zm8=\x07");
    assert_eq!(encode_osc52("foo", false), "\x1b]52;c;Zm9v\x07");
    assert_eq!(encode_osc52("🦀", false), "\x1b]52;c;8J+mgA==\x07");

    assert_eq!(
        encode_osc52("hello", true),
        "\x1bPtmux;\x1b\x1b]52;c;aGVsbG8=\x07\x1b\\"
    );

    // Must not panic in headless test environments
    let _ = copy_to_clipboard("hello clipboard");
}
