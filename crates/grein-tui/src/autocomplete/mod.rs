pub mod context;
pub mod dictionary;
pub mod engine;

#[cfg(test)]
mod tests;

pub use context::{extract_history_words, extract_transcript_identifiers};
pub use dictionary::COMMON_WORDS;
pub use engine::{
    compute_completions, ActiveCompletion, AutocompleteContext, Suggestion, SuggestionTier,
};
