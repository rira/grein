use crate::error::CoreError;

pub mod sqlite;

pub use sqlite::{SqliteDagStorage, SqliteStorage};

impl From<rusqlite::Error> for CoreError {
    fn from(err: rusqlite::Error) -> Self {
        CoreError::Storage(err.to_string())
    }
}
