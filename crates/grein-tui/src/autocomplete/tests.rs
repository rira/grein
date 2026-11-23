use std::collections::HashSet;

use super::dictionary::dictionary_words;
use super::{
    compute_completions, extract_history_words, extract_transcript_identifiers, ActiveCompletion,
    AutocompleteContext, Suggestion, SuggestionTier, COMMON_WORDS,
};

fn default_ctx() -> AutocompleteContext {
    AutocompleteContext {
        slash_commands: vec![
            "/help".into(),
            "theme".into(),
            "/worktree".into(),
            "/worktrees".into(),
            "/new".into(),
            "/fork".into(),
            "/exit".into(),
        ],
        workspace_files: vec![
            "crates/grein-tui/src/lib.rs".into(),
            "crates/grein-tui/src/app/mod.rs".into(),
            "crates/grein-tui/src/keymap.rs".into(),
            "Cargo.toml".into(),
        ],
        git_branches: vec![
            "main".into(),
            "feature/w3-autocomplete".into(),
            "fix/cursor-clamp".into(),
        ],
        transcript_tokens: vec![
            "compute_completions".into(),
            "ActiveCompletion".into(),
            "SuggestionTier".into(),
            "feature_flag".into(),
        ],
        recent_prompts: vec![
            "refactor the autocomplete engine".into(),
            "implement tiered completion".into(),
            "/theme nord".into(),
        ],
        history_words: vec![
            "composer".into(),
            "worktree".into(),
            "ConfigOption".into(),
            "config_map".into(),
        ],
    }
}

#[test]
fn suppresses_mid_word_cursor() {
    let ctx = AutocompleteContext::default();
    assert_eq!(compute_completions("hello", 3, &ctx), None);
    assert_eq!(compute_completions("imple", 2, &ctx), None);
    assert_eq!(compute_completions("crates/grein", 6, &ctx), None);

    let after_space = compute_completions("imple next", 5, &ctx);
    assert!(after_space.is_some());
    let ac = after_space.unwrap();
    assert_eq!(ac.token_start, 0);
    assert_eq!(ac.token_end, 5);
    assert_eq!(ac.prefix, "imple");

    let before_paren = compute_completions("call(imple)", 10, &ctx);
    assert!(before_paren.is_some());
}

#[test]
fn handles_utf8_multibyte_and_invalid_cursor_safely() {
    let ctx = AutocompleteContext {
        transcript_tokens: vec!["åäö_identifier".into()],
        ..Default::default()
    };

    let input = "åäö imple";
    let res = compute_completions(input, input.len(), &ctx).expect("should match after utf8");
    assert_eq!(res.token_start, 7);
    assert_eq!(res.token_end, input.len());
    assert_eq!(res.prefix, "imple");
    assert_eq!(
        res.active().map(|s| s.tier),
        Some(SuggestionTier::DictionaryWord)
    );

    let utf8_tok = compute_completions("åäö_", "åäö_".len(), &ctx).expect("utf8 ident");
    assert_eq!(utf8_tok.token_start, 0);
    assert_eq!(
        utf8_tok.active().map(|s| s.full_text.as_str()),
        Some("åäö_identifier")
    );

    assert_eq!(compute_completions("åäö", 1, &ctx), None);
    let clamped = compute_completions("imple", 999, &ctx).expect("clamps cursor");
    assert_eq!(clamped.token_end, 5);
}

#[test]
fn enforces_tier_thresholds() {
    let ctx = default_ctx();

    let slash_root = compute_completions("/", 1, &ctx).expect("slash at len 1");
    assert_eq!(slash_root.prefix, "/");
    assert!(slash_root
        .matches
        .iter()
        .all(|s| s.tier == SuggestionTier::SlashOrWorkspace && s.full_text.starts_with('/')));
    assert!(slash_root.matches.iter().any(|s| s.full_text == "/theme"));

    let slash_case = compute_completions(" /TH", 4, &ctx).expect("slash case-insensitive");
    assert_eq!(
        slash_case.active().map(|s| s.full_text.as_str()),
        Some("/theme")
    );

    assert_eq!(compute_completions("see /th", 7, &ctx), None);

    assert_eq!(compute_completions("@", 1, &ctx), None);
    let at_one = compute_completions("@c", 2, &ctx).expect("@ requires >= 1 char after @");
    assert_eq!(at_one.prefix, "@c");
    assert_eq!(
        at_one.active().map(|s| s.full_text.as_str()),
        Some("@crates/grein-tui/src/lib.rs")
    );
    assert_eq!(
        at_one.active().map(|s| s.insert_suffix.as_str()),
        Some("rates/grein-tui/src/lib.rs")
    );

    let at_base = compute_completions("@key", 4, &ctx).expect("@ basename");
    assert_eq!(
        at_base.active().map(|s| s.full_text.as_str()),
        Some("@keymap.rs")
    );
    assert_eq!(
        at_base.active().map(|s| s.insert_suffix.as_str()),
        Some("map.rs")
    );

    let empty_ctx = AutocompleteContext::default();
    assert_eq!(compute_completions("im", 2, &empty_ctx), None);
    assert!(compute_completions("imp", 3, &empty_ctx).is_some());

    assert_eq!(compute_completions("k", 1, &ctx), None);
    let tier1_two = compute_completions("ke", 2, &ctx).expect("tier 1 >= 2");
    assert_eq!(
        tier1_two.active().map(|s| s.tier),
        Some(SuggestionTier::SlashOrWorkspace)
    );

    let tier2_two = compute_completions("co", 2, &ctx).expect("tier 2 >= 2");
    assert!(tier2_two
        .matches
        .iter()
        .all(|s| s.tier == SuggestionTier::TranscriptOrHistory));
}

#[test]
fn suppresses_worktree_command_arguments() {
    let ctx = default_ctx();
    let cmd_match = compute_completions("/wor", 4, &ctx).expect("slash command /worktree");
    assert_eq!(
        cmd_match.active().map(|s| s.full_text.as_str()),
        Some("/worktree")
    );

    assert_eq!(compute_completions("/worktree ", 10, &ctx), None);
    assert_eq!(compute_completions("/worktree fe", 12, &ctx), None);
    assert_eq!(compute_completions("/wt fe", 6, &ctx), None);
}

#[test]
fn respects_tier_precedence_and_deduplicates_highest_tier() {
    let ctx = AutocompleteContext {
        workspace_files: vec!["src/implementation.rs".into(), "implementation".into()],
        git_branches: vec!["implement-branch".into()],
        transcript_tokens: vec!["implementation".into(), "implement_hook".into()],
        history_words: vec!["implementation".into(), "importance".into()],
        ..Default::default()
    };

    let res = compute_completions("imp", 3, &ctx).expect("matches across all tiers");
    assert!(res.matches.len() <= 12);

    assert_eq!(res.matches[0].full_text, "implementation");
    assert_eq!(res.matches[0].tier, SuggestionTier::SlashOrWorkspace);

    let imp_count = res
        .matches
        .iter()
        .filter(|m| m.full_text == "implementation")
        .count();
    assert_eq!(imp_count, 1);

    let tiers: Vec<SuggestionTier> = res.matches.iter().map(|m| m.tier).collect();
    let mut sorted_tiers = tiers.clone();
    sorted_tiers.sort();
    assert_eq!(tiers, sorted_tiers);

    assert!(tiers.contains(&SuggestionTier::SlashOrWorkspace));
    assert!(tiers.contains(&SuggestionTier::TranscriptOrHistory));
    assert!(tiers.contains(&SuggestionTier::DictionaryWord));

    let exact_ctx = default_ctx();
    assert_eq!(
        compute_completions("Cargo.toml", 10, &exact_ctx),
        None,
        "exact candidate match should not suggest empty suffix"
    );
}

#[test]
fn col0_full_line_recent_prompt_match() {
    let ctx = default_ctx();
    let res = compute_completions("ref", 3, &ctx).expect("full-line recent prompt match");
    let first = res.active().expect("has active");
    assert!(first.is_full_line);
    assert_eq!(first.tier, SuggestionTier::TranscriptOrHistory);
    assert_eq!(first.full_text, "refactor the autocomplete engine");
    assert_eq!(first.insert_suffix, "actor the autocomplete engine");
    assert_eq!(res.token_start, 0);
    assert_eq!(res.token_end, 3);

    let non_col0 = compute_completions("please ref", 10, &ctx).expect("word completion");
    assert!(non_col0.matches.iter().all(|s| !s.is_full_line));

    let with_newline = compute_completions("ref\nmore", 3, &ctx).expect("word before newline");
    assert!(with_newline.matches.iter().all(|s| !s.is_full_line));

    let slash_prompt = compute_completions("/th", 3, &ctx).expect("slash command");
    assert!(slash_prompt.matches.iter().all(|s| !s.is_full_line));
}

#[test]
fn preserves_capitalized_and_typed_casing() {
    let ctx = default_ctx();

    let cap = compute_completions("Imple", 5, &ctx).expect("capitalized dictionary match");
    assert_eq!(
        cap.active()
            .map(|s| (s.full_text.as_str(), s.insert_suffix.as_str())),
        Some(("Implement", "ment"))
    );
    assert!(cap
        .matches
        .iter()
        .any(|s| s.full_text == "Implementation" && s.insert_suffix == "mentation"));

    let all_upper = compute_completions("IMPLE", 5, &ctx).expect("uppercase dictionary match");
    assert!(all_upper
        .matches
        .iter()
        .any(|s| s.full_text == "IMPLEMENTATION" && s.insert_suffix == "MENTATION"));

    let tier2 = compute_completions("con", 3, &ctx).expect("tier 2 casing");
    assert_eq!(tier2.matches[0].full_text, "config_map");
    assert_eq!(tier2.matches[1].full_text, "configOption");
    assert_eq!(tier2.matches[1].insert_suffix, "figOption");
}

#[test]
fn steps_next_and_prev_with_wrapping() {
    let mut ac = compute_completions("imp", 3, &AutocompleteContext::default()).expect("matches");
    let count = ac.matches.len();
    assert!(count >= 2);
    assert_eq!(ac.selected_idx, 0);

    ac.step_prev();
    assert_eq!(ac.selected_idx, count - 1);
    assert_eq!(ac.active(), ac.matches.last());

    ac.step_next();
    assert_eq!(ac.selected_idx, 0);
    assert_eq!(ac.active(), ac.matches.first());

    let mut empty = ActiveCompletion {
        token_start: 0,
        token_end: 0,
        prefix: String::new(),
        matches: Vec::<Suggestion>::new(),
        selected_idx: 0,
    };
    empty.step_next();
    empty.step_prev();
    assert_eq!(empty.active(), None);
}

#[test]
fn extracts_transcript_identifiers_with_priority() {
    let texts = [
        "Please check `crates/grein-tui/src/lib.rs` and computeCompletions(foo_bar). Therefore skip plain words.",
        "Also update kebab-case, crate::autocomplete::SuggestionTier, and computeCompletions!",
        "Recent addition: newest_symbol_name in Cargo.toml (12345 ignored).",
    ];

    let tokens = extract_transcript_identifiers(&texts, 20);
    assert!(tokens.contains(&"crates/grein-tui/src/lib.rs".to_string()));
    assert!(tokens.contains(&"computeCompletions".to_string()));
    assert!(tokens.contains(&"foo_bar".to_string()));
    assert!(tokens.contains(&"kebab-case".to_string()));
    assert!(tokens.contains(&"crate::autocomplete::SuggestionTier".to_string()));
    assert!(tokens.contains(&"newest_symbol_name".to_string()));
    assert!(tokens.contains(&"Cargo.toml".to_string()));
    assert!(!tokens.contains(&"Therefore".to_string()));
    assert!(!tokens.contains(&"plain".to_string()));
    assert!(!tokens.contains(&"12345".to_string()));

    assert_eq!(tokens[0], "computeCompletions");

    let capped = extract_transcript_identifiers(&texts, 2);
    assert_eq!(capped.len(), 2);
    assert!(extract_transcript_identifiers(&texts, 0).is_empty());
}

#[test]
fn extracts_history_words_with_recency_and_frequency() {
    let prompts = vec![
        "fix authentication bug in 42".to_string(),
        "refactor authentication module".to_string(),
        "check composer rendering".to_string(),
    ];

    let words = extract_history_words(&prompts, 10);
    assert_eq!(words[0], "authentication");
    assert!(words.contains(&"composer".to_string()));
    assert!(words.contains(&"rendering".to_string()));
    assert!(!words.contains(&"in".to_string()));
    assert!(!words.contains(&"42".to_string()));
}

#[test]
fn dictionary_invariants_and_required_vocabulary() {
    assert!(COMMON_WORDS.len() >= 3000);
    assert_eq!(dictionary_words().count(), COMMON_WORDS.len());

    let mut seen = HashSet::with_capacity(COMMON_WORDS.len());
    for &word in COMMON_WORDS {
        assert!(word.len() >= 4, "word too short: {word}");
        assert!(
            word.bytes().all(|b| b.is_ascii_lowercase()),
            "invalid dictionary entry: {word}"
        );
        assert!(seen.insert(word), "duplicate dictionary entry: {word}");
    }

    for required in [
        "implementation",
        "architecture",
        "component",
        "configuration",
        "function",
        "interface",
        "repository",
        "worktree",
        "terminal",
        "refactor",
        "authentication",
        "serialization",
    ] {
        assert!(seen.contains(required), "missing required word: {required}");
    }
}
