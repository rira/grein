mod pane;
mod render;

pub use pane::{MessageRole, TranscriptMessage, TranscriptPane};
pub(crate) use render::extract_last_fenced_code_block;

use ratatui::text::Line;

use render::{
    clean_summary, render_block, shell_output_line_count, starts_with_code_fence,
    strip_ansi_sequences, trim_trailing_newlines, trimmed_line_count,
};

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
                    if clean.trim().is_empty() {
                        parts.push("✦".to_string());
                    } else if starts_with_code_fence(clean) {
                        parts.push(format!("✦\n{clean}"));
                    } else {
                        parts.push(format!("✦ {clean}"));
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
