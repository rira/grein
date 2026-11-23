mod json;
mod store;
mod toml;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use store::{default_prefs_db_path, resolve_prefs_db_path_from_env, UserPrefsStore};

pub(super) const THEME_NAMESPACE: &str = "theme";
pub(super) const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsFormat {
    Toml,
    Json,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsSnapshot {
    pub version: u32,
    pub preferences: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionPatch {
    ResetToDefault,
    Entries(BTreeMap<String, Option<String>>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsPatch {
    pub version: u32,
    pub preferences: BTreeMap<String, SectionPatch>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub updated: usize,
    pub reset_keys: usize,
    pub reset_sections: usize,
}
