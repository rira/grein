use std::collections::BTreeMap;
use std::str::FromStr;

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemePreset {
    #[default]
    TokyoNight,
    CatppuccinMocha,
    Nord,
    Ansi,
}

impl ThemePreset {
    pub fn all() -> &'static [ThemePreset] {
        &[
            ThemePreset::TokyoNight,
            ThemePreset::CatppuccinMocha,
            ThemePreset::Nord,
            ThemePreset::Ansi,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TokyoNight => "tokyo-night",
            Self::CatppuccinMocha => "catppuccin-mocha",
            Self::Nord => "nord",
            Self::Ansi => "ansi",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::TokyoNight => "Tokyo Night",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::Nord => "Nord",
            Self::Ansi => "ANSI",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "tokyo-night" | "tokyonight" | "tokyo_night" | "tokyo" => Some(Self::TokyoNight),
            "catppuccin-mocha" | "catppuccin_mocha" | "catppuccin" | "mocha" => {
                Some(Self::CatppuccinMocha)
            }
            "nord" => Some(Self::Nord),
            "ansi" | "default" | "terminal" => Some(Self::Ansi),
            _ => None,
        }
    }
}

impl FromStr for ThemePreset {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s).ok_or_else(|| format!("Unknown theme preset '{s}'"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePalette {
    pub bg: Color,
    pub panel_bg: Color,
    pub status_bar_bg: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub selection_bar: Color,
    pub border_focused: Color,
    pub border_unfocused: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_secondary: Color,
    pub user_accent: Color,
    pub dirty: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub diff_add: Color,
    pub diff_del: Color,
    pub diff_hunk: Color,
    pub pill_write_bg: Color,
    pub pill_write_fg: Color,
    pub pill_manage_bg: Color,
    pub pill_manage_fg: Color,
}

impl Default for ThemePalette {
    fn default() -> Self {
        Self::from_preset(ThemePreset::TokyoNight)
    }
}

impl ThemePalette {
    pub fn from_preset(preset: ThemePreset) -> Self {
        match preset {
            ThemePreset::TokyoNight => Self {
                bg: Color::Rgb(0x1a, 0x1b, 0x26),
                panel_bg: Color::Rgb(0x16, 0x16, 0x1e),
                status_bar_bg: Color::Rgb(0x1f, 0x23, 0x35),
                selection_bg: Color::Rgb(0x28, 0x34, 0x57),
                selection_fg: Color::Rgb(0xc0, 0xca, 0xf5),
                selection_bar: Color::Rgb(0x7a, 0xa2, 0xf7),
                border_focused: Color::Rgb(0x7a, 0xa2, 0xf7),
                border_unfocused: Color::Rgb(0x3b, 0x42, 0x61),
                text: Color::Rgb(0xc0, 0xca, 0xf5),
                muted: Color::Rgb(0x56, 0x5f, 0x89),
                accent: Color::Rgb(0x7d, 0xcf, 0xff),
                accent_secondary: Color::Rgb(0xbb, 0x9a, 0xf7),
                user_accent: Color::Rgb(0x9e, 0xce, 0x6a),
                dirty: Color::Rgb(0xe0, 0xaf, 0x68),
                success: Color::Rgb(0x9e, 0xce, 0x6a),
                error: Color::Rgb(0xf7, 0x76, 0x8e),
                warning: Color::Rgb(0xe0, 0xaf, 0x68),
                diff_add: Color::Rgb(0x9e, 0xce, 0x6a),
                diff_del: Color::Rgb(0xf7, 0x76, 0x8e),
                diff_hunk: Color::Rgb(0x7d, 0xcf, 0xff),
                pill_write_bg: Color::Rgb(0x7a, 0xa2, 0xf7),
                pill_write_fg: Color::Rgb(0x15, 0x16, 0x1e),
                pill_manage_bg: Color::Rgb(0xbb, 0x9a, 0xf7),
                pill_manage_fg: Color::Rgb(0x15, 0x16, 0x1e),
            },
            ThemePreset::CatppuccinMocha => Self {
                bg: Color::Rgb(0x1e, 0x1e, 0x2e),
                panel_bg: Color::Rgb(0x18, 0x18, 0x25),
                status_bar_bg: Color::Rgb(0x11, 0x11, 0x1b),
                selection_bg: Color::Rgb(0x31, 0x32, 0x44),
                selection_fg: Color::Rgb(0xcd, 0xd6, 0xf4),
                selection_bar: Color::Rgb(0x89, 0xb4, 0xfa),
                border_focused: Color::Rgb(0x89, 0xb4, 0xfa),
                border_unfocused: Color::Rgb(0x45, 0x47, 0x5a),
                text: Color::Rgb(0xcd, 0xd6, 0xf4),
                muted: Color::Rgb(0x6c, 0x70, 0x86),
                accent: Color::Rgb(0x89, 0xdc, 0xeb),
                accent_secondary: Color::Rgb(0xcb, 0xa6, 0xf7),
                user_accent: Color::Rgb(0xa6, 0xe3, 0xa1),
                dirty: Color::Rgb(0xf9, 0xe2, 0xaf),
                success: Color::Rgb(0xa6, 0xe3, 0xa1),
                error: Color::Rgb(0xf3, 0x8b, 0xa8),
                warning: Color::Rgb(0xf9, 0xe2, 0xaf),
                diff_add: Color::Rgb(0xa6, 0xe3, 0xa1),
                diff_del: Color::Rgb(0xf3, 0x8b, 0xa8),
                diff_hunk: Color::Rgb(0x89, 0xdc, 0xeb),
                pill_write_bg: Color::Rgb(0x89, 0xb4, 0xfa),
                pill_write_fg: Color::Rgb(0x11, 0x11, 0x1b),
                pill_manage_bg: Color::Rgb(0xcb, 0xa6, 0xf7),
                pill_manage_fg: Color::Rgb(0x11, 0x11, 0x1b),
            },
            ThemePreset::Nord => Self {
                bg: Color::Rgb(0x2e, 0x34, 0x40),
                panel_bg: Color::Rgb(0x24, 0x29, 0x33),
                status_bar_bg: Color::Rgb(0x3b, 0x42, 0x52),
                selection_bg: Color::Rgb(0x43, 0x4c, 0x5e),
                selection_fg: Color::Rgb(0xec, 0xef, 0xf4),
                selection_bar: Color::Rgb(0x88, 0xc0, 0xd0),
                border_focused: Color::Rgb(0x88, 0xc0, 0xd0),
                border_unfocused: Color::Rgb(0x4c, 0x56, 0x6a),
                text: Color::Rgb(0xd8, 0xde, 0xe9),
                muted: Color::Rgb(0x61, 0x6e, 0x88),
                accent: Color::Rgb(0x88, 0xc0, 0xd0),
                accent_secondary: Color::Rgb(0xb4, 0x8e, 0xad),
                user_accent: Color::Rgb(0xa3, 0xbe, 0x8c),
                dirty: Color::Rgb(0xeb, 0xcb, 0x8b),
                success: Color::Rgb(0xa3, 0xbe, 0x8c),
                error: Color::Rgb(0xbf, 0x61, 0x6a),
                warning: Color::Rgb(0xeb, 0xcb, 0x8b),
                diff_add: Color::Rgb(0xa3, 0xbe, 0x8c),
                diff_del: Color::Rgb(0xbf, 0x61, 0x6a),
                diff_hunk: Color::Rgb(0x81, 0xa1, 0xc1),
                pill_write_bg: Color::Rgb(0x88, 0xc0, 0xd0),
                pill_write_fg: Color::Rgb(0x2e, 0x34, 0x40),
                pill_manage_bg: Color::Rgb(0xb4, 0x8e, 0xad),
                pill_manage_fg: Color::Rgb(0x2e, 0x34, 0x40),
            },
            ThemePreset::Ansi => Self {
                bg: Color::Reset,
                panel_bg: Color::Reset,
                status_bar_bg: Color::Reset,
                selection_bg: Color::DarkGray,
                selection_fg: Color::Cyan,
                selection_bar: Color::Cyan,
                border_focused: Color::Cyan,
                border_unfocused: Color::DarkGray,
                text: Color::Reset,
                muted: Color::DarkGray,
                accent: Color::Cyan,
                accent_secondary: Color::Magenta,
                user_accent: Color::Green,
                dirty: Color::Yellow,
                success: Color::Green,
                error: Color::Red,
                warning: Color::Yellow,
                diff_add: Color::Green,
                diff_del: Color::Red,
                diff_hunk: Color::Cyan,
                pill_write_bg: Color::Cyan,
                pill_write_fg: Color::Black,
                pill_manage_bg: Color::Magenta,
                pill_manage_fg: Color::Black,
            },
        }
    }

    pub fn resolve_preset(
        theme_name: Option<&str>,
        overrides: &BTreeMap<String, String>,
    ) -> ThemePreset {
        theme_name
            .and_then(ThemePreset::from_str)
            .or_else(|| overrides.get("name").and_then(|n| ThemePreset::from_str(n)))
            .unwrap_or(ThemePreset::TokyoNight)
    }

    pub fn from_prefs(
        theme_name: Option<&str>,
        overrides: &BTreeMap<String, String>,
    ) -> Self {
        let preset = Self::resolve_preset(theme_name, overrides);
        let mut palette = Self::from_preset(preset);
        for (key, value) in overrides {
            if key == "name" {
                continue;
            }
            let Some(color) = Self::parse_color(value) else {
                continue;
            };
            match key.trim().to_ascii_lowercase().as_str() {
                "bg" => palette.bg = color,
                "panel_bg" => palette.panel_bg = color,
                "status_bar_bg" => palette.status_bar_bg = color,
                "selection_bg" => palette.selection_bg = color,
                "selection_fg" => palette.selection_fg = color,
                "selection_bar" => palette.selection_bar = color,
                "border_focused" => palette.border_focused = color,
                "border_unfocused" => palette.border_unfocused = color,
                "text" => palette.text = color,
                "muted" => palette.muted = color,
                "accent" => palette.accent = color,
                "accent_secondary" => palette.accent_secondary = color,
                "user_accent" => palette.user_accent = color,
                "dirty" => palette.dirty = color,
                "success" => palette.success = color,
                "error" => palette.error = color,
                "warning" => palette.warning = color,
                "diff_add" => palette.diff_add = color,
                "diff_del" => palette.diff_del = color,
                "diff_hunk" => palette.diff_hunk = color,
                "pill_write_bg" => palette.pill_write_bg = color,
                "pill_write_fg" => palette.pill_write_fg = color,
                "pill_manage_bg" => palette.pill_manage_bg = color,
                "pill_manage_fg" => palette.pill_manage_fg = color,
                _ => {}
            }
        }
        palette
    }

    pub fn to_color_map(&self, preset_name: &str) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), preset_name.to_string());
        map.insert("bg".to_string(), Self::format_color(self.bg));
        map.insert("panel_bg".to_string(), Self::format_color(self.panel_bg));
        map.insert(
            "status_bar_bg".to_string(),
            Self::format_color(self.status_bar_bg),
        );
        map.insert(
            "selection_bg".to_string(),
            Self::format_color(self.selection_bg),
        );
        map.insert(
            "selection_fg".to_string(),
            Self::format_color(self.selection_fg),
        );
        map.insert(
            "selection_bar".to_string(),
            Self::format_color(self.selection_bar),
        );
        map.insert(
            "border_focused".to_string(),
            Self::format_color(self.border_focused),
        );
        map.insert(
            "border_unfocused".to_string(),
            Self::format_color(self.border_unfocused),
        );
        map.insert("text".to_string(), Self::format_color(self.text));
        map.insert("muted".to_string(), Self::format_color(self.muted));
        map.insert("accent".to_string(), Self::format_color(self.accent));
        map.insert(
            "accent_secondary".to_string(),
            Self::format_color(self.accent_secondary),
        );
        map.insert(
            "user_accent".to_string(),
            Self::format_color(self.user_accent),
        );
        map.insert("dirty".to_string(), Self::format_color(self.dirty));
        map.insert("success".to_string(), Self::format_color(self.success));
        map.insert("error".to_string(), Self::format_color(self.error));
        map.insert("warning".to_string(), Self::format_color(self.warning));
        map.insert("diff_add".to_string(), Self::format_color(self.diff_add));
        map.insert("diff_del".to_string(), Self::format_color(self.diff_del));
        map.insert("diff_hunk".to_string(), Self::format_color(self.diff_hunk));
        map.insert(
            "pill_write_bg".to_string(),
            Self::format_color(self.pill_write_bg),
        );
        map.insert(
            "pill_write_fg".to_string(),
            Self::format_color(self.pill_write_fg),
        );
        map.insert(
            "pill_manage_bg".to_string(),
            Self::format_color(self.pill_manage_bg),
        );
        map.insert(
            "pill_manage_fg".to_string(),
            Self::format_color(self.pill_manage_fg),
        );
        map
    }

    pub fn parse_color(spec: &str) -> Option<Color> {
        let s = spec.trim();
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() == 6 && hex.is_ascii() {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                return Some(Color::Rgb(r, g, b));
            }
            if hex.len() == 3 && hex.is_ascii() {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                return Some(Color::Rgb(r, g, b));
            }
            return None;
        }
        let lower = s.to_ascii_lowercase();
        if let Some(inner) = lower
            .strip_prefix("indexed(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            let idx = inner.trim().parse::<u8>().ok()?;
            return Some(Color::Indexed(idx));
        }
        match lower.as_str() {
            "black" => Some(Color::Black),
            "red" => Some(Color::Red),
            "green" => Some(Color::Green),
            "yellow" => Some(Color::Yellow),
            "blue" => Some(Color::Blue),
            "magenta" => Some(Color::Magenta),
            "cyan" => Some(Color::Cyan),
            "white" => Some(Color::White),
            "gray" | "grey" => Some(Color::Gray),
            "dark_gray" | "darkgray" | "dark_grey" | "darkgrey" => Some(Color::DarkGray),
            "light_red" | "lightred" => Some(Color::LightRed),
            "light_green" | "lightgreen" => Some(Color::LightGreen),
            "light_yellow" | "lightyellow" => Some(Color::LightYellow),
            "light_blue" | "lightblue" => Some(Color::LightBlue),
            "light_magenta" | "lightmagenta" => Some(Color::LightMagenta),
            "light_cyan" | "lightcyan" => Some(Color::LightCyan),
            "reset" | "default" => Some(Color::Reset),
            _ => None,
        }
    }

    pub fn format_color(color: Color) -> String {
        match color {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            Color::Black => "black".to_string(),
            Color::Red => "red".to_string(),
            Color::Green => "green".to_string(),
            Color::Yellow => "yellow".to_string(),
            Color::Blue => "blue".to_string(),
            Color::Magenta => "magenta".to_string(),
            Color::Cyan => "cyan".to_string(),
            Color::White => "white".to_string(),
            Color::Gray => "gray".to_string(),
            Color::DarkGray => "dark_gray".to_string(),
            Color::LightRed => "light_red".to_string(),
            Color::LightGreen => "light_green".to_string(),
            Color::LightYellow => "light_yellow".to_string(),
            Color::LightBlue => "light_blue".to_string(),
            Color::LightMagenta => "light_magenta".to_string(),
            Color::LightCyan => "light_cyan".to_string(),
            Color::Reset => "reset".to_string(),
            Color::Indexed(idx) => format!("indexed({idx})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePickerModal {
    pub selected_idx: usize,
    pub original_preset: ThemePreset,
    pub original_overrides: BTreeMap<String, String>,
}
