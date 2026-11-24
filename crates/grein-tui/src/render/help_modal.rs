use crate::app::AppState;
use crate::keymap::BindableAction;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use super::layout::rounded_block;

#[derive(Debug, Clone)]
pub struct HelpEntry {
    pub section: &'static str,
    pub keys: String,
    pub description: String,
    pub keywords: &'static str,
}

fn is_word_boundary(chars: &[char], idx: usize) -> bool {
    if idx == 0 {
        return true;
    }
    let prev = chars[idx - 1];
    !prev.is_alphanumeric() || (prev.is_lowercase() && chars[idx].is_uppercase())
}

pub fn fuzzy_match_positions(haystack: &str, query: &str) -> Option<(i32, Vec<usize>)> {
    let q_chars: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if q_chars.is_empty() {
        return Some((0, Vec::new()));
    }

    let h_chars: Vec<char> = haystack.chars().collect();
    let h_low: Vec<char> = h_chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();

    if q_chars.len() > h_low.len() {
        return None;
    }

    let mut qi_check = 0usize;
    for &hc in &h_low {
        if hc == q_chars[qi_check] {
            qi_check += 1;
            if qi_check == q_chars.len() {
                break;
            }
        }
    }
    if qi_check < q_chars.len() {
        return None;
    }

    let q_len = q_chars.len();
    let h_len = h_low.len();
    if q_len > 64 || h_len > 512 {
        let mut positions = Vec::with_capacity(q_len);
        let mut qi = 0usize;
        for (hi, &hc) in h_low.iter().enumerate() {
            if hc == q_chars[qi] {
                positions.push(hi);
                qi += 1;
                if qi == q_len {
                    break;
                }
            }
        }
        return Some((0, positions));
    }

    let q_orig: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    let mut dp: Vec<Vec<Option<i32>>> = vec![vec![None; h_len]; q_len];
    let mut back: Vec<Vec<Option<usize>>> = vec![vec![None; h_len]; q_len];

    for hi in 0..h_len {
        if h_low[hi] == q_chars[0] {
            let mut s = 10i32 - (hi as i32).min(10);
            if is_word_boundary(&h_chars, hi) {
                s += 12;
            }
            if h_chars[hi] == q_orig[0] {
                s += 1;
            }
            dp[0][hi] = Some(s);
        }
    }

    for qi in 1..q_len {
        let mut best_non_adj: Option<(i32, usize)> = None;
        for hi in 0..h_len {
            if hi >= 2 {
                if let Some(prev_s) = dp[qi - 1][hi - 2] {
                    let key = prev_s + (hi as i32 - 2);
                    if best_non_adj.is_none_or(|(b, _)| key > b) {
                        best_non_adj = Some((key, hi - 2));
                    }
                }
            }
            if h_low[hi] != q_chars[qi] {
                continue;
            }
            let mut best_score: Option<i32> = None;
            let mut best_prev: Option<usize> = None;

            if hi >= 1 {
                if let Some(adj_s) = dp[qi - 1][hi - 1] {
                    let cand = adj_s + 15;
                    best_score = Some(cand);
                    best_prev = Some(hi - 1);
                }
            }
            if let Some((key, prev_hi)) = best_non_adj {
                let cand = key - (hi as i32 - 1);
                if best_score.is_none_or(|b| cand > b) {
                    best_score = Some(cand);
                    best_prev = Some(prev_hi);
                }
            }

            if let Some(mut s) = best_score {
                s += 10;
                if is_word_boundary(&h_chars, hi) {
                    s += 12;
                }
                if q_orig.get(qi) == Some(&h_chars[hi]) {
                    s += 1;
                }
                dp[qi][hi] = Some(s);
                back[qi][hi] = best_prev;
            }
        }
    }

    let (end_hi, best_score) = dp[q_len - 1]
        .iter()
        .enumerate()
        .filter_map(|(hi, opt)| opt.map(|s| (hi, s)))
        .max_by_key(|&(_, s)| s)?;

    let mut positions = vec![0usize; q_len];
    let mut cur_hi = Some(end_hi);
    for qi in (0..q_len).rev() {
        let hi = cur_hi?;
        positions[qi] = hi;
        cur_hi = back[qi][hi];
    }

    Some((best_score, positions))
}

fn highlight_spans(
    text: &str,
    matched_positions: &[usize],
    base_style: Style,
    hl_style: Style,
) -> Vec<Span<'static>> {
    if matched_positions.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut cur_hl = false;
    let mut pos_idx = 0usize;

    for (char_idx, ch) in text.chars().enumerate() {
        let is_hl = if pos_idx < matched_positions.len() && matched_positions[pos_idx] == char_idx {
            pos_idx += 1;
            true
        } else {
            false
        };
        if !buf.is_empty() && is_hl != cur_hl {
            let style = if cur_hl { hl_style } else { base_style };
            spans.push(Span::styled(std::mem::take(&mut buf), style));
        }
        cur_hl = is_hl;
        buf.push(ch);
    }
    if !buf.is_empty() {
        let style = if cur_hl { hl_style } else { base_style };
        spans.push(Span::styled(buf, style));
    }
    spans
}

pub fn build_help_entries(state: &AppState) -> Vec<HelpEntry> {
    let km = &state.keymap;
    let enter_manage = km.display_for(BindableAction::EnterManageMode);
    let zoom_key = km.display_for(BindableAction::ToggleZoom);
    let scroll_up = km.display_for(BindableAction::ScrollUp);
    let scroll_down = km.display_for(BindableAction::ScrollDown);
    let prev_sib = km.display_for(BindableAction::PrevSibling);
    let next_sib = km.display_for(BindableAction::NextSibling);
    let tree_up = km.display_for(BindableAction::TreeUp);
    let tree_down = km.display_for(BindableAction::TreeDown);
    let tree_parent = km.display_for(BindableAction::TreeParent);
    let tree_child = km.display_for(BindableAction::TreeChild);
    let tree_keys = if tree_up == "Shift+↑"
        && tree_down == "Shift+↓"
        && tree_parent == "Shift+←"
        && tree_child == "Shift+→"
    {
        "Shift+↑/↓/←/→".to_string()
    } else {
        format!("{tree_up}/{tree_down}/{tree_parent}/{tree_child}")
    };
    let compact_ac = |a| {
        km.display_for(a)
            .replace("Ctrl+", "C-")
            .replace("Shift+Tab", "S-Tab")
            .replace(", ", "/")
    };
    let ac_label = format!(
        "{}, {}/{}/{}",
        compact_ac(BindableAction::AcceptCompletion),
        compact_ac(BindableAction::NextCompletion),
        compact_ac(BindableAction::PrevCompletion),
        compact_ac(BindableAction::DismissCompletion)
    );
    let scroll_label = format!("{scroll_up} / {scroll_down}");
    let sib_label = format!("{prev_sib} / {next_sib}, {tree_keys}");
    let manage_scroll_desc = format!("Full/half-page preview; {scroll_up}/{scroll_down} scroll");

    const WRITE_SEC: &str = "── WRITE Mode (Terminal & Composer) ──";
    const MANAGE_SEC: &str = "── MANAGE Mode (Sidebar) ──";
    const WT_SEC: &str = "── Worktrees View ──";
    const CMD_SEC: &str = "── Palette & Slash Commands ──";

    vec![
        HelpEntry {
            section: WRITE_SEC,
            keys: enter_manage,
            description: "Toggle WRITE <-> MANAGE (sidebar)".to_string(),
            keywords: "hide show sidebar tree focus write manage switch",
        },
        HelpEntry {
            section: WRITE_SEC,
            keys: zoom_key,
            description: "Toggle sidebar zoom (full width)".to_string(),
            keywords: "hide show sidebar zoom fullscreen collapse pane",
        },
        HelpEntry {
            section: WRITE_SEC,
            keys: scroll_label,
            description: "Scroll viewport (Alt+1..9 jump root)".to_string(),
            keywords: "scroll viewport page root alt",
        },
        HelpEntry {
            section: WRITE_SEC,
            keys: sib_label,
            description: "Prev/next sibling; tree parent/child".to_string(),
            keywords: "sibling prev next cycle thread switch tree parent child up down left right shift",
        },
        HelpEntry {
            section: WRITE_SEC,
            keys: "Shift+Enter / Ctrl+J, Ctrl+C".to_string(),
            description: "Newline (Enter send) / cancel / clear".to_string(),
            keywords: "newline enter send cancel clear turn composer",
        },
        HelpEntry {
            section: WRITE_SEC,
            keys: "Ctrl+A/E/B/F/W/U/K/Y".to_string(),
            description: "Readline editing; ↑/↓ navigate lines".to_string(),
            keywords: "readline cursor kill yank move line",
        },
        HelpEntry {
            section: WRITE_SEC,
            keys: ac_label,
            description: "Autocomplete accept / cycle / dismiss".to_string(),
            keywords: "autocomplete ghost tab shift+tab ctrl+y ctrl+n ctrl+p ctrl+e completion word vim",
        },
        HelpEntry {
            section: MANAGE_SEC,
            keys: "Tab".to_string(),
            description: "Switch ᛇ Grein <-> ⎇ Worktrees".to_string(),
            keywords: "tab switch grein worktrees sidebar view",
        },
        HelpEntry {
            section: MANAGE_SEC,
            keys: "j/k/↑/↓, →/l, ↵".to_string(),
            description: "Move / unfold or child / enter WRITE".to_string(),
            keywords: "move navigate down up child enter write",
        },
        HelpEntry {
            section: MANAGE_SEC,
            keys: "h/←/Space, H/L, J/K".to_string(),
            description: "Fold / fold-or-expand all / switch root".to_string(),
            keywords: "fold collapse expand root switch",
        },
        HelpEntry {
            section: MANAGE_SEC,
            keys: "Ctrl+F/B, Ctrl+U/D".to_string(),
            description: manage_scroll_desc,
            keywords: "scroll preview half full page",
        },
        HelpEntry {
            section: MANAGE_SEC,
            keys: "gg / G, Alt+1..9".to_string(),
            description: "Top / bottom item; jump to root 1..9".to_string(),
            keywords: "top bottom jump root",
        },
        HelpEntry {
            section: MANAGE_SEC,
            keys: "n / f / r / d".to_string(),
            description: "New root / fork / inline rename / delete".to_string(),
            keywords: "new root fork branch rename delete",
        },
        HelpEntry {
            section: MANAGE_SEC,
            keys: "s / S, w / m / o / y".to_string(),
            description: "Child/root >_, worktree, join, cards, copy".to_string(),
            keywords: "shell terminal worktree join merge cards copy",
        },
        HelpEntry {
            section: MANAGE_SEC,
            keys: "A, :, /, ?, z, q".to_string(),
            description: "Agents, palette, search, help, zoom, quit".to_string(),
            keywords: "agent provider palette command search filter help zoom hide show sidebar quit",
        },
        HelpEntry {
            section: WT_SEC,
            keys: "Enter, n, s, w".to_string(),
            description: "Attach / new agent / shell / allocate".to_string(),
            keywords: "worktree attach new agent shell allocate branch cwd",
        },
        HelpEntry {
            section: WT_SEC,
            keys: "p / r / d / y / /".to_string(),
            description: "Prune / refresh / delete / copy / filter".to_string(),
            keywords: "worktree prune refresh delete copy filter search",
        },
        HelpEntry {
            section: WT_SEC,
            keys: "l / h / H / L".to_string(),
            description: "Unfold / fold / collapse or expand all".to_string(),
            keywords: "worktree fold collapse expand repo",
        },
        HelpEntry {
            section: CMD_SEC,
            keys: "/theme, /bind, /unbind".to_string(),
            description: "Theme picker and WRITE keybindings".to_string(),
            keywords: "theme color palette dark light swatch bind unbind keybind",
        },
        HelpEntry {
            section: CMD_SEC,
            keys: "/sidebar [dynamic|fixed]".to_string(),
            description: "Configure sidebar sizing & overflow".to_string(),
            keywords: "sidebar width dynamic fixed overflow hide show",
        },
        HelpEntry {
            section: CMD_SEC,
            keys: "/agent /model /mode /effort".to_string(),
            description: "Switch/spawn agent, model, mode, effort".to_string(),
            keywords: "agent provider model mode thinking effort reasoning config",
        },
        HelpEntry {
            section: CMD_SEC,
            keys: "/root /fork /join /term".to_string(),
            description: "Context & shell (!cmd, /wt, /prune)".to_string(),
            keywords: "root fork join merge term shell worktree prune",
        },
        HelpEntry {
            section: CMD_SEC,
            keys: "/copy [code|all], /quit".to_string(),
            description: "Clipboard, /autocomplete, /commands, /zoom".to_string(),
            keywords: "copy clipboard autocomplete ghost commands zoom hide show quit exit",
        },
    ]
}

struct MatchedHelpEntry<'a> {
    entry: &'a HelpEntry,
    key_positions: Vec<usize>,
    desc_positions: Vec<usize>,
}

fn entry_matches<'a>(entry: &'a HelpEntry, query: &str) -> Option<MatchedHelpEntry<'a>> {
    let tokens: Vec<&str> = query.split_whitespace().collect();
    if tokens.is_empty() {
        return Some(MatchedHelpEntry {
            entry,
            key_positions: Vec::new(),
            desc_positions: Vec::new(),
        });
    }
    let mut key_positions = Vec::new();
    let mut desc_positions = Vec::new();
    for tok in tokens {
        let key_match = fuzzy_match_positions(&entry.keys, tok);
        let desc_match = fuzzy_match_positions(&entry.description, tok);
        let sec_match = fuzzy_match_positions(entry.section, tok);
        let kw_match = fuzzy_match_positions(entry.keywords, tok);
        if key_match.is_none()
            && desc_match.is_none()
            && sec_match.is_none()
            && kw_match.is_none()
        {
            return None;
        }
        if let Some((_, pos)) = key_match {
            key_positions.extend(pos);
        }
        if let Some((_, pos)) = desc_match {
            desc_positions.extend(pos);
        }
    }
    key_positions.sort_unstable();
    key_positions.dedup();
    desc_positions.sort_unstable();
    desc_positions.dedup();
    Some(MatchedHelpEntry {
        entry,
        key_positions,
        desc_positions,
    })
}

pub(super) fn render_help_modal(f: &mut Frame, area: Rect, state: &AppState) -> (u16, u16) {
    let theme = &state.theme_palette;
    let section_style = Style::default()
        .fg(theme.warning)
        .add_modifier(Modifier::BOLD);
    let key_unfiltered_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let key_filtered_base_style = Style::default().fg(theme.accent);
    let key_hl_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let desc_base_style = Style::default().fg(theme.text);
    let desc_hl_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    let entries = build_help_entries(state);
    let total_count = entries.len();
    let query = state.help_filter_query.as_str();
    let is_filtered = !query.trim().is_empty();

    let matched: Vec<MatchedHelpEntry<'_>> = entries
        .iter()
        .filter_map(|e| entry_matches(e, query))
        .collect();
    let matched_count = matched.len();

    let mut raw_body: Vec<Line<'static>> = Vec::new();
    let mut current_section: Option<&'static str> = None;
    for item in &matched {
        if current_section != Some(item.entry.section) {
            current_section = Some(item.entry.section);
            raw_body.push(Line::from(Span::styled(
                format!(" {}", item.entry.section),
                section_style,
            )));
        }
        let key_style = if is_filtered {
            key_filtered_base_style
        } else {
            key_unfiltered_style
        };
        let mut spans = vec![Span::raw(" ")];
        spans.extend(highlight_spans(
            &item.entry.keys,
            &item.key_positions,
            key_style,
            key_hl_style,
        ));
        let key_chars = item.entry.keys.chars().count();
        let pad = 28usize.saturating_sub(key_chars) + 1;
        spans.push(Span::raw(" ".repeat(pad)));
        spans.extend(highlight_spans(
            &item.entry.description,
            &item.desc_positions,
            desc_base_style,
            desc_hl_style,
        ));
        raw_body.push(Line::from(spans));
    }

    if raw_body.is_empty() {
        raw_body.push(Line::from(Span::styled(
            format!(" No bindings matching '{}'", state.help_filter_query.trim()),
            Style::default().fg(theme.muted),
        )));
    }

    let show_filter_bar = state.help_filter_active || !state.help_filter_query.is_empty();
    let filter_bar_h = u16::from(show_filter_bar);

    let popup_width = area.width.saturating_sub(6).clamp(30, 76).min(area.width);
    let inner_width = popup_width.saturating_sub(2) as usize;
    let wrapped_body = crate::markdown::wrap_styled_lines(raw_body, inner_width);

    let popup_height = (wrapped_body.len() as u16 + 3 + filter_bar_h).min(area.height.max(1));
    let popup_x = area.x + area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.y + area.height.saturating_sub(popup_height) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    let title = if is_filtered {
        format!(" Keybindings & Commands ({matched_count}/{total_count}) ")
    } else {
        " Keybindings & Commands ".to_string()
    };

    f.render_widget(Clear, popup_area);
    let block = rounded_block(theme, true).title(title);
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    if inner.width == 0 || inner.height == 0 {
        return (0, 0);
    }

    if show_filter_bar && inner.height >= 2 {
        let filter_text = if state.help_filter_active {
            format!(" Filter: {}█ ", state.help_filter_query)
        } else {
            format!(" Filter: {} ", state.help_filter_query)
        };
        let filter_line = Line::from(vec![Span::styled(
            filter_text,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )]);
        f.render_widget(
            Paragraph::new(filter_line),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }

    let body_offset = if show_filter_bar && inner.height >= 2 {
        1u16
    } else {
        0u16
    };
    let body_height = inner.height.saturating_sub(1 + body_offset) as usize;
    let max_scroll = wrapped_body
        .len()
        .saturating_sub(body_height)
        .min(u16::MAX as usize) as u16;
    let scroll_y = state.help_scroll_offset.min(max_scroll);

    if body_height > 0 {
        f.render_widget(
            Paragraph::new(wrapped_body).scroll((scroll_y, 0)),
            Rect::new(
                inner.x,
                inner.y + body_offset,
                inner.width,
                body_height as u16,
            ),
        );
    }

    let footer_text = if state.help_filter_active {
        " Type to filter · Enter lock · ↑/↓/Ctrl+N/P scroll · Esc clear"
    } else if !state.help_filter_query.is_empty() {
        " / Edit filter · Esc clear · j/k scroll · q/?/Enter close"
    } else if max_scroll > 0 {
        " / Filter · Esc/q/? Close · ↑/↓/j/k/Ctrl+U/D/F/B/g/G Scroll"
    } else {
        " / Filter · Esc, q, ?, or Enter to close"
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
