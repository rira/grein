//! grein-core: Conversation DAG engine, SQLite persistence, and anchor parser.

pub mod anchor;
pub mod blackboard;
pub mod dag;
pub mod error;
pub mod model;
pub mod storage;
pub mod sync;

pub use blackboard::{
    Blackboard, BlackboardCategory, BlackboardEntry, BlackboardEntryId, BlackboardEntryKind,
    BoundedScratchpad, DecisionLandmark, KnowledgeImport,
};
pub use error::CoreError;
pub use sync::{
    build_context_prompt, build_context_prompt_with_blackboard, ContextCompiler, ContextPayload,
};
