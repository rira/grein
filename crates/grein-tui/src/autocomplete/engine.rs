use std::borrow::Cow;

use super::dictionary::COMMON_WORDS;

const MAX_MATCHES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SuggestionTier {
    SlashOrWorkspace,
    TranscriptOrHistory,
    DictionaryWord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub full_text: String,
    pub insert_suffix: String,
    pub tier: SuggestionTier,
    pub is_full_line: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutocompleteContext {
    pub slash_commands: Vec<String>,
    pub workspace_files: Vec<String>,
    pub git_branches: Vec<String>,
    pub transcript_tokens: Vec<String>,
    pub recent_prompts: Vec<String>,
    pub history_words: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveCompletion {
    pub token_start: usize,
    pub token_end: usize,
    pub prefix: String,
    pub matches: Vec<Suggestion>,
    pub selected_idx: usize,
}

impl ActiveCompletion {
    #[must_use]
    pub fn active(&self) -> Option<&Suggestion> {
        self.matches.get(self.selected_idx)
    }

    pub fn step_next(&mut self) {
        if !self.matches.is_empty() {
            self.selected_idx = (self.selected_idx + 1) % self.matches.len();
        }
    }

    pub fn step_prev(&mut self) {
        if !self.matches.is_empty() {
            self.selected_idx = if self.selected_idx == 0 {
                self.matches.len() - 1
            } else {
                self.selected_idx - 1
            };
        }
    }
}

fn clamp_char_boundary(input: &str, pos: usize) -> usize {
    let mut idx = pos.min(input.len());
    while idx > 0 && !input.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn is_token_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '@')
}

fn is_worktree_command_args(before_cursor: &str) -> bool {
    if before_cursor.contains('\n') || before_cursor.contains('\r') {
        return false;
    }
    let trimmed = before_cursor.trim_start();
    trimmed
        .get(..4)
        .is_some_and(|p| p.eq_ignore_ascii_case("/wt "))
        || trimmed
            .get(..10)
            .is_some_and(|p| p.eq_ignore_ascii_case("/worktree "))
}

fn strip_prefix_ci<'a>(candidate: &'a str, prefix: &str) -> Option<&'a str> {
    let mut cand_chars = candidate.chars();
    for p_ch in prefix.chars() {
        let c_ch = cand_chars.next()?;
        if !c_ch.to_lowercase().eq(p_ch.to_lowercase()) {
            return None;
        }
    }
    let rem = cand_chars.as_str();
    (!rem.is_empty()).then_some(rem)
}

fn push_unique(matches: &mut Vec<Suggestion>, suggestion: Suggestion) {
    if matches.len() >= MAX_MATCHES || suggestion.insert_suffix.is_empty() {
        return;
    }
    if !matches.iter().any(|m| m.full_text == suggestion.full_text) {
        matches.push(suggestion);
    }
}

fn add_slash_matches(matches: &mut Vec<Suggestion>, prefix: &str, commands: &[String]) {
    let normalized: Vec<Cow<'_, str>> = commands
        .iter()
        .filter_map(|cmd| {
            let trimmed = cmd.trim();
            if trimmed.is_empty() {
                None
            } else if trimmed.starts_with('/') {
                Some(Cow::Borrowed(trimmed))
            } else {
                Some(Cow::Owned(format!("/{trimmed}")))
            }
        })
        .collect();

    for cand in &normalized {
        if let Some(rem) = cand.strip_prefix(prefix) {
            push_unique(
                matches,
                Suggestion {
                    full_text: cand.to_string(),
                    insert_suffix: rem.to_string(),
                    tier: SuggestionTier::SlashOrWorkspace,
                    is_full_line: false,
                },
            );
        }
    }

    for cand in &normalized {
        if let Some(rem) = strip_prefix_ci(cand, prefix) {
            push_unique(
                matches,
                Suggestion {
                    full_text: cand.to_string(),
                    insert_suffix: rem.to_string(),
                    tier: SuggestionTier::SlashOrWorkspace,
                    is_full_line: false,
                },
            );
        }
    }
}

fn add_at_file_matches(matches: &mut Vec<Suggestion>, needle: &str, files: &[String]) {
    for file in files {
        if let Some(rem) = file.strip_prefix(needle) {
            if !rem.is_empty() {
                push_unique(
                    matches,
                    Suggestion {
                        full_text: format!("@{file}"),
                        insert_suffix: rem.to_string(),
                        tier: SuggestionTier::SlashOrWorkspace,
                        is_full_line: false,
                    },
                );
            }
        }
    }

    for file in files {
        if let Some((_, base)) = file.rsplit_once('/') {
            if let Some(rem) = base.strip_prefix(needle) {
                if !rem.is_empty() {
                    push_unique(
                        matches,
                        Suggestion {
                            full_text: format!("@{base}"),
                            insert_suffix: rem.to_string(),
                            tier: SuggestionTier::SlashOrWorkspace,
                            is_full_line: false,
                        },
                    );
                }
            }
        }
    }

    for file in files {
        if let Some(rem) = strip_prefix_ci(file, needle) {
            push_unique(
                matches,
                Suggestion {
                    full_text: format!("@{file}"),
                    insert_suffix: rem.to_string(),
                    tier: SuggestionTier::SlashOrWorkspace,
                    is_full_line: false,
                },
            );
        }
    }

    for file in files {
        if let Some((_, base)) = file.rsplit_once('/') {
            if let Some(rem) = strip_prefix_ci(base, needle) {
                push_unique(
                    matches,
                    Suggestion {
                        full_text: format!("@{base}"),
                        insert_suffix: rem.to_string(),
                        tier: SuggestionTier::SlashOrWorkspace,
                        is_full_line: false,
                    },
                );
            }
        }
    }
}

fn add_workspace_and_branch_matches(
    matches: &mut Vec<Suggestion>,
    prefix: &str,
    files: &[String],
    branches: &[String],
) {
    let push_exact = |matches: &mut Vec<Suggestion>, cand: &str| {
        if let Some(rem) = cand.strip_prefix(prefix) {
            if !rem.is_empty() {
                push_unique(
                    matches,
                    Suggestion {
                        full_text: cand.to_string(),
                        insert_suffix: rem.to_string(),
                        tier: SuggestionTier::SlashOrWorkspace,
                        is_full_line: false,
                    },
                );
            }
        }
    };

    let push_ci = |matches: &mut Vec<Suggestion>, cand: &str| {
        if let Some(rem) = strip_prefix_ci(cand, prefix) {
            push_unique(
                matches,
                Suggestion {
                    full_text: cand.to_string(),
                    insert_suffix: rem.to_string(),
                    tier: SuggestionTier::SlashOrWorkspace,
                    is_full_line: false,
                },
            );
        }
    };

    for file in files {
        push_exact(matches, file);
    }
    for file in files {
        if let Some((_, base)) = file.rsplit_once('/') {
            push_exact(matches, base);
        }
    }
    for branch in branches {
        push_exact(matches, branch);
    }

    for file in files {
        push_ci(matches, file);
    }
    for file in files {
        if let Some((_, base)) = file.rsplit_once('/') {
            push_ci(matches, base);
        }
    }
    for branch in branches {
        push_ci(matches, branch);
    }
}

fn add_tier2_matches(
    matches: &mut Vec<Suggestion>,
    input: &str,
    cursor: usize,
    token_start: usize,
    prefix: &str,
    ctx: &AutocompleteContext,
    is_slash_cmd_pos: bool,
) {
    let prefix_chars = prefix.chars().count();
    if prefix_chars < 2 {
        return;
    }

    if token_start == 0
        && !input[cursor..].contains('\n')
        && input.len() == cursor
        && !prefix.starts_with('/')
    {
        for prompt in &ctx.recent_prompts {
            if prompt.contains('\n')
                || prompt.contains('\r')
                || prompt.trim_start().starts_with('/')
            {
                continue;
            }
            if let Some(rem) = prompt.strip_prefix(input) {
                if !rem.is_empty() {
                    push_unique(
                        matches,
                        Suggestion {
                            full_text: prompt.clone(),
                            insert_suffix: rem.to_string(),
                            tier: SuggestionTier::TranscriptOrHistory,
                            is_full_line: true,
                        },
                    );
                }
            }
        }
    }

    if prefix.starts_with('@') || is_slash_cmd_pos {
        return;
    }

    for cand in ctx.transcript_tokens.iter().chain(ctx.history_words.iter()) {
        if let Some(rem) = cand.strip_prefix(prefix) {
            if !rem.is_empty() {
                push_unique(
                    matches,
                    Suggestion {
                        full_text: cand.clone(),
                        insert_suffix: rem.to_string(),
                        tier: SuggestionTier::TranscriptOrHistory,
                        is_full_line: false,
                    },
                );
            }
        }
    }

    for cand in ctx.transcript_tokens.iter().chain(ctx.history_words.iter()) {
        if let Some(rem) = strip_prefix_ci(cand, prefix) {
            push_unique(
                matches,
                Suggestion {
                    full_text: format!("{prefix}{rem}"),
                    insert_suffix: rem.to_string(),
                    tier: SuggestionTier::TranscriptOrHistory,
                    is_full_line: false,
                },
            );
        }
    }
}

fn add_dictionary_matches(matches: &mut Vec<Suggestion>, prefix: &str) {
    if matches.len() >= MAX_MATCHES
        || prefix.len() < 3
        || !prefix.chars().all(|c| c.is_ascii_alphabetic())
    {
        return;
    }

    let lower_prefix = prefix.to_ascii_lowercase();
    let first_upper = prefix
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_uppercase);
    let all_upper = prefix.bytes().all(|b| b.is_ascii_uppercase());
    let rest_lower = prefix
        .as_bytes()
        .get(1..)
        .is_none_or(|rest| rest.iter().all(u8::is_ascii_lowercase));

    for &word in COMMON_WORDS {
        if let Some(rem) = word.strip_prefix(&lower_prefix) {
            if rem.is_empty() {
                continue;
            }
            let (full_text, insert_suffix) = if all_upper {
                (word.to_ascii_uppercase(), rem.to_ascii_uppercase())
            } else if first_upper && rest_lower {
                let first = word[..1].to_ascii_uppercase();
                (format!("{first}{}", &word[1..]), rem.to_string())
            } else if first_upper {
                (format!("{prefix}{rem}"), rem.to_string())
            } else {
                (word.to_string(), rem.to_string())
            };

            push_unique(
                matches,
                Suggestion {
                    full_text,
                    insert_suffix,
                    tier: SuggestionTier::DictionaryWord,
                    is_full_line: false,
                },
            );

            if matches.len() >= MAX_MATCHES {
                break;
            }
        }
    }
}

#[must_use]
pub fn compute_completions(
    input: &str,
    cursor: usize,
    ctx: &AutocompleteContext,
) -> Option<ActiveCompletion> {
    let cursor = clamp_char_boundary(input, cursor);

    if let Some(next_ch) = input[cursor..].chars().next() {
        if is_token_char(next_ch) {
            return None;
        }
    }

    if is_worktree_command_args(&input[..cursor]) {
        return None;
    }

    let mut token_start = cursor;
    for (idx, ch) in input[..cursor].char_indices().rev() {
        if is_token_char(ch) {
            token_start = idx;
        } else {
            break;
        }
    }

    let prefix = &input[token_start..cursor];
    if prefix.is_empty() {
        return None;
    }

    let at_command_start = input[..token_start].chars().all(|c| c == ' ' || c == '\t');
    let is_slash_cmd_pos = prefix.starts_with('/') && at_command_start;

    let mut matches = Vec::new();

    if is_slash_cmd_pos {
        add_slash_matches(&mut matches, prefix, &ctx.slash_commands);
    } else if let Some(needle) = prefix.strip_prefix('@') {
        if !needle.is_empty() {
            add_at_file_matches(&mut matches, needle, &ctx.workspace_files);
        }
    } else if !prefix.starts_with('/') && prefix.chars().count() >= 2 {
        add_workspace_and_branch_matches(
            &mut matches,
            prefix,
            &ctx.workspace_files,
            &ctx.git_branches,
        );
    }

    add_tier2_matches(
        &mut matches,
        input,
        cursor,
        token_start,
        prefix,
        ctx,
        is_slash_cmd_pos,
    );

    add_dictionary_matches(&mut matches, prefix);

    if matches.is_empty() {
        None
    } else {
        Some(ActiveCompletion {
            token_start,
            token_end: cursor,
            prefix: prefix.to_string(),
            matches,
            selected_idx: 0,
        })
    }
}
