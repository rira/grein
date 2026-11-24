use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

use grein_core::port::outbound::{PreferenceRepository, SettingsCodec};
use grein_core::prefs::codec_for;
use grein_core::{
    default_prefs_db_path, JsonSettingsCodec, SettingsFormat, TomlSettingsCodec, UserPrefsStore,
};
use grein_tui::theme::ThemePalette;

use crate::args::{
    InputFormatArg, OutputFormatArg, SettingsArgs, SettingsCommand, ThemeArgs, ThemeCommand,
};

pub struct SettingsDeps {
    pub read_repo: Arc<dyn PreferenceRepository>,
    pub write_repo: Option<Arc<dyn PreferenceRepository>>,
    pub toml_codec: Arc<dyn SettingsCodec>,
    pub json_codec: Arc<dyn SettingsCodec>,
}

impl SettingsDeps {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let path_opt = default_prefs_db_path();
        let read_repo: Arc<dyn PreferenceRepository> = match path_opt.as_ref() {
            Some(path) if path.exists() => Arc::new(UserPrefsStore::open(path)?),
            _ => Arc::new(UserPrefsStore::in_memory()?),
        };
        let write_repo: Option<Arc<dyn PreferenceRepository>> = match path_opt.as_ref() {
            Some(path) => Some(Arc::new(UserPrefsStore::open(path)?)),
            None => None,
        };
        Ok(Self {
            read_repo,
            write_repo,
            toml_codec: Arc::new(TomlSettingsCodec),
            json_codec: Arc::new(JsonSettingsCodec),
        })
    }

    pub fn from_read_only_env() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let path_opt = default_prefs_db_path();
        let read_repo: Arc<dyn PreferenceRepository> = match path_opt.as_ref() {
            Some(path) if path.exists() => Arc::new(UserPrefsStore::open(path)?),
            _ => Arc::new(UserPrefsStore::in_memory()?),
        };
        Ok(Self {
            read_repo,
            write_repo: None,
            toml_codec: Arc::new(TomlSettingsCodec),
            json_codec: Arc::new(JsonSettingsCodec),
        })
    }

    fn codec(&self, format: SettingsFormat) -> &dyn SettingsCodec {
        match format {
            SettingsFormat::Toml => self.toml_codec.as_ref(),
            SettingsFormat::Json => self.json_codec.as_ref(),
        }
    }
}

fn resolve_export_format(explicit: Option<OutputFormatArg>, file: Option<&Path>) -> SettingsFormat {
    if let Some(fmt) = explicit {
        return match fmt {
            OutputFormatArg::Toml => SettingsFormat::Toml,
            OutputFormatArg::Json => SettingsFormat::Json,
        };
    }
    if let Some(path) = file.filter(|p| *p != Path::new("-")) {
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            return SettingsFormat::Json;
        }
    }
    SettingsFormat::Toml
}

fn resolve_import_format(explicit: InputFormatArg) -> Option<SettingsFormat> {
    match explicit {
        InputFormatArg::Auto => None,
        InputFormatArg::Toml => Some(SettingsFormat::Toml),
        InputFormatArg::Json => Some(SettingsFormat::Json),
    }
}

fn normalize_sections(raw: &[String]) -> Option<Vec<String>> {
    let items: Vec<String> = raw
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

fn require_write_repo(
    deps: &SettingsDeps,
) -> Result<&dyn PreferenceRepository, Box<dyn std::error::Error + Send + Sync>> {
    deps.write_repo
        .as_deref()
        .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
            Box::new(io::Error::new(
                io::ErrorKind::NotFound,
                "preferences database path is unavailable; set GREIN_CONFIG_DB, XDG_CONFIG_HOME, or HOME",
            ))
        })
}

fn read_input(file: &Path) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if file == Path::new("-") {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        Ok(buf)
    } else {
        Ok(fs::read_to_string(file)?)
    }
}

fn write_output(
    file: Option<&Path>,
    content: &str,
    label: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match file {
        None => {
            print!("{content}");
            Ok(())
        }
        Some(path) if path == Path::new("-") => {
            print!("{content}");
            Ok(())
        }
        Some(path) => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent)?;
                }
            }
            fs::write(path, content)?;
            println!("Exported {label} to {}", path.display());
            Ok(())
        }
    }
}

pub fn handle_settings_with(
    args: &SettingsArgs,
    deps: &SettingsDeps,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match &args.command {
        SettingsCommand::Export(export_args) => {
            let fmt = resolve_export_format(export_args.format, export_args.file.as_deref());
            let only = normalize_sections(&export_args.only);
            let snapshot = deps.read_repo.export_snapshot(only.as_deref())?;
            let output = deps.codec(fmt).encode(&snapshot)?;
            write_output(export_args.file.as_deref(), &output, "settings")?;
            Ok(())
        }
        SettingsCommand::Import(import_args) => {
            let fmt = resolve_import_format(import_args.format);
            let raw = read_input(&import_args.file)?;
            let store = require_write_repo(deps)?;
            let only = normalize_sections(&import_args.only);
            let report = store.import_str(&raw, fmt, only.as_deref(), import_args.replace)?;
            println!(
                "Imported settings (updated: {}, reset_keys: {}, reset_sections: {})",
                report.updated, report.reset_keys, report.reset_sections
            );
            Ok(())
        }
    }
}

pub fn handle_settings(args: &SettingsArgs) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let deps = match &args.command {
        SettingsCommand::Export(_) => SettingsDeps::from_read_only_env()?,
        SettingsCommand::Import(_) => SettingsDeps::from_env()?,
    };
    handle_settings_with(args, &deps)
}

pub fn export_theme_string_with(
    store: &dyn PreferenceRepository,
    codec: &dyn SettingsCodec,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let theme_name = store.theme()?;
    let overrides = store.theme_prefs()?;
    let preset = ThemePalette::resolve_preset(theme_name.as_deref(), &overrides);
    let palette = ThemePalette::from_prefs(theme_name.as_deref(), &overrides);
    let map = palette.to_color_map(preset.as_str());
    Ok(codec.encode_section("theme", &map)?)
}

pub fn export_theme_string(
    store: &dyn PreferenceRepository,
    fmt: SettingsFormat,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    export_theme_string_with(store, codec_for(fmt))
}

pub fn handle_theme_with(
    args: &ThemeArgs,
    deps: &SettingsDeps,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match &args.command {
        ThemeCommand::Export(export_args) => {
            let fmt = resolve_export_format(export_args.format, export_args.file.as_deref());
            let output = export_theme_string_with(deps.read_repo.as_ref(), deps.codec(fmt))?;
            write_output(export_args.file.as_deref(), &output, "theme")?;
            Ok(())
        }
        ThemeCommand::Import(import_args) => {
            let fmt = resolve_import_format(import_args.format);
            let raw = read_input(&import_args.file)?;
            let store = require_write_repo(deps)?;
            let report = store.import_theme_str(&raw, fmt, import_args.replace)?;
            println!(
                "Imported theme (updated: {}, reset_keys: {}, reset_sections: {})",
                report.updated, report.reset_keys, report.reset_sections
            );
            Ok(())
        }
    }
}

pub fn handle_theme(args: &ThemeArgs) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let deps = match &args.command {
        ThemeCommand::Export(_) => SettingsDeps::from_read_only_env()?,
        ThemeCommand::Import(_) => SettingsDeps::from_env()?,
    };
    handle_theme_with(args, &deps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_export_format_autodetects_extension_and_respects_override() {
        assert_eq!(resolve_export_format(None, None), SettingsFormat::Toml);
        assert_eq!(
            resolve_export_format(None, Some(Path::new("-"))),
            SettingsFormat::Toml
        );
        assert_eq!(
            resolve_export_format(None, Some(Path::new("settings.json"))),
            SettingsFormat::Json
        );
        assert_eq!(
            resolve_export_format(None, Some(Path::new("THEME.JSON"))),
            SettingsFormat::Json
        );
        assert_eq!(
            resolve_export_format(Some(OutputFormatArg::Toml), Some(Path::new("settings.json"))),
            SettingsFormat::Toml
        );
        assert_eq!(
            resolve_export_format(Some(OutputFormatArg::Json), None),
            SettingsFormat::Json
        );
    }

    #[test]
    fn test_export_theme_string_resolves_preset_and_custom_hex() {
        let store = UserPrefsStore::in_memory().expect("in_memory");
        store.set_theme("nord").expect("set theme");
        store
            .set_theme_pref("accent", "#ff00aa")
            .expect("set accent");

        let toml_out = export_theme_string(&store, SettingsFormat::Toml).expect("export toml");
        assert!(toml_out.contains("[theme]"));
        assert!(toml_out.contains("name = \"nord\""));
        assert!(toml_out.contains("accent = \"#ff00aa\""));

        let json_out = export_theme_string(&store, SettingsFormat::Json).expect("export json");
        let val: serde_json::Value = serde_json::from_str(&json_out).expect("valid json");
        assert_eq!(val["preferences"]["theme"]["name"], "nord");
        assert_eq!(val["preferences"]["theme"]["accent"], "#ff00aa");
    }
}
