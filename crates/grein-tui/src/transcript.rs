use crate::{AppAction, AppEvent, PaneFocus, TuiComponent};
use crossterm::event::KeyCode;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptViewport {
    pub scroll_offset: usize,
    pub max_scroll: usize,
    pub auto_scroll: bool,
}

impl Default for TranscriptViewport {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            max_scroll: 0,
            auto_scroll: true,
        }
    }
}

impl TranscriptViewport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn max_scroll(&self) -> usize {
        self.max_scroll
    }

    pub fn auto_scroll(&self) -> bool {
        self.auto_scroll
    }

    pub fn set_max_scroll(&mut self, max_scroll: usize) {
        self.max_scroll = max_scroll;
        if self.auto_scroll {
            self.scroll_offset = max_scroll;
        } else {
            self.scroll_offset = self.scroll_offset.min(max_scroll);
        }
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.auto_scroll = self.max_scroll == 0;
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self
            .scroll_offset
            .saturating_add(lines)
            .min(self.max_scroll);
        self.auto_scroll = self.scroll_offset >= self.max_scroll;
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = self.max_scroll == 0;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.max_scroll;
        self.auto_scroll = true;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptBlock {
    UserTurn {
        content: String,
    },
    AssistantMarkdown {
        content: String,
    },
    ToolCall {
        id: Option<String>,
        tool_name: String,
        summary: Option<String>,
        output: String,
        completed: bool,
        is_error: bool,
        collapsed: bool,
    },
    ShellExecution {
        command: String,
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
        collapsed: bool,
    },
    JoinNote {
        text: String,
    },
    ErrorNotice {
        message: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadTranscript {
    pub blocks: Vec<TranscriptBlock>,
}

impl ThreadTranscript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
    }

    pub fn push_user(&mut self, content: impl Into<String>) {
        self.blocks.push(TranscriptBlock::UserTurn {
            content: content.into(),
        });
    }

    pub fn append_assistant_chunk(&mut self, chunk: &str) {
        if let Some(TranscriptBlock::AssistantMarkdown { content }) = self.blocks.last_mut() {
            content.push_str(chunk);
        } else if !chunk.is_empty() {
            self.blocks.push(TranscriptBlock::AssistantMarkdown {
                content: chunk.to_string(),
            });
        }
    }

    pub fn start_tool_call(
        &mut self,
        id: Option<String>,
        tool_name: impl Into<String>,
        summary: Option<String>,
    ) {
        self.blocks.push(TranscriptBlock::ToolCall {
            id,
            tool_name: tool_name.into(),
            summary,
            output: String::new(),
            completed: false,
            is_error: false,
            collapsed: false,
        });
    }

    pub fn complete_tool_call(
        &mut self,
        id: Option<&str>,
        tool_name: &str,
        output: impl Into<String>,
        is_error: bool,
    ) {
        let output = output.into();
        let collapsed = output.lines().count() > 4 || output.len() > 320;
        let matched = self.blocks.iter_mut().rev().find(|block| match block {
            TranscriptBlock::ToolCall {
                id: block_id,
                tool_name: block_tool,
                completed,
                ..
            } if !*completed => match id {
                Some(target_id) => block_id.as_deref() == Some(target_id),
                None => block_tool == tool_name,
            },
            _ => false,
        });

        if let Some(TranscriptBlock::ToolCall {
            output: existing_output,
            completed,
            is_error: existing_err,
            collapsed: existing_collapsed,
            ..
        }) = matched
        {
            *existing_output = output;
            *completed = true;
            *existing_err = is_error;
            *existing_collapsed = collapsed;
        } else {
            self.blocks.push(TranscriptBlock::ToolCall {
                id: id.map(str::to_string),
                tool_name: tool_name.to_string(),
                summary: None,
                output,
                completed: true,
                is_error,
                collapsed,
            });
        }
    }

    pub fn push_shell_pending(&mut self, command: impl Into<String>) {
        self.blocks.push(TranscriptBlock::ShellExecution {
            command: command.into(),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            collapsed: false,
        });
    }

    pub fn complete_shell(
        &mut self,
        command: &str,
        stdout: String,
        stderr: String,
        exit_code: i32,
    ) {
        let collapsed = shell_output_line_count(&stdout, &stderr) > 24;
        let matched = self.blocks.iter_mut().rev().find(|block| match block {
            TranscriptBlock::ShellExecution {
                command: block_cmd,
                exit_code: block_exit,
                ..
            } => block_exit.is_none() && block_cmd == command,
            _ => false,
        });

        if let Some(TranscriptBlock::ShellExecution {
            stdout: existing_stdout,
            stderr: existing_stderr,
            exit_code: existing_exit,
            collapsed: existing_collapsed,
            ..
        }) = matched
        {
            *existing_stdout = stdout;
            *existing_stderr = stderr;
            *existing_exit = Some(exit_code);
            *existing_collapsed = collapsed;
        } else {
            self.blocks.push(TranscriptBlock::ShellExecution {
                command: command.to_string(),
                stdout,
                stderr,
                exit_code: Some(exit_code),
                collapsed,
            });
        }
    }

    pub fn push_join_note(&mut self, text: impl Into<String>) {
        self.blocks.push(TranscriptBlock::JoinNote { text: text.into() });
    }

    pub fn push_error(&mut self, message: impl Into<String>) {
        self.blocks.push(TranscriptBlock::ErrorNotice {
            message: message.into(),
        });
    }

    pub fn toggle_all_collapsible(&mut self) -> bool {
        let any_collapsed = self.blocks.iter().any(|block| match block {
            TranscriptBlock::ToolCall {
                completed,
                output,
                collapsed,
                ..
            } => *collapsed && *completed && !trim_trailing_newlines(output).is_empty(),
            TranscriptBlock::ShellExecution {
                stdout,
                stderr,
                exit_code,
                collapsed,
                ..
            } => *collapsed && exit_code.is_some() && shell_output_line_count(stdout, stderr) > 0,
            _ => false,
        });

        if any_collapsed {
            for block in &mut self.blocks {
                match block {
                    TranscriptBlock::ToolCall { collapsed, .. }
                    | TranscriptBlock::ShellExecution { collapsed, .. } => {
                        *collapsed = false;
                    }
                    _ => {}
                }
            }
            true
        } else {
            for block in &mut self.blocks {
                match block {
                    TranscriptBlock::ToolCall {
                        completed,
                        output,
                        collapsed,
                        ..
                    } if *completed => {
                        let line_count = trimmed_line_count(output);
                        if line_count > 1 || output.len() > 320 {
                            *collapsed = true;
                        }
                    }
                    TranscriptBlock::ShellExecution {
                        stdout,
                        stderr,
                        exit_code,
                        collapsed,
                        ..
                    } if exit_code.is_some() && shell_output_line_count(stdout, stderr) > 1 => {
                        *collapsed = true;
                    }
                    _ => {}
                }
            }
            false
        }
    }

    pub fn latest_assistant_text(&self) -> Option<String> {
        self.blocks.iter().rev().find_map(|block| match block {
            TranscriptBlock::AssistantMarkdown { content } if !content.trim().is_empty() => {
                Some(content.clone())
            }
            _ => None,
        })
    }

    pub fn latest_code_block(&self) -> Option<String> {
        let mut last_code: Option<String> = None;
        for block in &self.blocks {
            if let TranscriptBlock::AssistantMarkdown { content } = block {
                extract_last_fenced_code_block(content, &mut last_code);
            }
        }
        last_code
    }

    pub fn full_plain_text(&self) -> String {
        let mut parts = Vec::new();
        for block in &self.blocks {
            match block {
                TranscriptBlock::UserTurn { content } => {
                    let clean = trim_trailing_newlines(content);
                    let mut out = String::new();
                    for (idx, line) in clean.split('\n').enumerate() {
                        if idx > 0 {
                            out.push('\n');
                            out.push_str("  ");
                        } else {
                            out.push_str("❯ ");
                        }
                        out.push_str(line.trim_end_matches('\r'));
                    }
                    parts.push(out);
                }
                TranscriptBlock::AssistantMarkdown { content } => {
                    let clean = trim_trailing_newlines(content);
                    if clean.is_empty() {
                        parts.push("Assistant".to_string());
                    } else {
                        parts.push(format!("Assistant\n{clean}"));
                    }
                }
                TranscriptBlock::ToolCall {
                    tool_name,
                    summary,
                    output,
                    completed,
                    is_error,
                    ..
                } => {
                    let mut out = if !*completed {
                        format!("⚙ {tool_name}")
                    } else {
                        let status = if *is_error { "✖" } else { "✓" };
                        format!("{status} {tool_name}")
                    };
                    if let Some(sum) = clean_summary(summary.as_deref()) {
                        out.push_str(&format!(" ({sum})"));
                    }
                    let clean_out = trim_trailing_newlines(output);
                    if *completed && !clean_out.is_empty() {
                        out.push('\n');
                        out.push_str(clean_out);
                    }
                    parts.push(out);
                }
                TranscriptBlock::ShellExecution {
                    command,
                    stdout,
                    stderr,
                    exit_code,
                    ..
                } => {
                    let mut out = match exit_code {
                        Some(code) => format!("$ {command} (exit {code})"),
                        None => format!("$ {command}"),
                    };
                    let clean_out = strip_ansi_sequences(trim_trailing_newlines(stdout));
                    let clean_err = strip_ansi_sequences(trim_trailing_newlines(stderr));
                    if !clean_out.is_empty() {
                        out.push('\n');
                        out.push_str(&clean_out);
                    }
                    if !clean_err.is_empty() {
                        out.push_str("\n[stderr]\n");
                        out.push_str(&clean_err);
                    }
                    parts.push(out);
                }
                TranscriptBlock::JoinNote { text } => {
                    parts.push(format!("↳ {}", trim_trailing_newlines(text)));
                }
                TranscriptBlock::ErrorNotice { message } => {
                    parts.push(format!("✖ Error: {}", trim_trailing_newlines(message)));
                }
            }
        }
        parts.join("\n\n")
    }

    pub fn render_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        for (idx, block) in self.blocks.iter().enumerate() {
            if idx > 0 {
                let prev_compact_tool = matches!(
                    self.blocks[idx - 1],
                    TranscriptBlock::ToolCall {
                        completed,
                        collapsed,
                        ..
                    } if !completed || collapsed
                );
                let cur_is_tool = matches!(block, TranscriptBlock::ToolCall { .. });
                if !(prev_compact_tool && cur_is_tool) {
                    out.push(Line::raw(""));
                }
            }
            render_block(block, width, &mut out);
        }
        out
    }
}

fn trim_trailing_newlines(text: &str) -> &str {
    text.trim_end_matches(['\r', '\n'])
}

fn trimmed_line_count(text: &str) -> usize {
    let clean = trim_trailing_newlines(text);
    if clean.is_empty() {
        0
    } else {
        clean.lines().count()
    }
}

fn shell_output_line_count(stdout: &str, stderr: &str) -> usize {
    trimmed_line_count(stdout) + trimmed_line_count(stderr)
}

fn clean_summary(summary: Option<&str>) -> Option<&str> {
    summary.map(str::trim).filter(|s| !s.is_empty())
}

fn format_line_count_label(count: usize) -> String {
    if count == 1 {
        "1 line".to_string()
    } else {
        format!("{count} lines")
    }
}

fn render_block(block: &TranscriptBlock, width: usize, out: &mut Vec<Line<'static>>) {
    match block {
        TranscriptBlock::UserTurn { content } => {
            let user_style = Style::default().fg(Color::Green);
            let clean = trim_trailing_newlines(content);
            let mut iter = clean.split('\n');
            let first = iter.next().unwrap_or("").trim_end_matches('\r');
            let mut first_spans =
                vec![Span::styled("❯ ", user_style.add_modifier(Modifier::BOLD))];
            if !first.is_empty() {
                first_spans.push(Span::styled(first.to_string(), user_style));
            }
            out.push(Line::from(first_spans));
            for cont in iter {
                let cont = cont.trim_end_matches('\r');
                let mut spans = vec![Span::styled("  ", user_style)];
                if !cont.is_empty() {
                    spans.push(Span::styled(cont.to_string(), user_style));
                }
                out.push(Line::from(spans));
            }
        }
        TranscriptBlock::AssistantMarkdown { content } => {
            out.push(Line::from(Span::styled(
                "Assistant",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            out.extend(crate::markdown::render_markdown_lines_with_width(
                content, width,
            ));
        }
        TranscriptBlock::ToolCall {
            tool_name,
            summary,
            output,
            completed,
            is_error,
            collapsed,
            ..
        } => render_tool_call(
            tool_name,
            summary.as_deref(),
            output,
            *completed,
            *is_error,
            *collapsed,
            out,
        ),
        TranscriptBlock::ShellExecution {
            command,
            stdout,
            stderr,
            exit_code,
            collapsed,
        } => render_shell_execution(command, stdout, stderr, *exit_code, *collapsed, width, out),
        TranscriptBlock::JoinNote { text } => {
            let style = Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC);
            for (idx, line) in trim_trailing_newlines(text).split('\n').enumerate() {
                let prefix = if idx == 0 { "↳ " } else { "  " };
                out.push(Line::from(Span::styled(
                    format!("{prefix}{}", line.trim_end_matches('\r')),
                    style,
                )));
            }
        }
        TranscriptBlock::ErrorNotice { message } => {
            let style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
            for (idx, line) in trim_trailing_newlines(message).split('\n').enumerate() {
                let prefix = if idx == 0 { "✖ Error: " } else { "         " };
                out.push(Line::from(Span::styled(
                    format!("{prefix}{}", line.trim_end_matches('\r')),
                    style,
                )));
            }
        }
    }
}

fn render_tool_call(
    tool_name: &str,
    summary: Option<&str>,
    output: &str,
    completed: bool,
    is_error: bool,
    collapsed: bool,
    out: &mut Vec<Line<'static>>,
) {
    let summary = clean_summary(summary);
    if !completed {
        let mut spans = vec![Span::styled(
            format!("⚙ {tool_name}"),
            Style::default().fg(Color::Yellow),
        )];
        if let Some(sum) = summary {
            spans.push(Span::styled(
                format!(" ({sum})"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        out.push(Line::from(spans));
        return;
    }

    let (status_glyph, status_color) = if is_error {
        ("✖", Color::Red)
    } else {
        ("✓", Color::Green)
    };
    let line_count = trimmed_line_count(output);

    if collapsed {
        let mut spans = vec![
            Span::styled("▶ ", Style::default().fg(Color::DarkGray)),
            Span::styled(status_glyph, Style::default().fg(status_color)),
            Span::raw(" "),
            Span::styled(
                tool_name.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(sum) = summary {
            spans.push(Span::styled(
                format!(" ({sum})"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        if line_count > 0 {
            spans.push(Span::styled(
                format!(" · {}  [Ctrl+O]", format_line_count_label(line_count)),
                Style::default().fg(Color::DarkGray),
            ));
        }
        out.push(Line::from(spans));
        return;
    }

    let mut header_spans = vec![
        Span::styled("▼ ", Style::default().fg(Color::DarkGray)),
        Span::styled(status_glyph, Style::default().fg(status_color)),
        Span::raw(" "),
        Span::styled(
            tool_name.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(sum) = summary {
        header_spans.push(Span::styled(
            format!(" ({sum})"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    out.push(Line::from(header_spans));

    let clean_output = trim_trailing_newlines(output);
    if clean_output.is_empty() {
        return;
    }

    let gutter_style = Style::default().fg(Color::DarkGray);
    let is_diff = looks_like_unified_diff(clean_output);
    for body_line in clean_output.lines() {
        let body_span = if is_diff {
            style_diff_line(body_line)
        } else {
            Span::raw(body_line.to_string())
        };
        out.push(Line::from(vec![
            Span::styled("│ ", gutter_style),
            body_span,
        ]));
    }
}

fn looks_like_unified_diff(output: &str) -> bool {
    let mut has_minus_hdr = false;
    let mut has_plus_hdr = false;
    for line in output.lines() {
        if line.starts_with("diff --git ") {
            return true;
        }
        if line.starts_with("@@ ") && line[3..].contains("@@") {
            return true;
        }
        if line.starts_with("--- ") {
            has_minus_hdr = true;
        } else if line.starts_with("+++ ") {
            has_plus_hdr = true;
        }
        if has_minus_hdr && has_plus_hdr {
            return true;
        }
    }
    false
}

fn style_diff_line(line: &str) -> Span<'static> {
    if line.starts_with("@@") {
        Span::styled(line.to_string(), Style::default().fg(Color::Cyan))
    } else if line.starts_with("diff --git ")
        || line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
    {
        Span::styled(
            line.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else if line.starts_with('+') {
        Span::styled(line.to_string(), Style::default().fg(Color::Green))
    } else if line.starts_with('-') {
        Span::styled(line.to_string(), Style::default().fg(Color::Red))
    } else {
        Span::raw(line.to_string())
    }
}

fn render_shell_execution(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
    collapsed: bool,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    let Some(code) = exit_code else {
        out.push(Line::from(Span::styled(
            format!("$ {command}"),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        return;
    };

    let exit_color = if code == 0 { Color::Green } else { Color::Red };
    out.push(Line::from(vec![
        Span::styled(
            format!("$ {command} "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("(exit {code})"), Style::default().fg(exit_color)),
    ]));

    let total_lines = shell_output_line_count(stdout, stderr);
    if total_lines == 0 {
        return;
    }

    if collapsed {
        out.push(Line::from(vec![
            Span::styled("▶ ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}  [Ctrl+O]", format_line_count_label(total_lines)),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        return;
    }

    let clean_stdout = trim_trailing_newlines(stdout);
    let clean_stderr = trim_trailing_newlines(stderr);
    let mut combined = String::new();
    if !clean_stdout.is_empty() {
        combined.push_str(clean_stdout);
    }
    if !clean_stderr.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str("[stderr]\n");
        combined.push_str(clean_stderr);
    }

    render_ansi_safe_body(&combined, width, out);
}

fn is_backtick_closing_fence(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 3 && trimmed.bytes().all(|b| b == b'`')
}

fn render_ansi_safe_body(body: &str, width: usize, out: &mut Vec<Line<'static>>) {
    let mut chunk: Vec<&str> = Vec::new();
    for line in body.split('\n') {
        let line = line.trim_end_matches('\r');
        if is_backtick_closing_fence(line) {
            if !chunk.is_empty() {
                let wrapped = format!("```ansi\n{}\n```", chunk.join("\n"));
                out.extend(crate::markdown::render_markdown_lines_with_width(
                    &wrapped, width,
                ));
                chunk.clear();
            }
            out.push(Line::raw(strip_ansi_sequences(line)));
        } else {
            chunk.push(line);
        }
    }
    if !chunk.is_empty() {
        let wrapped = format!("```ansi\n{}\n```", chunk.join("\n"));
        out.extend(crate::markdown::render_markdown_lines_with_width(
            &wrapped, width,
        ));
    }
}

fn parse_fence_open(line: &str) -> Option<(char, usize)> {
    let leading_spaces = line.bytes().take_while(|&b| b == b' ').count();
    if leading_spaces > 3 {
        return None;
    }
    let rest = &line[leading_spaces..];
    let fence_ch = rest.chars().next()?;
    if fence_ch != '`' && fence_ch != '~' {
        return None;
    }
    let count = rest.bytes().take_while(|&b| b == fence_ch as u8).count();
    if count < 3 {
        return None;
    }
    let info = &rest[count..];
    if fence_ch == '`' && info.contains('`') {
        return None;
    }
    Some((fence_ch, count))
}

fn is_fence_close(line: &str, fence_ch: char, min_len: usize) -> bool {
    let leading_spaces = line.bytes().take_while(|&b| b == b' ').count();
    if leading_spaces > 3 {
        return false;
    }
    let rest = &line[leading_spaces..];
    let count = rest.bytes().take_while(|&b| b == fence_ch as u8).count();
    count >= min_len && rest[count..].trim().is_empty()
}

fn extract_last_fenced_code_block(markdown: &str, last_code: &mut Option<String>) {
    let mut active_fence: Option<(char, usize, Vec<&str>)> = None;
    for line in markdown.lines() {
        match &mut active_fence {
            Some((fence_ch, fence_len, body_lines)) => {
                if is_fence_close(line, *fence_ch, *fence_len) {
                    *last_code = Some(body_lines.join("\n"));
                    active_fence = None;
                } else {
                    body_lines.push(line);
                }
            }
            None => {
                if let Some((fence_ch, fence_len)) = parse_fence_open(line) {
                    active_fence = Some((fence_ch, fence_len, Vec::new()));
                }
            }
        }
    }
}

fn strip_ansi_sequences(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut plain_start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            0x1B => {
                out.push_str(&input[plain_start..i]);
                i = skip_escape_sequence(bytes, i);
                plain_start = i;
            }
            b'\r' => {
                out.push_str(&input[plain_start..i]);
                i += 1;
                plain_start = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&input[plain_start..]);
    out
}

fn skip_escape_sequence(bytes: &[u8], esc_idx: usize) -> usize {
    let Some(&next) = bytes.get(esc_idx + 1) else {
        return bytes.len();
    };
    match next {
        b'[' => {
            let mut j = esc_idx + 2;
            while j < bytes.len() && (0x20..=0x3F).contains(&bytes[j]) {
                j += 1;
            }
            if j >= bytes.len() {
                bytes.len()
            } else if (0x40..=0x7E).contains(&bytes[j]) {
                j + 1
            } else {
                j
            }
        }
        b']' => {
            let mut j = esc_idx + 2;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    return j + 1;
                }
                if bytes[j] == 0x1B && bytes.get(j + 1) == Some(&b'\\') {
                    return j + 2;
                }
                j += 1;
            }
            bytes.len()
        }
        b'(' | b')' | b'*' | b'+' => {
            if esc_idx + 2 < bytes.len() && bytes[esc_idx + 2].is_ascii() {
                esc_idx + 3
            } else if esc_idx + 2 < bytes.len() {
                esc_idx + 2
            } else {
                bytes.len()
            }
        }
        b if (0x20..=0x7E).contains(&b) => esc_idx + 2,
        _ => esc_idx + 1,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMessage {
    pub role: MessageRole,
    pub content: String,
}

impl TranscriptMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
        }
    }

    pub fn role(&self) -> &MessageRole {
        &self.role
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

#[derive(Debug, Clone, Default)]
pub struct TranscriptPane {
    focused: bool,
    messages: Vec<TranscriptMessage>,
    scroll_offset: usize,
}

impl TranscriptPane {
    pub fn new() -> Self {
        Self {
            focused: false,
            messages: Vec::new(),
            scroll_offset: 0,
        }
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn add_message(&mut self, message: TranscriptMessage) {
        self.messages.push(message);
    }

    pub fn messages(&self) -> &[TranscriptMessage] {
        &self.messages
    }

    pub fn handle_event(&mut self, event: &AppEvent) -> bool {
        match event {
            AppEvent::FocusChanged(target) => {
                self.focused = *target == PaneFocus::Transcript;
                true
            }
            AppEvent::Key(key) => {
                if !self.focused {
                    return false;
                }
                matches!(key.code, KeyCode::Up | KeyCode::Down)
            }
            _ => false,
        }
    }

    pub fn dispatch_key(&mut self, event: &AppEvent) -> Option<AppAction> {
        if !self.focused {
            return None;
        }
        match event {
            AppEvent::Key(key) => match key.code {
                KeyCode::Down => {
                    self.scroll_offset = self.scroll_offset.saturating_add(1);
                    Some(AppAction::Scrolled { delta: 1 })
                }
                KeyCode::Up => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                    Some(AppAction::Scrolled { delta: -1 })
                }
                _ => None,
            },
            _ => None,
        }
    }
}

impl TuiComponent for TranscriptPane {
    fn is_focused(&self) -> bool {
        self.focused
    }

    fn handle_event(&mut self, event: &AppEvent) -> bool {
        self.handle_event(event)
    }

    fn dispatch_key(&mut self, event: &AppEvent) -> Option<AppAction> {
        self.dispatch_key(event)
    }
}
