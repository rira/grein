use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchDirection {
    #[default]
    Forward,
    Reverse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub line_index: usize,
    pub byte_start: usize,
    pub byte_end: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptSearchState {
    pub editing: bool,
    pub direction: SearchDirection,
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub current_index: Option<usize>,
}

impl TranscriptSearchState {
    pub fn open(&mut self, direction: SearchDirection) {
        self.editing = true;
        self.direction = direction;
        self.query.clear();
        self.matches.clear();
        self.current_index = None;
    }

    pub fn confirm(&mut self) {
        self.editing = false;
        if self.query.is_empty() {
            self.matches.clear();
            self.current_index = None;
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_visible(&self) -> bool {
        self.editing || !self.query.is_empty()
    }

    pub fn recompute_matches(&mut self, lines: &[Line<'_>], anchor_line: Option<usize>) {
        self.matches.clear();
        if self.query.is_empty() {
            self.current_index = None;
            return;
        }

        let case_sensitive = self.query.chars().any(char::is_uppercase);
        let lower_query = if case_sensitive {
            String::new()
        } else {
            self.query.to_lowercase()
        };

        for (line_index, line) in lines.iter().enumerate() {
            let line_text = line_plain_text(line);
            if line_text.is_empty() {
                continue;
            }
            if case_sensitive {
                find_exact_line_matches(line_index, &line_text, &self.query, &mut self.matches);
            } else {
                find_case_insensitive_line_matches(
                    line_index,
                    &line_text,
                    &lower_query,
                    &mut self.matches,
                );
            }
        }

        self.current_index = select_initial_match(&self.matches, self.direction, anchor_line);
    }

    pub fn next_match(&mut self) -> Option<&SearchMatch> {
        let step_forward = matches!(self.direction, SearchDirection::Forward);
        self.step_match(step_forward)
    }

    pub fn prev_match(&mut self) -> Option<&SearchMatch> {
        let step_forward = matches!(self.direction, SearchDirection::Reverse);
        self.step_match(step_forward)
    }

    pub fn active_line_index(&self) -> Option<usize> {
        self.current_index
            .and_then(|idx| self.matches.get(idx))
            .map(|m| m.line_index)
    }

    pub fn counter_label(&self) -> String {
        let total = self.matches.len();
        if total == 0 {
            return "[0/0]".to_string();
        }
        let current = self
            .current_index
            .filter(|&idx| idx < total)
            .map_or(0, |idx| idx + 1);
        format!("[{current}/{total}]")
    }

    fn step_match(&mut self, forward_step: bool) -> Option<&SearchMatch> {
        let len = self.matches.len();
        if len == 0 {
            self.current_index = None;
            return None;
        }
        let next = match (self.current_index, forward_step) {
            (Some(idx), true) if idx < len => (idx + 1) % len,
            (None, true) | (Some(_), true) => 0,
            (Some(idx), false) if idx > 0 && idx < len => idx - 1,
            (_, false) => len - 1,
        };
        self.current_index = Some(next);
        self.matches.get(next)
    }
}

#[derive(Clone, Copy)]
struct TaggedMatch {
    line_index: usize,
    byte_start: usize,
    byte_end: usize,
    is_active: bool,
}

pub fn highlight_lines_with_matches(
    lines: Vec<Line<'static>>,
    matches: &[SearchMatch],
    current_index: Option<usize>,
) -> Vec<Line<'static>> {
    if matches.is_empty() || lines.is_empty() {
        return lines;
    }

    let line_count = lines.len();
    let mut tagged: Vec<TaggedMatch> = matches
        .iter()
        .enumerate()
        .filter(|(_, m)| m.line_index < line_count && m.byte_start < m.byte_end)
        .map(|(idx, m)| TaggedMatch {
            line_index: m.line_index,
            byte_start: m.byte_start,
            byte_end: m.byte_end,
            is_active: current_index == Some(idx),
        })
        .collect();

    if tagged.is_empty() {
        return lines;
    }

    tagged.sort_by_key(|m| (m.line_index, m.byte_start, m.byte_end, !m.is_active));

    let inactive_style = Style::default().bg(Color::DarkGray).fg(Color::White);
    let active_style = Style::default()
        .bg(Color::Yellow)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);

    let mut cursor = 0usize;
    lines
        .into_iter()
        .enumerate()
        .map(|(line_idx, mut line)| {
            while cursor < tagged.len() && tagged[cursor].line_index < line_idx {
                cursor += 1;
            }
            let start_idx = cursor;
            while cursor < tagged.len() && tagged[cursor].line_index == line_idx {
                cursor += 1;
            }
            if start_idx == cursor {
                return line;
            }

            let line_matches = &tagged[start_idx..cursor];
            line.spans = slice_line_spans(line.spans, line_matches, inactive_style, active_style);
            line
        })
        .collect()
}

fn line_plain_text(line: &Line<'_>) -> String {
    let total_len: usize = line.spans.iter().map(|s| s.content.len()).sum();
    let mut text = String::with_capacity(total_len);
    for span in &line.spans {
        text.push_str(span.content.as_ref());
    }
    text
}

fn find_exact_line_matches(
    line_index: usize,
    line_text: &str,
    query: &str,
    out: &mut Vec<SearchMatch>,
) {
    let mut offset = 0usize;
    while let Some(rel) = line_text[offset..].find(query) {
        let byte_start = offset + rel;
        let byte_end = byte_start + query.len();
        out.push(SearchMatch {
            line_index,
            byte_start,
            byte_end,
        });
        offset = byte_end;
    }
}

fn find_case_insensitive_line_matches(
    line_index: usize,
    line_text: &str,
    lower_query: &str,
    out: &mut Vec<SearchMatch>,
) {
    if lower_query.is_empty() {
        return;
    }

    let mut lowered = String::with_capacity(line_text.len());
    let mut boundary_map: Vec<Option<usize>> = Vec::with_capacity(line_text.len() + 1);

    for (orig_start, ch) in line_text.char_indices() {
        let lower_start = lowered.len();
        lowered.extend(ch.to_lowercase());
        let lower_end = lowered.len();
        boundary_map.resize(lower_end, None);
        boundary_map[lower_start] = Some(orig_start);
    }
    boundary_map.push(Some(line_text.len()));

    let mut offset = 0usize;
    while offset <= lowered.len() {
        let Some(rel) = lowered[offset..].find(lower_query) else {
            break;
        };
        let match_start = offset + rel;
        let match_end = match_start + lower_query.len();
        if let (Some(orig_start), Some(orig_end)) = (
            boundary_map.get(match_start).copied().flatten(),
            boundary_map.get(match_end).copied().flatten(),
        ) {
            out.push(SearchMatch {
                line_index,
                byte_start: orig_start,
                byte_end: orig_end,
            });
            offset = match_end;
        } else {
            let step = lowered[match_start..]
                .chars()
                .next()
                .map_or(1, char::len_utf8);
            offset = match_start + step;
        }
    }
}

fn select_initial_match(
    matches: &[SearchMatch],
    direction: SearchDirection,
    anchor_line: Option<usize>,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    match direction {
        SearchDirection::Forward => {
            let anchor = anchor_line.unwrap_or(0);
            matches
                .iter()
                .position(|m| m.line_index >= anchor)
                .or(Some(0))
        }
        SearchDirection::Reverse => match anchor_line {
            Some(anchor) => matches
                .iter()
                .rposition(|m| m.line_index <= anchor)
                .or(Some(matches.len() - 1)),
            None => Some(matches.len() - 1),
        },
    }
}

fn snap_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn slice_line_spans(
    spans: Vec<Span<'static>>,
    line_matches: &[TaggedMatch],
    inactive_style: Style,
    active_style: Style,
) -> Vec<Span<'static>> {
    let mut result = Vec::with_capacity(spans.len());
    let mut span_start = 0usize;

    for span in spans {
        let span_len = span.content.len();
        if span_len == 0 {
            result.push(span);
            continue;
        }
        let span_end = span_start + span_len;
        let mut local_cursor = 0usize;

        {
            let text = span.content.as_ref();
            for &m in line_matches {
                if m.byte_end <= span_start {
                    continue;
                }
                if m.byte_start >= span_end {
                    break;
                }
                let raw_start = m.byte_start.saturating_sub(span_start).min(span_len);
                let raw_end = m.byte_end.saturating_sub(span_start).min(span_len);
                let slice_start = snap_char_boundary(text, raw_start).max(local_cursor);
                let slice_end = snap_char_boundary(text, raw_end);
                if slice_start >= slice_end {
                    continue;
                }
                if slice_start > local_cursor {
                    result.push(Span::styled(
                        text[local_cursor..slice_start].to_string(),
                        span.style,
                    ));
                }
                let highlight_style = if m.is_active {
                    active_style
                } else {
                    inactive_style
                };
                result.push(Span::styled(
                    text[slice_start..slice_end].to_string(),
                    highlight_style,
                ));
                local_cursor = slice_end;
            }

            if local_cursor < span_len {
                result.push(Span::styled(text[local_cursor..].to_string(), span.style));
            }
        }

        span_start = span_end;
    }

    result
}
