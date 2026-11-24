use crate::app::{AgentPickerModal, AppState};
use crate::engine_port::AgentProviderInfo;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use super::layout::rounded_block;

fn provider_display_for_id(providers: &[AgentProviderInfo], id: &str) -> String {
    providers
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(id))
        .map(|p| p.display_name.clone())
        .unwrap_or_else(|| id.to_string())
}

pub(super) fn palette_window_bounds(total: usize, selected: usize, max_visible: usize) -> (usize, usize) {
    let clamped_sel = selected.min(total.saturating_sub(1));
    let start = if max_visible == 0 || total <= max_visible {
        0
    } else {
        let max_start = total - max_visible;
        if clamped_sel >= max_visible {
            (clamped_sel + 1 - max_visible).min(max_start)
        } else {
            0
        }
    };
    (start, clamped_sel)
}

pub(super) fn render_agent_picker_modal(
    f: &mut Frame,
    area: Rect,
    picker: &AgentPickerModal,
    state: &AppState,
) {
    let theme = &state.theme_palette;
    let providers = &state.agent_providers;
    let selected_idx = picker.selected_idx.min(providers.len().saturating_sub(1));
    let selected_provider = providers.get(selected_idx);
    let show_hint = selected_provider.is_some_and(|p| !p.available);

    let popup_width = area.width.saturating_sub(6).clamp(36, 76).min(area.width);
    let item_rows = providers.len().max(1);
    let content_rows = 1 + item_rows + usize::from(show_hint) + 1 + 1;
    let popup_height = (content_rows as u16 + 2)
        .min(area.height.saturating_sub(2).max(7))
        .min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let block = rounded_block(theme, true).title(" Select Agent ");
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let target_id = if picker.target_thread_id.is_empty() {
        state.active_thread_id.as_str()
    } else {
        picker.target_thread_id.as_str()
    };
    let active_provider_id = state
        .thread_meta
        .get(target_id)
        .and_then(|m| m.agent_provider.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let active_label = active_provider_id
        .map(|id| provider_display_for_id(providers, id))
        .unwrap_or_else(|| "default".to_string());
    let default_label = providers
        .iter()
        .find(|p| p.is_default)
        .map(|p| p.display_name.clone())
        .or_else(|| {
            state
                .default_agent_provider
                .as_deref()
                .map(|id| provider_display_for_id(providers, id))
        })
        .unwrap_or_else(|| "none".to_string());

    let header = Line::from(vec![
        Span::styled(" Active: ", Style::default().fg(theme.muted)),
        Span::styled(
            active_label,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · Default: ", Style::default().fg(theme.muted)),
        Span::styled(default_label, Style::default().fg(theme.text)),
    ]);

    let lines: Vec<Line<'static>> = if providers.is_empty() {
        vec![Line::from(Span::styled(
            "  No agent providers discovered",
            Style::default().fg(theme.muted),
        ))]
    } else {
        providers
            .iter()
            .enumerate()
            .map(|(idx, provider)| {
                let is_selected = idx == selected_idx;
                let is_active =
                    active_provider_id.is_some_and(|id| id.eq_ignore_ascii_case(&provider.id));
                let suffix = match (is_active, provider.is_default) {
                    (true, true) => " (active, default)",
                    (true, false) => " (active)",
                    (false, true) => " (default)",
                    (false, false) => "",
                };
                let status_label = if provider.available {
                    "ready"
                } else {
                    "not found"
                };
                let dot_color = if provider.available {
                    theme.success
                } else {
                    theme.muted
                };
                let name = provider.display_name.as_str();
                let used_w = 4 + name.chars().count() + suffix.chars().count();
                let right_w = status_label.chars().count() + 1;
                let gap = (inner.width as usize).saturating_sub(used_w + right_w).max(1);

                if is_selected {
                    let bg = theme.selection_bg;
                    let mut name_style = Style::default()
                        .fg(if provider.available {
                            theme.selection_fg
                        } else {
                            theme.muted
                        })
                        .bg(bg);
                    if provider.available {
                        name_style = name_style.add_modifier(Modifier::BOLD);
                    }
                    Line::from(vec![
                        Span::styled("▌ ", Style::default().fg(theme.selection_bar).bg(bg)),
                        Span::styled("●", Style::default().fg(dot_color).bg(bg)),
                        Span::styled(" ", Style::default().bg(bg)),
                        Span::styled(format!("{name}{suffix}"), name_style),
                        Span::styled(" ".repeat(gap), Style::default().bg(bg)),
                        Span::styled(
                            format!("{status_label} "),
                            Style::default().fg(theme.muted).bg(bg),
                        ),
                    ])
                } else {
                    let name_style = if provider.available {
                        Style::default().fg(theme.text)
                    } else {
                        Style::default().fg(theme.muted)
                    };
                    let mut spans = vec![
                        Span::raw("  "),
                        Span::styled("●", Style::default().fg(dot_color)),
                        Span::raw(" "),
                        Span::styled(name.to_string(), name_style),
                    ];
                    if !suffix.is_empty() {
                        spans.push(Span::styled(suffix, Style::default().fg(theme.muted)));
                    }
                    spans.push(Span::raw(" ".repeat(gap)));
                    spans.push(Span::styled(
                        format!("{status_label} "),
                        Style::default().fg(theme.muted),
                    ));
                    Line::from(spans)
                }
            })
            .collect()
    };

    f.render_widget(
        Paragraph::new(header),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let footer_rows = u16::from(inner.height >= 2);
    let hint_rows = u16::from(show_hint && inner.height >= 4);
    let list_h = inner.height.saturating_sub(1 + footer_rows + hint_rows) as usize;
    if list_h > 0 {
        let scroll_y = if lines.len() <= list_h {
            0
        } else {
            let max_s = lines.len() - list_h;
            if selected_idx >= list_h {
                (selected_idx + 1 - list_h).min(max_s) as u16
            } else {
                0
            }
        };
        f.render_widget(
            Paragraph::new(lines).scroll((scroll_y, 0)),
            Rect::new(inner.x, inner.y + 1, inner.width, list_h as u16),
        );
    }

    if hint_rows == 1 {
        let hint_line = match selected_provider
            .and_then(|p| p.install_hint.as_deref())
            .map(str::trim)
            .filter(|h| !h.is_empty())
        {
            Some(hint) => Line::from(vec![
                Span::styled(
                    " Install: ",
                    Style::default()
                        .fg(theme.warning)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(hint.to_string(), Style::default().fg(theme.muted)),
            ]),
            None => Line::from(Span::styled(
                " Not found on PATH",
                Style::default().fg(theme.muted),
            )),
        };
        f.render_widget(
            Paragraph::new(hint_line),
            Rect::new(inner.x, inner.y + inner.height - 2, inner.width, 1),
        );
    }

    if footer_rows == 1 {
        let footer = Line::from(Span::styled(
            " ↑/↓/j/k Select · Enter New sibling · s Switch · d Default · Esc/q Close",
            Style::default().fg(theme.muted),
        ));
        f.render_widget(
            Paragraph::new(footer),
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
    }
}
