use ratatui::style::Style;
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineMeta {
    pub soft_wrapped: bool,
    pub joined_by_space: bool,
}

#[derive(Debug, Clone)]
enum CellSegment {
    Word(Vec<Span<'static>>, usize),
    Space(Vec<Span<'static>>, usize),
}

struct MetaTracker<'a> {
    out_meta: &'a mut Vec<LineMeta>,
    next_meta: LineMeta,
}

pub(crate) fn char_display_width(ch: char) -> usize {
    if ch.is_ascii() {
        if ch.is_control() {
            0
        } else {
            1
        }
    } else {
        let mut buf = [0u8; 4];
        Span::raw(&*ch.encode_utf8(&mut buf)).width()
    }
}

fn str_display_width(s: &str) -> usize {
    if s.is_ascii() {
        s.bytes().filter(|b| !b.is_ascii_control()).count()
    } else {
        Span::raw(s).width()
    }
}

pub(super) fn spans_display_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| str_display_width(s.content.as_ref())).sum()
}

pub(super) fn push_merged_span(target: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = target.last_mut() {
        if last.style == style {
            last.content.to_mut().push_str(text);
            return;
        }
    }
    target.push(Span::styled(text.to_string(), style));
}

pub(super) fn push_merged_char(target: &mut Vec<Span<'static>>, ch: char, style: Style) {
    if let Some(last) = target.last_mut() {
        if last.style == style {
            last.content.to_mut().push(ch);
            return;
        }
    }
    target.push(Span::styled(ch.to_string(), style));
}

fn tokenize_spans(spans: Vec<Span<'static>>, normalize_ws: bool) -> Vec<CellSegment> {
    let mut segments: Vec<CellSegment> = Vec::new();
    for span in spans {
        let style = span.style;
        let text = span.content.as_ref();
        let mut cur_start = 0usize;
        let mut cur_is_ws: Option<bool> = None;

        for (byte_idx, ch) in text.char_indices() {
            let is_ws = ch.is_whitespace();
            match cur_is_ws {
                None => {
                    cur_is_ws = Some(is_ws);
                    cur_start = byte_idx;
                }
                Some(prev_ws) if prev_ws != is_ws => {
                    append_token_slice(
                        &mut segments,
                        &text[cur_start..byte_idx],
                        prev_ws,
                        style,
                        normalize_ws,
                    );
                    cur_is_ws = Some(is_ws);
                    cur_start = byte_idx;
                }
                Some(_) => {}
            }
        }
        if let Some(prev_ws) = cur_is_ws {
            append_token_slice(
                &mut segments,
                &text[cur_start..],
                prev_ws,
                style,
                normalize_ws,
            );
        }
    }
    segments
}

fn append_token_slice(
    segments: &mut Vec<CellSegment>,
    slice: &str,
    is_ws: bool,
    style: Style,
    normalize_ws: bool,
) {
    if slice.is_empty() {
        return;
    }
    if is_ws {
        let text = if normalize_ws {
            slice.replace(['\t', '\n', '\r'], " ")
        } else {
            slice.to_string()
        };
        let w = str_display_width(&text);
        if let Some(CellSegment::Space(spans, total_w)) = segments.last_mut() {
            push_merged_span(spans, &text, style);
            *total_w += w;
        } else {
            segments.push(CellSegment::Space(vec![Span::styled(text, style)], w));
        }
    } else {
        let w = str_display_width(slice);
        if let Some(CellSegment::Word(spans, total_w)) = segments.last_mut() {
            push_merged_span(spans, slice, style);
            *total_w += w;
        } else {
            segments.push(CellSegment::Word(
                vec![Span::styled(slice.to_string(), style)],
                w,
            ));
        }
    }
}

fn push_wrapped_line(
    out: &mut Vec<Vec<Span<'static>>>,
    tracker: Option<&mut MetaTracker<'_>>,
    spans: Vec<Span<'static>>,
    next_joined_by_space: bool,
) {
    out.push(spans);
    if let Some(t) = tracker {
        t.out_meta.push(t.next_meta);
        t.next_meta = LineMeta {
            soft_wrapped: true,
            joined_by_space: next_joined_by_space,
        };
    }
}

fn push_spans_char_wrapped(
    out: &mut Vec<Vec<Span<'static>>>,
    mut tracker: Option<&mut MetaTracker<'_>>,
    cur_line: &mut Vec<Span<'static>>,
    cur_w: &mut usize,
    spans: Vec<Span<'static>>,
    max_width: usize,
) {
    for sp in spans {
        for ch in sp.content.chars() {
            let cw = char_display_width(ch);
            if *cur_w > 0 && *cur_w + cw > max_width {
                push_wrapped_line(
                    out,
                    tracker.as_deref_mut(),
                    std::mem::take(cur_line),
                    false,
                );
                *cur_w = 0;
            }
            push_merged_char(cur_line, ch, sp.style);
            *cur_w += cw;
        }
    }
}

pub(super) fn wrap_spans_to_width(
    spans: Vec<Span<'static>>,
    col_width: usize,
) -> Vec<Vec<Span<'static>>> {
    let max_w = col_width.max(1);
    let segments = tokenize_spans(spans, true);
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur_line: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    let mut pending_space: Option<(Vec<Span<'static>>, usize)> = None;

    for seg in segments {
        match seg {
            CellSegment::Space(sp_spans, sp_w) => {
                if cur_w > 0 {
                    match &mut pending_space {
                        Some((existing, existing_w)) => {
                            for sp in sp_spans {
                                push_merged_span(existing, sp.content.as_ref(), sp.style);
                            }
                            *existing_w += sp_w;
                        }
                        None => pending_space = Some((sp_spans, sp_w)),
                    }
                }
            }
            CellSegment::Word(word_spans, word_w) => {
                let space_w = if cur_w > 0 {
                    pending_space.as_ref().map_or(0, |(_, w)| *w)
                } else {
                    0
                };

                if cur_w > 0 && cur_w + space_w + word_w <= max_w {
                    if let Some((sp_spans, _)) = pending_space.take() {
                        for sp in sp_spans {
                            push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                        }
                    }
                    for sp in word_spans {
                        push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                    }
                    cur_w += space_w + word_w;
                } else {
                    if cur_w > 0 {
                        lines.push(std::mem::take(&mut cur_line));
                        cur_w = 0;
                    }
                    pending_space = None;

                    if word_w <= max_w {
                        for sp in word_spans {
                            push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                        }
                        cur_w = word_w;
                    } else {
                        push_spans_char_wrapped(
                            &mut lines,
                            None,
                            &mut cur_line,
                            &mut cur_w,
                            word_spans,
                            max_w,
                        );
                    }
                }
            }
        }
    }

    if !cur_line.is_empty() || lines.is_empty() {
        lines.push(cur_line);
    }

    lines
}

pub fn wrap_styled_lines_with_meta(
    lines: Vec<Line<'static>>,
    max_width: usize,
) -> (Vec<Line<'static>>, Vec<LineMeta>) {
    let mut wrapped_lines = Vec::new();
    let mut wrapped_meta = Vec::new();
    for line in lines {
        let (fragments, meta) = wrap_styled_line_with_meta(line, max_width);
        wrapped_lines.extend(fragments);
        wrapped_meta.extend(meta);
    }
    debug_assert_eq!(wrapped_lines.len(), wrapped_meta.len());
    (wrapped_lines, wrapped_meta)
}

pub fn wrap_styled_lines(lines: Vec<Line<'static>>, max_width: usize) -> Vec<Line<'static>> {
    wrap_styled_lines_with_meta(lines, max_width).0
}

pub fn wrap_styled_line(line: Line<'static>, max_width: usize) -> Vec<Line<'static>> {
    wrap_styled_line_with_meta(line, max_width).0
}

pub fn wrap_styled_line_with_meta(
    line: Line<'static>,
    max_width: usize,
) -> (Vec<Line<'static>>, Vec<LineMeta>) {
    if max_width == 0 {
        return (vec![line], vec![LineMeta::default()]);
    }

    let line_style = line.style;
    let line_alignment = line.alignment;
    let segments = tokenize_spans(line.spans, false);
    let mut out: Vec<Vec<Span<'static>>> = Vec::new();
    let mut out_meta: Vec<LineMeta> = Vec::new();
    let mut tracker = MetaTracker {
        out_meta: &mut out_meta,
        next_meta: LineMeta::default(),
    };
    let mut cur_line: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    let mut has_word_on_line = false;
    let mut pending_ws: Option<(Vec<Span<'static>>, usize)> = None;

    for seg in segments {
        match seg {
            CellSegment::Space(sp_spans, sp_w) => {
                if has_word_on_line {
                    match &mut pending_ws {
                        Some((existing, existing_w)) => {
                            for sp in sp_spans {
                                push_merged_span(existing, sp.content.as_ref(), sp.style);
                            }
                            *existing_w += sp_w;
                        }
                        None => pending_ws = Some((sp_spans, sp_w)),
                    }
                } else {
                    push_spans_char_wrapped(
                        &mut out,
                        Some(&mut tracker),
                        &mut cur_line,
                        &mut cur_w,
                        sp_spans,
                        max_width,
                    );
                }
            }
            CellSegment::Word(word_spans, word_w) => {
                let ws_w = if has_word_on_line {
                    pending_ws.as_ref().map_or(0, |(_, w)| *w)
                } else {
                    0
                };

                if cur_w + ws_w + word_w <= max_width {
                    if let Some((sp_spans, _)) = pending_ws.take() {
                        for sp in sp_spans {
                            push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                        }
                    }
                    for sp in word_spans {
                        push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                    }
                    cur_w += ws_w + word_w;
                } else {
                    let broke_on_ws = pending_ws.take().is_some();
                    if cur_w > 0 && (has_word_on_line || word_w <= max_width) {
                        push_wrapped_line(
                            &mut out,
                            Some(&mut tracker),
                            std::mem::take(&mut cur_line),
                            broke_on_ws,
                        );
                        cur_w = 0;
                    }

                    if word_w <= max_width {
                        for sp in word_spans {
                            push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
                        }
                        cur_w = word_w;
                    } else {
                        push_spans_char_wrapped(
                            &mut out,
                            Some(&mut tracker),
                            &mut cur_line,
                            &mut cur_w,
                            word_spans,
                            max_width,
                        );
                    }
                }
                has_word_on_line = true;
            }
        }
    }

    if let Some((sp_spans, sp_w)) = pending_ws.take() {
        if cur_w + sp_w <= max_width {
            for sp in sp_spans {
                push_merged_span(&mut cur_line, sp.content.as_ref(), sp.style);
            }
        }
    }

    if !cur_line.is_empty() || out.is_empty() {
        push_wrapped_line(&mut out, Some(&mut tracker), cur_line, false);
    }

    let lines = out
        .into_iter()
        .map(|spans| {
            let mut l = Line::from(spans).style(line_style);
            l.alignment = line_alignment;
            l
        })
        .collect::<Vec<_>>();
    debug_assert_eq!(lines.len(), out_meta.len());
    (lines, out_meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn test_wrap_styled_lines_with_meta_space_vs_hard_newline() {
        let input = vec![Line::raw("alpha beta gamma"), Line::raw("second line")];
        let (lines, meta) = wrap_styled_lines_with_meta(input.clone(), 11);
        assert_eq!(lines.len(), meta.len());
        assert_eq!(plain(&lines), vec!["alpha beta", "gamma", "second line"]);
        assert_eq!(
            meta,
            vec![
                LineMeta {
                    soft_wrapped: false,
                    joined_by_space: false,
                },
                LineMeta {
                    soft_wrapped: true,
                    joined_by_space: true,
                },
                LineMeta {
                    soft_wrapped: false,
                    joined_by_space: false,
                },
            ]
        );
        assert_eq!(wrap_styled_lines(input, 11), lines);
    }

    #[test]
    fn test_wrap_styled_lines_with_meta_mid_token_url_and_mixed() {
        let url = "https://example.com/very/long/path/token";
        let (lines, meta) = wrap_styled_lines_with_meta(vec![Line::raw(url)], 14);
        assert_eq!(lines.len(), meta.len());
        assert_eq!(
            plain(&lines),
            vec!["https://exampl", "e.com/very/lon", "g/path/token"]
        );
        assert_eq!(
            meta,
            vec![
                LineMeta {
                    soft_wrapped: false,
                    joined_by_space: false,
                },
                LineMeta {
                    soft_wrapped: true,
                    joined_by_space: false,
                },
                LineMeta {
                    soft_wrapped: true,
                    joined_by_space: false,
                },
            ]
        );

        let mixed = format!("See {url}");
        let (mixed_lines, mixed_meta) = wrap_styled_lines_with_meta(vec![Line::raw(mixed)], 14);
        assert_eq!(mixed_lines.len(), mixed_meta.len());
        assert_eq!(
            plain(&mixed_lines),
            vec!["See", "https://exampl", "e.com/very/lon", "g/path/token"]
        );
        assert_eq!(
            mixed_meta,
            vec![
                LineMeta {
                    soft_wrapped: false,
                    joined_by_space: false,
                },
                LineMeta {
                    soft_wrapped: true,
                    joined_by_space: true,
                },
                LineMeta {
                    soft_wrapped: true,
                    joined_by_space: false,
                },
                LineMeta {
                    soft_wrapped: true,
                    joined_by_space: false,
                },
            ]
        );
    }

    #[test]
    fn test_wrap_styled_lines_with_meta_edge_cases_preserve_length_invariant() {
        let (empty_lines, empty_meta) = wrap_styled_lines_with_meta(Vec::new(), 10);
        assert!(empty_lines.is_empty());
        assert!(empty_meta.is_empty());

        let (zero_w, zero_meta) = wrap_styled_lines_with_meta(vec![Line::raw("abc def")], 0);
        assert_eq!(zero_w.len(), zero_meta.len());
        assert_eq!(zero_meta, vec![LineMeta::default()]);

        let (blank, blank_meta) = wrap_styled_lines_with_meta(vec![Line::raw("")], 10);
        assert_eq!(blank.len(), blank_meta.len());
        assert_eq!(blank_meta, vec![LineMeta::default()]);

        let (wide, wide_meta) = wrap_styled_lines_with_meta(vec![Line::raw("🦀🦀🦀🦀")], 5);
        assert_eq!(wide.len(), wide_meta.len());
        assert_eq!(plain(&wide), vec!["🦀🦀", "🦀🦀"]);
        assert_eq!(
            wide_meta,
            vec![
                LineMeta {
                    soft_wrapped: false,
                    joined_by_space: false,
                },
                LineMeta {
                    soft_wrapped: true,
                    joined_by_space: false,
                },
            ]
        );
    }
}
