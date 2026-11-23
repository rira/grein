use std::fs;
use std::io::{self, Read};
use std::path::Path;

use grein_core::{default_prefs_db_path, SettingsFormat, UserPrefsStore};
use grein_tui::theme::ThemePalette;

use crate::args::{
    InputFormatArg, OutputFormatArg, SettingsArgs, SettingsCommand, ThemeArgs, ThemeCommand,
};

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

fn open_store_for_read() -> Result<UserPrefsStore, Box<dyn std::error::Error + Send + Sync>> {
    match default_prefs_db_path() {
        Some(path) if path.exists() => Ok(UserPrefsStore::open(&path)?),
        _ => Ok(UserPrefsStore::in_memory()?),
    }
}

fn open_store_for_write() -> Result<UserPrefsStore, Box<dyn std::error::Error + Send + Sync>> {
    match UserPrefsStore::open_default()? {
        Some(store) => Ok(store),
        None => Err(Box::new(io::Error::new(
            io::ErrorKind::NotFound,
            "preferences database path is unavailable; set GREIN_CONFIG_DB, XDG_CONFIG_HOME, or HOME",
        ))),
    }
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

pub fn handle_settings(args: &SettingsArgs) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match &args.command {
        SettingsCommand::Export(export_args) => {
            let fmt = resolve_export_format(export_args.format, export_args.file.as_deref());
            let store = open_store_for_read()?;
            let only = normalize_sections(&export_args.only);
            let output = store.export_string(fmt, only.as_deref())?;
            write_output(export_args.file.as_deref(), &output, "settings")?;
            Ok(())
        }
        SettingsCommand::Import(import_args) => {
            let fmt = resolve_import_format(import_args.format);
            let raw = read_input(&import_args.file)?;
            let store = open_store_for_write()?;
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

pub fn export_theme_string(
    store: &UserPrefsStore,
    fmt: SettingsFormat,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let theme_name = store.theme()?;
    let overrides = store.theme_prefs()?;
    let preset = ThemePalette::resolve_preset(theme_name.as_deref(), &overrides);
    let palette = ThemePalette::from_prefs(theme_name.as_deref(), &overrides);
    let map = palette.to_color_map(preset.as_str());
    Ok(UserPrefsStore::format_single_section("theme", &map, fmt)?)
}

pub fn handle_theme(args: &ThemeArgs) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match &args.command {
        ThemeCommand::Export(export_args) => {
            let fmt = resolve_export_format(export_args.format, export_args.file.as_deref());
            let store = open_store_for_read()?;
            let output = export_theme_string(&store, fmt)?;
            write_output(export_args.file.as_deref(), &output, "theme")?;
            Ok(())
        }
        ThemeCommand::Import(import_args) => {
            let fmt = resolve_import_format(import_args.format);
            let raw = read_input(&import_args.file)?;
            let store = open_store_for_write()?;
            let report = store.import_theme_str(&raw, fmt, import_args.replace)?;
            println!(
                "Imported theme (updated: {}, reset_keys: {}, reset_sections: {})",
                report.updated, report.reset_keys, report.reset_sections
            );
            Ok(())
        }
    }
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
