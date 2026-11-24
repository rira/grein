use super::{
    normalize_section_name, SectionPatch, SettingsFormat, SettingsPatch, SettingsSnapshot,
    SETTINGS_VERSION, THEME_NAMESPACE,
};
use crate::error::CoreError;
use crate::port::outbound::SettingsCodec;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default)]
pub struct JsonSettingsCodec;

impl SettingsCodec for JsonSettingsCodec {
    fn format(&self) -> SettingsFormat {
        SettingsFormat::Json
    }

    fn format_name(&self) -> &'static str {
        "json"
    }

    fn encode(&self, snapshot: &SettingsSnapshot) -> crate::Result<String> {
        format_json_snapshot(snapshot)
    }

    fn encode_section(
        &self,
        section: &str,
        entries: &BTreeMap<String, String>,
    ) -> crate::Result<String> {
        let section = normalize_section_name(section)?;
        let mut preferences = BTreeMap::new();
        preferences.insert(section.to_string(), entries.clone());
        format_json_snapshot(&SettingsSnapshot {
            version: SETTINGS_VERSION,
            preferences,
        })
    }

    fn decode(&self, raw: &str) -> crate::Result<SettingsPatch> {
        parse_json_settings(raw)
    }

    fn decode_theme(&self, raw: &str) -> crate::Result<SettingsPatch> {
        parse_json_theme(raw)
    }
}

pub(super) fn format_json_snapshot(snapshot: &SettingsSnapshot) -> Result<String, CoreError> {
    let mut json = serde_json::to_string_pretty(snapshot)?;
    json.push('\n');
    Ok(json)
}

pub(super) fn parse_json_settings(raw: &str) -> Result<SettingsPatch, CoreError> {
    let root = parse_json_root(raw)?;
    parse_json_settings_root(&root)
}

pub(super) fn parse_json_theme(raw: &str) -> Result<SettingsPatch, CoreError> {
    let root = parse_json_root(raw)?;
    parse_json_theme_root(&root)
}

fn parse_json_root(raw: &str) -> Result<serde_json::Map<String, Value>, CoreError> {
    let value: Value = serde_json::from_str(raw)?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(CoreError::SyncError(
            "expected top-level JSON object".to_string(),
        )),
    }
}

fn parse_json_u32(val: &Value) -> Result<u32, CoreError> {
    val.as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| {
            CoreError::SyncError(
                "invalid settings 'version': expected unsigned integer".to_string(),
            )
        })
}

fn parse_json_scalar_or_null(
    section: &str,
    key: &str,
    val: &Value,
) -> Result<Option<String>, CoreError> {
    match val {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        Value::Bool(b) => Ok(Some(b.to_string())),
        Value::Number(n) => Ok(Some(n.to_string())),
        Value::Array(_) | Value::Object(_) => Err(CoreError::SyncError(format!(
            "invalid value for '{section}.{key}': nested arrays and objects are not supported"
        ))),
    }
}

fn parse_json_section_patch(section: &str, val: &Value) -> Result<SectionPatch, CoreError> {
    if section.trim().is_empty() {
        return Err(CoreError::SyncError(
            "section name cannot be empty".to_string(),
        ));
    }
    match val {
        Value::Null => Ok(SectionPatch::ResetToDefault),
        Value::Object(obj) => {
            let mut entries = BTreeMap::new();
            for (key, item) in obj {
                if key.trim().is_empty() {
                    return Err(CoreError::SyncError(format!(
                        "key cannot be empty in section '{section}'"
                    )));
                }
                entries.insert(key.clone(), parse_json_scalar_or_null(section, key, item)?);
            }
            Ok(SectionPatch::Entries(entries))
        }
        _ => Err(CoreError::SyncError(format!(
            "invalid section '{section}': expected object or null"
        ))),
    }
}

fn parse_json_settings_root(
    root: &serde_json::Map<String, Value>,
) -> Result<SettingsPatch, CoreError> {
    if let Some(prefs_val) = root.get("preferences") {
        for key in root.keys() {
            if key != "preferences" && key != "version" {
                return Err(CoreError::SyncError(format!(
                    "unexpected top-level key '{key}' alongside 'preferences'"
                )));
            }
        }
        let version = root
            .get("version")
            .map(parse_json_u32)
            .transpose()?
            .unwrap_or(SETTINGS_VERSION);
        let prefs_obj = prefs_val.as_object().ok_or_else(|| {
            CoreError::SyncError("'preferences' must be a JSON object".to_string())
        })?;
        let mut preferences = BTreeMap::new();
        for (section, val) in prefs_obj {
            preferences.insert(section.clone(), parse_json_section_patch(section, val)?);
        }
        return Ok(SettingsPatch {
            version,
            preferences,
        });
    }

    let mut version = SETTINGS_VERSION;
    let mut preferences = BTreeMap::new();
    for (key, val) in root {
        if key == "version" {
            version = parse_json_u32(val)?;
            continue;
        }
        preferences.insert(key.clone(), parse_json_section_patch(key, val)?);
    }
    Ok(SettingsPatch {
        version,
        preferences,
    })
}

fn parse_json_theme_root(
    root: &serde_json::Map<String, Value>,
) -> Result<SettingsPatch, CoreError> {
    if root.contains_key("preferences") {
        let full = parse_json_settings_root(root)?;
        let theme_patch = full
            .preferences
            .get(THEME_NAMESPACE)
            .cloned()
            .ok_or_else(|| {
                CoreError::SyncError("settings JSON does not contain a 'theme' section".to_string())
            })?;
        return Ok(SettingsPatch {
            version: full.version,
            preferences: BTreeMap::from([(THEME_NAMESPACE.to_string(), theme_patch)]),
        });
    }

    let version = root
        .get("version")
        .map(parse_json_u32)
        .transpose()?
        .unwrap_or(SETTINGS_VERSION);

    if let Some(theme_val) = root.get(THEME_NAMESPACE) {
        if matches!(theme_val, Value::Object(_) | Value::Null) {
            let patch = parse_json_section_patch(THEME_NAMESPACE, theme_val)?;
            return Ok(SettingsPatch {
                version,
                preferences: BTreeMap::from([(THEME_NAMESPACE.to_string(), patch)]),
            });
        }
    }

    let mut entries = BTreeMap::new();
    for (key, val) in root {
        if key == "version" {
            continue;
        }
        if matches!(val, Value::Object(_)) {
            return Err(CoreError::SyncError(
                "settings JSON does not contain a 'theme' section".to_string(),
            ));
        }
        if key.trim().is_empty() {
            return Err(CoreError::SyncError(
                "theme key cannot be empty".to_string(),
            ));
        }
        entries.insert(
            key.clone(),
            parse_json_scalar_or_null(THEME_NAMESPACE, key, val)?,
        );
    }

    if entries.is_empty() {
        return Err(CoreError::SyncError(
            "theme JSON contains neither a 'theme' section nor theme keys".to_string(),
        ));
    }

    Ok(SettingsPatch {
        version,
        preferences: BTreeMap::from([(
            THEME_NAMESPACE.to_string(),
            SectionPatch::Entries(entries),
        )]),
    })
}
