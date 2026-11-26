pub mod color_caps;
mod presets;

pub use color_caps::{detect_terminal_truecolor, rgb_to_xterm256, supports_truecolor_from_env};

use std::collections::BTreeMap;
use std::str::FromStr;

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeTone {
    Dark,
    Light,
    Terminal,
}

impl ThemeTone {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Terminal => "term",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemePreset {
    #[default]
    TokyoNight,
    CatppuccinMocha,
    Nord,
    Ansi,
    GruvboxDark,
    RosePine,
    Kanagawa,
    Dracula,
    Everforest,
    OneDark,
    SolarizedDark,
    GitHubDark,
    Monokai,
    CatppuccinLatte,
    GruvboxLight,
    RosePineDawn,
}

impl ThemePreset {
    pub fn all() -> &'static [ThemePreset] {
        &[
            ThemePreset::TokyoNight,
            ThemePreset::CatppuccinMocha,
            ThemePreset::Nord,
            ThemePreset::Ansi,
            ThemePreset::GruvboxDark,
            ThemePreset::RosePine,
            ThemePreset::Kanagawa,
            ThemePreset::Dracula,
            ThemePreset::Everforest,
            ThemePreset::OneDark,
            ThemePreset::SolarizedDark,
            ThemePreset::GitHubDark,
            ThemePreset::Monokai,
            ThemePreset::CatppuccinLatte,
            ThemePreset::GruvboxLight,
            ThemePreset::RosePineDawn,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TokyoNight => "tokyo-night",
            Self::CatppuccinMocha => "catppuccin-mocha",
            Self::Nord => "nord",
            Self::Ansi => "ansi",
            Self::GruvboxDark => "gruvbox-dark",
            Self::RosePine => "rose-pine",
            Self::Kanagawa => "kanagawa",
            Self::Dracula => "dracula",
            Self::Everforest => "everforest",
            Self::OneDark => "one-dark",
            Self::SolarizedDark => "solarized-dark",
            Self::GitHubDark => "github-dark",
            Self::Monokai => "monokai",
            Self::CatppuccinLatte => "catppuccin-latte",
            Self::GruvboxLight => "gruvbox-light",
            Self::RosePineDawn => "rose-pine-dawn",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::TokyoNight => "Tokyo Night",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::Nord => "Nord",
            Self::Ansi => "ANSI",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::RosePine => "Rose Pine",
            Self::Kanagawa => "Kanagawa",
            Self::Dracula => "Dracula",
            Self::Everforest => "Everforest",
            Self::OneDark => "One Dark",
            Self::SolarizedDark => "Solarized Dark",
            Self::GitHubDark => "GitHub Dark",
            Self::Monokai => "Monokai",
            Self::CatppuccinLatte => "Catppuccin Latte",
            Self::GruvboxLight => "Gruvbox Light",
            Self::RosePineDawn => "Rose Pine Dawn",
        }
    }

    pub fn tone(&self) -> ThemeTone {
        match self {
            Self::Ansi => ThemeTone::Terminal,
            Self::CatppuccinLatte | Self::GruvboxLight | Self::RosePineDawn => ThemeTone::Light,
            Self::TokyoNight
            | Self::CatppuccinMocha
            | Self::Nord
            | Self::GruvboxDark
            | Self::RosePine
            | Self::Kanagawa
            | Self::Dracula
            | Self::Everforest
            | Self::OneDark
            | Self::SolarizedDark
            | Self::GitHubDark
            | Self::Monokai => ThemeTone::Dark,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "tokyo-night" | "tokyonight" | "tokyo_night" | "tokyo" => Some(Self::TokyoNight),
            "catppuccin-mocha" | "catppuccin_mocha" | "catppuccin" | "mocha" => {
                Some(Self::CatppuccinMocha)
            }
            "nord" => Some(Self::Nord),
            "ansi" | "default" | "terminal" => Some(Self::Ansi),
            "gruvbox-dark" | "gruvbox" | "gruvbox_dark" => Some(Self::GruvboxDark),
            "rose-pine" | "rose_pine" | "rosepine" | "rosé-pine" => Some(Self::RosePine),
            "kanagawa" | "kanagawa-wave" => Some(Self::Kanagawa),
            "dracula" => Some(Self::Dracula),
            "everforest" | "everforest-dark" => Some(Self::Everforest),
            "one-dark" | "onedark" | "one_dark" => Some(Self::OneDark),
            "solarized-dark" | "solarized" | "solarized_dark" => Some(Self::SolarizedDark),
            "github-dark" | "github" | "github_dark" => Some(Self::GitHubDark),
            "monokai" | "monokai-pro" => Some(Self::Monokai),
            "catppuccin-latte" | "latte" | "catppuccin_latte" => Some(Self::CatppuccinLatte),
            "gruvbox-light" | "gruvbox_light" => Some(Self::GruvboxLight),
            "rose-pine-dawn" | "rose_pine_dawn" | "dawn" => Some(Self::RosePineDawn),
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
    pub ghost_text: Color,
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

type ColorGetter = fn(&ThemePalette) -> Color;
type ColorMutGetter = fn(&mut ThemePalette) -> &mut Color;

const THEME_COLOR_FIELDS: &[(&str, ColorGetter, ColorMutGetter)] = &[
    ("bg", |p| p.bg, |p| &mut p.bg),
    ("panel_bg", |p| p.panel_bg, |p| &mut p.panel_bg),
    ("status_bar_bg", |p| p.status_bar_bg, |p| &mut p.status_bar_bg),
    ("selection_bg", |p| p.selection_bg, |p| &mut p.selection_bg),
    ("selection_fg", |p| p.selection_fg, |p| &mut p.selection_fg),
    ("selection_bar", |p| p.selection_bar, |p| &mut p.selection_bar),
    ("border_focused", |p| p.border_focused, |p| &mut p.border_focused),
    ("border_unfocused", |p| p.border_unfocused, |p| &mut p.border_unfocused),
    ("text", |p| p.text, |p| &mut p.text),
    ("muted", |p| p.muted, |p| &mut p.muted),
    ("ghost_text", |p| p.ghost_text, |p| &mut p.ghost_text),
    ("accent", |p| p.accent, |p| &mut p.accent),
    ("accent_secondary", |p| p.accent_secondary, |p| &mut p.accent_secondary),
    ("user_accent", |p| p.user_accent, |p| &mut p.user_accent),
    ("dirty", |p| p.dirty, |p| &mut p.dirty),
    ("success", |p| p.success, |p| &mut p.success),
    ("error", |p| p.error, |p| &mut p.error),
    ("warning", |p| p.warning, |p| &mut p.warning),
    ("diff_add", |p| p.diff_add, |p| &mut p.diff_add),
    ("diff_del", |p| p.diff_del, |p| &mut p.diff_del),
    ("diff_hunk", |p| p.diff_hunk, |p| &mut p.diff_hunk),
    ("pill_write_bg", |p| p.pill_write_bg, |p| &mut p.pill_write_bg),
    ("pill_write_fg", |p| p.pill_write_fg, |p| &mut p.pill_write_fg),
    ("pill_manage_bg", |p| p.pill_manage_bg, |p| &mut p.pill_manage_bg),
    ("pill_manage_fg", |p| p.pill_manage_fg, |p| &mut p.pill_manage_fg),
];

impl Default for ThemePalette {
    fn default() -> Self {
        Self::from_preset(ThemePreset::TokyoNight)
    }
}

impl ThemePalette {
    pub fn from_preset(preset: ThemePreset) -> Self {
        presets::palette_for_preset(preset)
    }

    pub fn adapt_for_color_support(&self, truecolor: bool) -> Self {
        let mut adapted = self.clone();
        if truecolor {
            return adapted;
        }
        for &(_, _, mut_getter) in THEME_COLOR_FIELDS {
            let slot = mut_getter(&mut adapted);
            if let Color::Rgb(r, g, b) = *slot {
                *slot = Color::Indexed(color_caps::rgb_to_xterm256(r, g, b));
            }
        }
        adapted
    }

    pub fn swatch_colors(&self) -> [Color; 5] {
        [
            self.accent,
            self.accent_secondary,
            self.success,
            self.warning,
            self.error,
        ]
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

    pub fn from_prefs(theme_name: Option<&str>, overrides: &BTreeMap<String, String>) -> Self {
        let preset = Self::resolve_preset(theme_name, overrides);
        let mut palette = Self::from_preset(preset);
        for (key, value) in overrides {
            if key == "name" {
                continue;
            }
            let Some(color) = Self::parse_color(value) else {
                continue;
            };
            let normalized = key.trim().to_ascii_lowercase();
            if let Some(&(_, _, mut_getter)) = THEME_COLOR_FIELDS
                .iter()
                .find(|&&(field, _, _)| field == normalized)
            {
                *mut_getter(&mut palette) = color;
            }
        }
        palette
    }

    pub fn to_color_map(&self, preset_name: &str) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), preset_name.to_string());
        for &(field, getter, _) in THEME_COLOR_FIELDS {
            map.insert(field.to_string(), Self::format_color(getter(self)));
        }
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
