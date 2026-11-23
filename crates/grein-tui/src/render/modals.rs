use crate::app::{
    worktree_display_label, AppState, ConfigPickerModal, PendingPermission, PendingQuitConfirm,
    PendingThreadDelete, PendingWorktreeDelete, WorktreeAutocompleteItem,
};
use crate::keymap::BindableAction;
use crate::theme::{ThemePalette, ThemePickerModal, ThemePreset};
use crate::ui::tree::compute_tree_scroll_offset;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::layout::rounded_block;

pub(super) fn render_config_picker_modal(
    f: &mut Frame,
    area: Rect,
    picker: &ConfigPickerModal,
    theme: &ThemePalette,
) {
    let popup_width = area.width.saturating_sub(6).clamp(32, 74).min(area.width);
    let content_rows = picker.options.len() + 3;
    let popup_height = (content_rows as u16 + 2)
        .min(area.height.saturating_sub(2).max(7))
        .min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let block = rounded_block(theme, true).title(format!(" Select {} ", picker.title));
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let scope_style = if picker.set_default {
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    };
    let scope_label = if picker.set_default {
        "Default for new branches"
    } else {
        "This branch only"
    };
    let scope_alt = if picker.set_default {
        "This branch only"
    } else {
        "Default for new branches"
    };
    let scope_line = Line::from(vec![
        Span::styled(" Scope: ", Style::default().fg(theme.muted)),
        Span::styled(format!("[{scope_label}]"), scope_style),
        Span::styled(
            format!("  (Tab/d: {scope_alt})"),
            Style::default().fg(theme.muted),
        ),
    ]);

    let footer_line = Line::from(Span::styled(
        " ↑/↓/j/k Move · Enter Apply · Tab/d Scope · Esc/q Close",
        Style::default().fg(theme.muted),
    ));

    let opt_height = inner.height.saturating_sub(2) as usize;
    let mut opt_lines = Vec::with_capacity(picker.options.len());
    for (idx, choice) in picker.options.iter().enumerate() {
        let is_selected = idx == picker.selected_index;
        let is_active = choice.value == picker.current_value;
        let marker = if is_selected { "▌ " } else { "  " };
        let label = if !choice.name.is_empty() && choice.name != choice.value {
            format!("{} ({})", choice.name, choice.value)
        } else {
            choice.value.clone()
        };
        let desc = choice
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty());

        if is_selected {
            let bg = theme.selection_bg;
            let sel_style = Style::default()
                .fg(theme.selection_fg)
                .bg(bg)
                .add_modifier(Modifier::BOLD);
            let mut spans = vec![Span::styled(
                marker,
                Style::default().fg(theme.selection_bar).bg(bg),
            )];
            if is_active {
                spans.push(Span::styled(
                    "✓ ",
                    Style::default()
                        .fg(theme.success)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled("  ", Style::default().bg(bg)));
            }
            spans.push(Span::styled(label, sel_style));
            if let Some(d) = desc {
                spans.push(Span::styled(
                    format!(" — {d}"),
                    Style::default().fg(theme.muted).bg(bg),
                ));
            }
            opt_lines.push(Line::from(spans));
        } else if is_active {
            let active_style = Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD);
            let mut spans = vec![
                Span::raw(marker),
                Span::styled("✓ ", active_style),
                Span::styled(label, active_style),
            ];
            if let Some(d) = desc {
                spans.push(Span::styled(
                    format!(" — {d}"),
                    Style::default().fg(theme.muted),
                ));
            }
            opt_lines.push(Line::from(spans));
        } else {
            let mut spans = vec![
                Span::raw(marker),
                Span::raw("  "),
                Span::styled(label, Style::default().fg(theme.text)),
            ];
            if let Some(d) = desc {
                spans.push(Span::styled(
                    format!(" — {d}"),
                    Style::default().fg(theme.muted),
                ));
            }
            opt_lines.push(Line::from(spans));
        }
    }

    let scroll_y = if opt_height == 0 || opt_lines.len() <= opt_height {
        0
    } else {
        let max_s = opt_lines.len() - opt_height;
        if picker.selected_index >= opt_height {
            (picker.selected_index + 1 - opt_height).min(max_s) as u16
        } else {
            0
        }
    };

    f.render_widget(
        Paragraph::new(scope_line),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if opt_height > 0 {
        f.render_widget(
            Paragraph::new(opt_lines).scroll((scroll_y, 0)),
            Rect::new(inner.x, inner.y + 1, inner.width, opt_height as u16),
        );
    }
    if inner.height >= 2 {
        f.render_widget(
            Paragraph::new(footer_line),
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
    }
}

pub(super) fn render_theme_picker_modal(
    f: &mut Frame,
    area: Rect,
    picker: &ThemePickerModal,
    state: &AppState,
) {
    let theme = &state.theme_palette;
    let presets = ThemePreset::all();
    let popup_width = area.width.saturating_sub(6).clamp(32, 62).min(area.width);
    let popup_height = (presets.len() as u16 + 5)
        .min(area.height.saturating_sub(2).max(7))
        .min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let block = rounded_block(theme, true).title(" Select Theme (Live Preview) ");
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let header = Line::from(vec![
        Span::styled(" Saved: ", Style::default().fg(theme.muted)),
        Span::styled(
            picker.original_preset.display_name().to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let mut lines = Vec::with_capacity(presets.len());
    for (idx, preset) in presets.iter().enumerate() {
        let is_selected = idx == picker.selected_idx;
        let is_original = *preset == picker.original_preset;
        let suffix = if is_original { " (saved)" } else { "" };
        if is_selected {
            lines.push(Line::from(vec![
                Span::styled(
                    "▌ ",
                    Style::default()
                        .fg(theme.selection_bar)
                        .bg(theme.selection_bg),
                ),
                Span::styled(
                    format!("{} [{}]{suffix}", preset.display_name(), preset.as_str()),
                    Style::default()
                        .fg(theme.selection_fg)
                        .bg(theme.selection_bg)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{} [{}]", preset.display_name(), preset.as_str()),
                    Style::default().fg(theme.text),
                ),
                Span::styled(suffix, Style::default().fg(theme.muted)),
            ]));
        }
    }

    let footer = Line::from(Span::styled(
        " ↑/↓/j/k Preview · Enter Confirm · Esc/q Revert",
        Style::default().fg(theme.muted),
    ));

    f.render_widget(
        Paragraph::new(header),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let list_h = inner.height.saturating_sub(2);
    if list_h > 0 {
        f.render_widget(
            Paragraph::new(lines),
            Rect::new(inner.x, inner.y + 1, inner.width, list_h),
        );
    }
    if inner.height >= 2 {
        f.render_widget(
            Paragraph::new(footer),
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
    }
}

pub(super) fn render_help_modal(f: &mut Frame, area: Rect, state: &AppState) -> (u16, u16) {
    let theme = &state.theme_palette;
    let section_style = Style::default()
        .fg(theme.warning)
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    let km = &state.keymap;
    let enter_manage = km.display_for(BindableAction::EnterManageMode);
    let zoom_key = km.display_for(BindableAction::ToggleZoom);
    let scroll_up = km.display_for(BindableAction::ScrollUp);
    let scroll_down = km.display_for(BindableAction::ScrollDown);
    let prev_sib = km.display_for(BindableAction::PrevSibling);
    let next_sib = km.display_for(BindableAction::NextSibling);
    let scroll_label = format!("{scroll_up} / {scroll_down}");
    let sib_label = format!("{prev_sib} / {next_sib}");
    let manage_scroll_desc = format!("Full/half-page preview; {scroll_up}/{scroll_down} scroll");

    let sections: [(&str, Vec<(String, String)>); 4] = [
        (
            "── WRITE Mode (Terminal & Composer) ──",
            vec![
                (
                    enter_manage,
                    "Toggle WRITE <-> MANAGE (sidebar)".to_string(),
                ),
                (zoom_key, "Toggle sidebar zoom (full width)".to_string()),
                (
                    scroll_label,
                    "Scroll viewport (Alt+1..9 jump root)".to_string(),
                ),
                (sib_label, "Prev / next sibling context".to_string()),
                (
                    "Ctrl+C (Composer)".to_string(),
                    "Cancel turn / clear input / exit hint".to_string(),
                ),
                (
                    "Shift+Enter / Ctrl+J".to_string(),
                    "Newline in Composer (Enter sends)".to_string(),
                ),
                (
                    "Ctrl+A/E/B/F/W/U/K/Y".to_string(),
                    "Readline editing; ↑/↓ navigate lines".to_string(),
                ),
            ],
        ),
        (
            "── MANAGE Mode (Sidebar) ──",
            vec![
                (
                    "Tab".to_string(),
                    "Switch ᛇ Grein <-> ⎇ Worktrees".to_string(),
                ),
                (
                    "j/k/↑/↓, →/l, ↵".to_string(),
                    "Move / unfold or child / enter WRITE".to_string(),
                ),
                (
                    "h/←/Space, H/L, J/K".to_string(),
                    "Fold / fold-or-expand all / switch root".to_string(),
                ),
                ("Ctrl+F/B, Ctrl+U/D".to_string(), manage_scroll_desc),
                (
                    "gg / G, Alt+1..9".to_string(),
                    "Top / bottom item; jump to root 1..9".to_string(),
                ),
                (
                    "n / f / r / d".to_string(),
                    "New root / fork / inline rename / delete".to_string(),
                ),
                (
                    "s / S, w / m / o / y".to_string(),
                    "Child/root >_, worktree, join, cards, copy".to_string(),
                ),
                (
                    ": , /, ?, z, q".to_string(),
                    "Palette / search / help / zoom / quit".to_string(),
                ),
            ],
        ),
        (
            "── Worktrees View ──",
            vec![
                (
                    "Enter, s, w".to_string(),
                    "Attach worktree / shell / allocate".to_string(),
                ),
                (
                    "p / r / d / y / /".to_string(),
                    "Prune / refresh / delete / copy / filter".to_string(),
                ),
                (
                    "l / h / H / L".to_string(),
                    "Unfold / fold / collapse or expand all".to_string(),
                ),
            ],
        ),
        (
            "── Palette & Slash Commands ──",
            vec![
                (
                    "/theme, /bind, /unbind".to_string(),
                    "Theme picker and WRITE keybindings".to_string(),
                ),
                (
                    "/sidebar [dynamic|fixed]".to_string(),
                    "Configure sidebar sizing & overflow".to_string(),
                ),
                (
                    "/model /mode /thinking".to_string(),
                    "Configure agent model, mode, effort".to_string(),
                ),
                (
                    "/root /fork /join /term".to_string(),
                    "Context & shell (!cmd, /wt, /prune)".to_string(),
                ),
                (
                    "/copy [code|all], /quit".to_string(),
                    "Clipboard export, /commands, /zoom, /q".to_string(),
                ),
            ],
        ),
    ];

    let mut raw_body: Vec<Line<'static>> = Vec::new();
    for (title, rows) in sections {
        raw_body.push(Line::from(Span::styled(format!(" {title}"), section_style)));
        for (k, desc) in rows {
            raw_body.push(Line::from(vec![
                Span::styled(format!(" {k:<25} "), key_style),
                Span::styled(desc, Style::default().fg(theme.text)),
            ]));
        }
    }

    let popup_width = area.width.saturating_sub(6).clamp(30, 76).min(area.width);
    let inner_width = popup_width.saturating_sub(2) as usize;
    let wrapped_body = crate::markdown::wrap_styled_lines(raw_body, inner_width);

    let popup_height = (wrapped_body.len() as u16 + 3).min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let block = rounded_block(theme, true).title(" Keybindings & Commands ");
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return (0, 0);
    }

    let body_height = inner.height.saturating_sub(1) as usize;
    let max_scroll = wrapped_body
        .len()
        .saturating_sub(body_height)
        .min(u16::MAX as usize) as u16;
    let scroll_y = state.help_scroll_offset.min(max_scroll);

    if body_height > 0 {
        f.render_widget(
            Paragraph::new(wrapped_body).scroll((scroll_y, 0)),
            Rect::new(inner.x, inner.y, inner.width, body_height as u16),
        );
    }

    let footer_text = if max_scroll > 0 {
        " Esc/q/? Close · ↑/↓/j/k/Ctrl+U/D/F/B/g/G Scroll"
    } else {
        " Esc, q, ?, or Enter to close"
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            footer_text,
            Style::default().fg(theme.muted),
        ))),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
    (scroll_y, max_scroll)
}

pub(super) fn render_permission_modal(
    f: &mut Frame,
    area: Rect,
    perm: &PendingPermission,
    theme: &ThemePalette,
) {
    let popup_width = area.width.saturating_sub(8).clamp(20, 64).min(area.width);
    let popup_height = 9u16.min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let mut popup_block = Block::default()
        .title(" Permission Required ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        );
    if theme.panel_bg != Color::Reset {
        popup_block = popup_block.style(Style::default().bg(theme.panel_bg).fg(theme.text));
    }
    let popup_lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            perm.description.clone(),
            Style::default().fg(theme.text),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "[y] Allow  [n] Deny",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    let popup_widget = Paragraph::new(popup_lines)
        .block(popup_block)
        .wrap(Wrap { trim: false });
    f.render_widget(popup_widget, popup_area);
}

pub(super) fn render_worktree_delete_modal(
    f: &mut Frame,
    area: Rect,
    pending: &PendingWorktreeDelete,
    theme: &ThemePalette,
) {
    let popup_width = area.width.saturating_sub(8).clamp(28, 72).min(area.width);
    let mut lines = Vec::new();
    lines.push(Line::raw(""));
    if pending.dirty_count > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "Dirty ({} files)! Force delete {}? [y/N]",
                pending.dirty_count, pending.label
            ),
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!("Delete worktree {}? [y/N]", pending.label),
            Style::default()
                .fg(theme.text)
                .add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(Span::styled(
        format!("Path: {}", pending.path),
        Style::default().fg(theme.muted),
    )));
    if pending.attached_count > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "{} attached thread(s) will reset to repo root",
                pending.attached_count
            ),
            Style::default().fg(theme.warning),
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "[y] Delete  [n/Esc] Cancel",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));

    let popup_height = (lines.len() as u16 + 3).max(8).min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let border_color = if pending.dirty_count > 0 {
        theme.error
    } else {
        theme.warning
    };
    let mut block = Block::default()
        .title(" Delete Worktree ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        );
    if theme.panel_bg != Color::Reset {
        block = block.style(Style::default().bg(theme.panel_bg).fg(theme.text));
    }
    let widget = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(widget, popup_area);
}

pub(super) fn render_thread_delete_modal(
    f: &mut Frame,
    area: Rect,
    pending: &PendingThreadDelete,
    theme: &ThemePalette,
) {
    let popup_width = area.width.saturating_sub(8).clamp(28, 64).min(area.width);
    let display = if pending.title.trim().is_empty() {
        pending.thread_id.clone()
    } else {
        pending.title.clone()
    };
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("Delete context \"{display}\"?"),
            Style::default()
                .fg(theme.text)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Child branches/terminals under it may also be removed.",
            Style::default().fg(theme.muted),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "[y] Delete  [n/Esc] Cancel",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    let popup_height = (lines.len() as u16 + 3).max(8).min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let mut block = Block::default()
        .title(" Delete Context ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        );
    if theme.panel_bg != Color::Reset {
        block = block.style(Style::default().bg(theme.panel_bg).fg(theme.text));
    }
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        popup_area,
    );
}

pub(super) fn render_quit_confirm_modal(
    f: &mut Frame,
    area: Rect,
    pending: &PendingQuitConfirm,
    theme: &ThemePalette,
) {
    let popup_width = area.width.saturating_sub(8).clamp(28, 60).min(area.width);
    let mut details = Vec::new();
    if pending.active_terminals > 0 {
        details.push(format!("{} live shell(s)", pending.active_terminals));
    }
    if pending.busy_agents > 0 {
        details.push(format!("{} running agent turn(s)", pending.busy_agents));
    }
    let summary = details.join(" and ");
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("Quit Grein while {summary} active?"),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "[y] Quit  [n/Esc] Cancel",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    let popup_height = (lines.len() as u16 + 3).max(7).min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);
    let mut block = Block::default()
        .title(" Confirm Quit ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        );
    if theme.panel_bg != Color::Reset {
        block = block.style(Style::default().bg(theme.panel_bg).fg(theme.text));
    }
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        popup_area,
    );
}

pub(super) fn render_worktree_autocomplete_popup(
    f: &mut Frame,
    composer_area: Rect,
    state: &AppState,
) {
    if composer_area.y < 3 || composer_area.width < 20 {
        return;
    }
    let candidates = state.worktree_autocomplete_candidates();
    if candidates.is_empty() {
        return;
    }
    let max_visible = candidates.len().min(8);
    let popup_height = (max_visible as u16 + 2).min(composer_area.y);
    if popup_height < 3 {
        return;
    }
    let popup_y = composer_area.y.saturating_sub(popup_height);
    let popup_area = Rect::new(composer_area.x, popup_y, composer_area.width, popup_height);
    let theme = &state.theme_palette;

    f.render_widget(Clear, popup_area);
    let block = rounded_block(theme, true).title(" Worktrees (Tab complete · Enter attach) ");
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let selected = state.worktree_autocomplete_idx.min(candidates.len() - 1);
    let scroll = compute_tree_scroll_offset(0, Some(selected), candidates.len(), inner.height);
    let lines: Vec<Line<'static>> = candidates
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let is_selected = idx == selected;
            let bg = theme.selection_bg;
            let sel_style = Style::default()
                .fg(theme.selection_fg)
                .bg(bg)
                .add_modifier(Modifier::BOLD);
            match item {
                WorktreeAutocompleteItem::CreateNew => {
                    if is_selected {
                        Line::from(vec![
                            Span::styled(
                                "▌ ",
                                Style::default().fg(theme.selection_bar).bg(bg),
                            ),
                            Span::styled("+ New isolated worktree", sel_style),
                            Span::styled(
                                " — create branch worktree",
                                Style::default().fg(theme.muted).bg(bg),
                            ),
                        ])
                    } else {
                        Line::from(vec![
                            Span::styled(
                                "  + New isolated worktree",
                                Style::default()
                                    .fg(theme.success)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                " — create branch worktree",
                                Style::default().fg(theme.muted),
                            ),
                        ])
                    }
                }
                WorktreeAutocompleteItem::Existing(entry) => {
                    let branch = worktree_display_label(entry);
                    let primary_tag = if entry.is_primary { " (primary)" } else { "" };
                    if is_selected {
                        let mut spans = vec![
                            Span::styled(
                                "▌ ",
                                Style::default().fg(theme.selection_bar).bg(bg),
                            ),
                            Span::styled(
                                format!("{} │ {branch}{primary_tag}", entry.repo_name),
                                sel_style,
                            ),
                        ];
                        if entry.dirty_count > 0 {
                            spans.push(Span::styled(" │ ", sel_style));
                            spans.push(Span::styled(
                                format!("*{}", entry.dirty_count),
                                Style::default()
                                    .fg(theme.dirty)
                                    .bg(bg)
                                    .add_modifier(Modifier::BOLD),
                            ));
                        }
                        if !entry.attached_thread_ids.is_empty() {
                            spans.push(Span::styled(
                                format!(" [{}]", entry.attached_thread_ids.len()),
                                Style::default().fg(theme.muted).bg(bg),
                            ));
                        }
                        Line::from(spans)
                    } else {
                        let mut spans = vec![
                            Span::raw("  "),
                            Span::styled(
                                entry.repo_name.clone(),
                                Style::default().fg(theme.accent),
                            ),
                            Span::styled(" │ ", Style::default().fg(theme.muted)),
                            Span::styled(
                                format!("{branch}{primary_tag}"),
                                Style::default().fg(theme.text),
                            ),
                        ];
                        if entry.dirty_count > 0 {
                            spans.push(Span::styled(" │ ", Style::default().fg(theme.muted)));
                            spans.push(Span::styled(
                                format!("*{}", entry.dirty_count),
                                Style::default().fg(theme.dirty),
                            ));
                        }
                        if !entry.attached_thread_ids.is_empty() {
                            spans.push(Span::styled(
                                format!(" [{}]", entry.attached_thread_ids.len()),
                                Style::default().fg(theme.muted),
                            ));
                        }
                        Line::from(spans)
                    }
                }
            }
        })
        .collect();

    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}
