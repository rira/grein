use crate::app::{active_shell_label, AppState, SidebarView};
use crate::engine_port::{SubagentSnapshot, SubagentStatus};
use crate::keymap::BindableAction;
use crate::runner::now_epoch_ms;
use crate::search::{SearchDirection, TranscriptSearchState};
use crate::theme::ThemePalette;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::layout::rounded_block;

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn format_elapsed_ms(started_at_ms: u64, now_ms: u64) -> String {
    if started_at_ms == 0 {
        return "0s".to_string();
    }
    let secs = now_ms.saturating_sub(started_at_ms) / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub(super) fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let sanitized: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = sanitized.chars();
    let collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() && max_chars > 1 {
        let mut trimmed: String = collected.chars().take(max_chars - 1).collect();
        trimmed.push('…');
        trimmed
    } else {
        collected
    }
}

pub(super) fn render_subagent_strip(
    f: &mut Frame,
    area: Rect,
    subagents: &[SubagentSnapshot],
    theme: &ThemePalette,
) {
    let now_ms = now_epoch_ms();
    let mut lines: Vec<Line<'static>> = Vec::new();
    for sa in subagents.iter().take(5) {
        let (glyph, color, default_act) = match sa.status {
            SubagentStatus::Running => ("●", theme.warning, "working"),
            SubagentStatus::Completed => ("✓", theme.success, "done"),
            SubagentStatus::Failed => ("✖", theme.error, "failed"),
        };
        let activity = sa
            .activity
            .as_deref()
            .filter(|a| !a.trim().is_empty())
            .unwrap_or(default_act);
        let elapsed = format_elapsed_ms(sa.started_at_ms, now_ms);

        let mut spans = vec![
            Span::styled(
                format!("{glyph} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("Agent({}: {})", sa.agent_type, sa.role),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(ref model) = sa.model {
            if !model.trim().is_empty() {
                spans.push(Span::styled(
                    format!(" · {model}"),
                    Style::default().fg(theme.muted),
                ));
            }
        }
        spans.push(Span::styled(
            format!("  {activity} · {elapsed}"),
            Style::default().fg(theme.text),
        ));
        lines.push(Line::from(spans));
    }

    if subagents.len() > 5 {
        let overflow = subagents.len() - 5;
        lines.push(Line::from(Span::styled(
            format!("+{overflow} more"),
            Style::default().fg(theme.muted),
        )));
    }

    let block = rounded_block(theme, false).title(" Subagents ");
    let widget = Paragraph::new(lines).block(block);
    f.render_widget(widget, area);
}

fn format_status_git_branch(branch: &str) -> String {
    let clean = branch.trim();
    if let Some(rest) = clean.strip_prefix("worktree-") {
        if rest.len() >= 8
            && rest.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
            && rest.bytes().filter(|b| b.is_ascii_hexdigit()).count() >= 8
        {
            let short_id: String = rest.chars().take(8).collect();
            return format!("worktree: {short_id}");
        }
    }
    if let Some(stripped) = clean.strip_suffix("-wt") {
        if !stripped.is_empty() {
            return format!("worktree: {stripped}");
        }
    }
    clean.to_string()
}

pub(super) fn render_status_bar(
    f: &mut Frame,
    area: Rect,
    state: &AppState,
    subagent_count: usize,
    search: &TranscriptSearchState,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let theme = &state.theme_palette;
    let bar_bg = theme.status_bar_bg;
    let active_id = state.active_thread_id();
    let active_is_terminal = state.is_terminal_thread(active_id);
    let tree_focused = state.active_focus == crate::app::PaneFocus::Tree
        || state.focused_pane == crate::app::PaneFocus::Tree
        || state.focus == crate::app::PaneFocus::Tree;

    let status = state
        .thread_statuses
        .get(active_id)
        .map(String::as_str)
        .unwrap_or("idle");
    let active_tool = state
        .thread_active_tool
        .get(active_id)
        .and_then(|t| t.as_deref())
        .filter(|t| !t.trim().is_empty());
    let thought = state
        .thread_thoughts
        .get(active_id)
        .map(String::as_str)
        .filter(|t| !t.trim().is_empty());

    let is_busy = !active_is_terminal
        && (matches!(status, "streaming" | "executing" | "thinking")
            || active_tool.is_some()
            || thought.is_some());

    let (pill_text, pill_bg, pill_fg) = if tree_focused {
        match state.sidebar_view {
            SidebarView::Grein => (
                " MANAGE · GREIN ",
                theme.pill_manage_bg,
                theme.pill_manage_fg,
            ),
            SidebarView::Worktrees => (
                " MANAGE · WORKTREES ",
                theme.pill_manage_bg,
                theme.pill_manage_fg,
            ),
        }
    } else if active_is_terminal {
        (" WRITE · SHELL ", theme.pill_write_bg, theme.pill_write_fg)
    } else {
        (" WRITE · AGENT ", theme.pill_write_bg, theme.pill_write_fg)
    };

    let (mode, model, live_status, status_color) = if active_is_terminal {
        let (st, col) = match state.terminal_exit_code(active_id) {
            Some(Some(0)) => ("✓ exit 0".to_string(), theme.muted),
            Some(Some(code)) => (format!("✗ exit {code}"), theme.error),
            Some(None) => ("✓ exited".to_string(), theme.muted),
            None => ("● live".to_string(), theme.success),
        };
        ("shell".to_string(), active_shell_label(), st, col)
    } else {
        let (model_opt, mode_opt) = state
            .thread_agent_meta
            .get(active_id)
            .map(|(m, md)| (m.as_deref(), md.as_deref()))
            .unwrap_or((None, None));
        let mode = mode_opt
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("agent")
            .to_string();
        let model = model_opt
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("default")
            .to_string();

        let spin = SPINNER_FRAMES[(state.tick_count as usize) % SPINNER_FRAMES.len()];
        let (live_status, status_color) = if let Some(tool) = active_tool {
            (format!("{spin} tool: {tool}"), theme.warning)
        } else if let Some(th) = thought {
            let preview = truncate_single_line(th, 24);
            (format!("{spin} thinking: {preview}"), theme.warning)
        } else {
            match status {
                "thinking" => (format!("{spin} thinking"), theme.warning),
                "executing" => (format!("{spin} executing"), theme.warning),
                "streaming" => (format!("{spin} streaming"), theme.warning),
                "error" => ("⚠ error".to_string(), theme.error),
                "permission" | "permission_required" => ("⚠ permission".to_string(), theme.error),
                _ => ("✓ idle".to_string(), theme.success),
            }
        };
        (mode, model, live_status, status_color)
    };

    let sa_label = if subagent_count == 1 {
        "1 subagent".to_string()
    } else {
        format!("{subagent_count} subagents")
    };

    let git_meta = state.thread_meta.get(active_id).and_then(|m| {
        m.git_branch
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(|b| (b, m.git_dirty))
    });
    let git_badge = git_meta.map(|(branch, dirty)| {
        let formatted = format_status_git_branch(branch);
        let raw = if dirty > 0 {
            format!("⎇ {formatted}*")
        } else {
            format!("⎇ {formatted}")
        };
        let capped = truncate_single_line(&raw, 28);
        let color = if dirty > 0 { theme.dirty } else { theme.accent };
        (capped, color)
    });
    let provider_badge = (!active_is_terminal)
        .then(|| {
            state
                .thread_meta
                .get(active_id)
                .and_then(|m| m.agent_provider.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|p| truncate_single_line(p, 20))
        })
        .flatten();

    let right_prefix = format!("{mode} · {model} · ");
    let right_suffix = format!(" · {sa_label}");
    let git_prefix_chars = git_badge
        .as_ref()
        .map_or(0, |(badge, _)| badge.chars().count() + 3);
    let provider_prefix_chars = provider_badge
        .as_ref()
        .map_or(0, |p| p.chars().count() + 3);
    let total_right_chars = git_prefix_chars
        + provider_prefix_chars
        + right_prefix.chars().count()
        + live_status.chars().count()
        + right_suffix.chars().count();

    let scrollback_badge = if active_is_terminal {
        state
            .pty_screens
            .get(active_id)
            .filter(|s| s.scrollback_offset > 0)
            .map(|s| format!(" · [scroll -{}]", s.scrollback_offset))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let mut spans = vec![
        Span::styled(
            pill_text,
            Style::default()
                .fg(pill_fg)
                .bg(pill_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().bg(bar_bg)),
    ];
    let mut left_len = pill_text.chars().count() + 1;

    if search.is_visible() {
        let prefix = match search.direction {
            SearchDirection::Forward => '/',
            SearchDirection::Reverse => '?',
        };
        let query_part = format!("{prefix}{}", search.query);
        let counter_part = format!(" {}", search.counter_label());
        let search_hint = if search.editing {
            " · Enter/Esc"
        } else {
            " · [n]ext [N]prev [Esc] clear"
        };
        let counter_color = if search.matches.is_empty() && !search.query.is_empty() {
            theme.error
        } else {
            theme.warning
        };
        spans.push(Span::styled(
            query_part.clone(),
            Style::default()
                .fg(theme.accent)
                .bg(bar_bg)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            counter_part.clone(),
            Style::default().fg(counter_color).bg(bar_bg),
        ));
        left_len += query_part.chars().count() + counter_part.chars().count();
        if left_len + search_hint.chars().count() + 1 + total_right_chars <= area.width as usize {
            spans.push(Span::styled(
                search_hint,
                Style::default().fg(theme.muted).bg(bar_bg),
            ));
            left_len += search_hint.chars().count();
        }
        if !scrollback_badge.is_empty() {
            spans.push(Span::styled(
                scrollback_badge.clone(),
                Style::default()
                    .fg(theme.warning)
                    .bg(bar_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            left_len += scrollback_badge.chars().count();
        }
    } else {
        let manage_key = state
            .keymap
            .chords_for(BindableAction::EnterManageMode)
            .first()
            .map(|c| c.display_label())
            .unwrap_or_else(|| "Ctrl+G".to_string());
        let zoom_key = state
            .keymap
            .chords_for(BindableAction::ToggleZoom)
            .first()
            .map(|c| c.display_label())
            .unwrap_or_else(|| "Alt+Z".to_string());
        let scroll_up_key = state
            .keymap
            .chords_for(BindableAction::ScrollUp)
            .first()
            .map(|c| c.display_label())
            .unwrap_or_else(|| "Alt+K".to_string());
        let scroll_down_key = state
            .keymap
            .chords_for(BindableAction::ScrollDown)
            .first()
            .map(|c| c.display_label())
            .unwrap_or_else(|| "Alt+J".to_string());
        let prev_sib_key = state
            .keymap
            .chords_for(BindableAction::PrevSibling)
            .first()
            .map(|c| c.display_label())
            .unwrap_or_else(|| "Alt+H".to_string());
        let next_sib_key = state
            .keymap
            .chords_for(BindableAction::NextSibling)
            .first()
            .map(|c| c.display_label())
            .unwrap_or_else(|| "Alt+L".to_string());

        let left_hint = if tree_focused {
            match state.sidebar_view {
                SidebarView::Grein => {
                    "[↵]write [n]ew [f]ork [s]hell [w]orktree [r]ename [d]el  : cmd  ? help"
                        .to_string()
                }
                SidebarView::Worktrees => {
                    "[↵]attach [n]ew [s]hell [w]orktree [p]rune [r]efresh [d]el  / filter  ? help"
                        .to_string()
                }
            }
        } else if active_is_terminal {
            if state.terminal_exit_code(active_id).is_some() {
                format!("exited · Enter restart · Esc/{manage_key} manage")
            } else {
                format!("{manage_key} manage · {zoom_key} sidebar · {scroll_up_key}/{scroll_down_key} scroll")
            }
        } else if is_busy {
            format!("Esc cancel · {manage_key} manage · {zoom_key} sidebar")
        } else {
            format!("{manage_key} manage · {zoom_key} sidebar · {scroll_up_key}/{scroll_down_key} scroll · {prev_sib_key}/{next_sib_key} sibling · /help")
        };

        if let Some(notice) = state.status_notice.as_deref() {
            let notice_text = format!(" {notice} ");
            left_len += notice_text.chars().count();
            spans.push(Span::styled(
                notice_text,
                Style::default()
                    .fg(theme.warning)
                    .bg(bar_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            let remaining_for_hint = (area.width as usize)
                .saturating_sub(left_len + total_right_chars + scrollback_badge.chars().count() + 4);
            if remaining_for_hint >= 16 {
                let compact_hint = truncate_single_line(&format!("· {left_hint}"), remaining_for_hint);
                left_len += compact_hint.chars().count();
                spans.push(Span::styled(
                    compact_hint,
                    Style::default().fg(theme.muted).bg(bar_bg),
                ));
            }
        } else {
            let max_hint = (area.width as usize)
                .saturating_sub(left_len + total_right_chars + scrollback_badge.chars().count() + 1)
                .max(12);
            let fitted_hint = truncate_single_line(&left_hint, max_hint);
            left_len += fitted_hint.chars().count();
            spans.push(Span::styled(
                fitted_hint,
                Style::default().fg(theme.muted).bg(bar_bg),
            ));
        }
        if !scrollback_badge.is_empty() {
            left_len += scrollback_badge.chars().count();
            spans.push(Span::styled(
                scrollback_badge,
                Style::default()
                    .fg(theme.warning)
                    .bg(bar_bg)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }

    let width = area.width as usize;
    if left_len + 1 + total_right_chars <= width {
        let gap = width - left_len - total_right_chars;
        spans.push(Span::styled(" ".repeat(gap), Style::default().bg(bar_bg)));
        if let Some((badge, badge_color)) = git_badge {
            spans.push(Span::styled(
                badge,
                Style::default().fg(badge_color).bg(bar_bg),
            ));
            spans.push(Span::styled(
                " · ",
                Style::default().fg(theme.muted).bg(bar_bg),
            ));
        }
        if let Some(ref provider) = provider_badge {
            spans.push(Span::styled(
                provider.clone(),
                Style::default().fg(theme.accent).bg(bar_bg),
            ));
            spans.push(Span::styled(
                " · ",
                Style::default().fg(theme.muted).bg(bar_bg),
            ));
        }
        spans.push(Span::styled(
            right_prefix,
            Style::default().fg(theme.muted).bg(bar_bg),
        ));
        spans.push(Span::styled(
            live_status,
            Style::default().fg(status_color).bg(bar_bg),
        ));
        spans.push(Span::styled(
            right_suffix,
            Style::default().fg(theme.muted).bg(bar_bg),
        ));
    } else if left_len < width {
        let remaining = width - left_len;
        if remaining >= 2 {
            spans.push(Span::styled(" ", Style::default().bg(bar_bg)));
            let compact_git = git_badge
                .as_ref()
                .map(|(badge, _)| format!("{badge} · "))
                .unwrap_or_default();
            let compact_provider = provider_badge
                .as_ref()
                .map(|p| format!("{p} · "))
                .unwrap_or_default();
            let compact_right = truncate_single_line(
                &format!("{compact_git}{compact_provider}{right_prefix}{live_status}{right_suffix}"),
                remaining - 1,
            );
            let pad = remaining.saturating_sub(1 + compact_right.chars().count());
            spans.push(Span::styled(
                compact_right,
                Style::default().fg(theme.muted).bg(bar_bg),
            ));
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), Style::default().bg(bar_bg)));
            }
        }
    }

    let widget = Paragraph::new(Line::from(spans)).style(Style::default().bg(bar_bg));
    f.render_widget(widget, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_elapsed_ms_and_truncation_and_git_branch() {
        assert_eq!(format_elapsed_ms(0, 10_000), "0s");
        assert_eq!(format_elapsed_ms(5_000, 3_000), "0s");
        assert_eq!(format_elapsed_ms(1_000, 45_999), "44s");
        assert_eq!(format_elapsed_ms(1_000, 65_000), "1m 04s");
        assert_eq!(format_elapsed_ms(1_000, 3_665_000), "1h 01m");

        assert_eq!(truncate_single_line("  hello   world  ", 20), "hello world");
        assert_eq!(truncate_single_line("abcdefghij", 5), "abcd…");
        assert_eq!(truncate_single_line("abcdef", 1), "a");
        assert_eq!(truncate_single_line("abcdef", 0), "");

        assert_eq!(
            format_status_git_branch("worktree-abcdef123456"),
            "worktree: abcdef12"
        );
        assert_eq!(format_status_git_branch("feat-wt"), "worktree: feat");
        assert_eq!(format_status_git_branch("main"), "main");
    }
}
