//! grein-core: Conversation DAG engine, SQLite persistence, and anchor parser.

pub mod anchor;
pub mod blackboard;
pub mod dag;
pub mod error;
pub mod model;
pub mod prefs;
pub mod storage;
pub mod sync;

pub use blackboard::{
    Blackboard, BlackboardCategory, BlackboardEntry, BlackboardEntryId, BlackboardEntryKind,
    BoundedScratchpad, DecisionLandmark, KnowledgeImport,
};
pub use error::CoreError;
pub use model::ThreadKind;
pub use prefs::{
    default_prefs_db_path, ImportReport, SectionPatch, SettingsFormat, SettingsPatch,
    SettingsSnapshot, UserPrefsStore,
};
pub use sync::{build_context_prompt, ContextCompiler, ContextPayload};
