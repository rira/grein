mod json;
mod store;
mod toml;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::error::{CoreError, Result};
use crate::port::outbound::SettingsCodec;

pub use json::JsonSettingsCodec;
pub use store::{default_prefs_db_path, resolve_prefs_db_path_from_env, UserPrefsStore};
pub use toml::TomlSettingsCodec;

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

pub fn codec_for(format: SettingsFormat) -> &'static dyn SettingsCodec {
    static TOML: TomlSettingsCodec = TomlSettingsCodec;
    static JSON: JsonSettingsCodec = JsonSettingsCodec;
    match format {
        SettingsFormat::Toml => &TOML,
        SettingsFormat::Json => &JSON,
    }
}

pub(super) fn normalize_section_name(section: &str) -> Result<&str> {
    let trimmed = section.trim();
    if trimmed.is_empty() {
        return Err(CoreError::SyncError(
            "section name cannot be empty".to_string(),
        ));
    }
    Ok(trimmed)
}
