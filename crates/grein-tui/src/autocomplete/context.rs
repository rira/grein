use std::collections::HashMap;

struct TokenStats {
    count: usize,
    last_group_idx: usize,
    last_seq: usize,
}

fn is_double_colon_char(text: &str, idx: usize, prev_ch: Option<char>) -> bool {
    prev_ch == Some(':')
        || text
            .get(idx + 1..)
            .is_some_and(|rest| rest.starts_with(':'))
}

fn trim_punctuation_edges(token: &str) -> &str {
    token.trim_matches(|c: char| !c.is_alphanumeric())
}

fn scan_raw_tokens(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    let mut prev_ch: Option<char> = None;

    for (idx, ch) in text.char_indices() {
        let valid = ch.is_alphanumeric()
            || matches!(ch, '_' | '-' | '/' | '.')
            || (ch == ':' && is_double_colon_char(text, idx, prev_ch));

        if valid {
            if start.is_none() {
                start = Some(idx);
            }
        } else if let Some(s) = start.take() {
            let trimmed = trim_punctuation_edges(&text[s..idx]);
            if !trimmed.is_empty() {
                tokens.push(trimmed);
            }
        }
        prev_ch = Some(ch);
    }

    if let Some(s) = start {
        let trimmed = trim_punctuation_edges(&text[s..]);
        if !trimmed.is_empty() {
            tokens.push(trimmed);
        }
    }

    tokens
}

fn is_identifier_like(token: &str) -> bool {
    if token.contains('_')
        || token.contains('-')
        || token.contains('/')
        || token.contains('.')
        || token.contains("::")
    {
        return true;
    }

    if token.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }

    let has_lower = token.chars().any(char::is_lowercase);
    let has_non_leading_upper = token.chars().skip(1).any(char::is_uppercase);
    has_lower && has_non_leading_upper
}

fn rank_and_collect<'a>(
    counts: HashMap<&'a str, TokenStats>,
    total_groups: usize,
    max_tokens: usize,
) -> Vec<String> {
    let denom = total_groups.max(1);
    let mut entries: Vec<(&'a str, TokenStats)> = counts.into_iter().collect();

    entries.sort_by(|(tok_a, a), (tok_b, b)| {
        let score_a = a
            .count
            .saturating_mul(4)
            .saturating_add(((a.last_group_idx + 1) * 8) / denom);
        let score_b = b
            .count
            .saturating_mul(4)
            .saturating_add(((b.last_group_idx + 1) * 8) / denom);

        score_b
            .cmp(&score_a)
            .then_with(|| b.last_group_idx.cmp(&a.last_group_idx))
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| b.last_seq.cmp(&a.last_seq))
            .then_with(|| tok_a.cmp(tok_b))
    });

    entries.truncate(max_tokens);
    entries
        .into_iter()
        .map(|(tok, _)| tok.to_string())
        .collect()
}

fn collect_tokens<'a, I>(groups: I, max_tokens: usize, identifiers_only: bool) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    if max_tokens == 0 {
        return Vec::new();
    }

    let mut counts: HashMap<&'a str, TokenStats> = HashMap::new();
    let mut seq = 0usize;
    let mut total_groups = 0usize;

    for (group_idx, text) in groups.into_iter().enumerate() {
        total_groups = group_idx + 1;
        for token in scan_raw_tokens(text) {
            if token.len() > 128 || token.chars().count() < 3 {
                continue;
            }
            if !token.chars().any(char::is_alphabetic) {
                continue;
            }
            if identifiers_only && !is_identifier_like(token) {
                continue;
            }

            let entry = counts.entry(token).or_insert(TokenStats {
                count: 0,
                last_group_idx: group_idx,
                last_seq: seq,
            });
            entry.count += 1;
            entry.last_group_idx = group_idx;
            entry.last_seq = seq;
            seq += 1;
        }
    }

    if counts.is_empty() {
        return Vec::new();
    }

    rank_and_collect(counts, total_groups, max_tokens)
}

#[must_use]
pub fn extract_transcript_identifiers(texts: &[&str], max_tokens: usize) -> Vec<String> {
    collect_tokens(texts.iter().copied(), max_tokens, true)
}

#[must_use]
pub fn extract_history_words(prompts: &[String], max_tokens: usize) -> Vec<String> {
    collect_tokens(prompts.iter().map(String::as_str), max_tokens, false)
}
