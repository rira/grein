//! grein-core: Conversation DAG engine, SQLite persistence, and anchor parser.

pub mod anchor;
pub mod dag;
pub mod error;
pub mod model;
pub mod storage;

pub use error::CoreError;
