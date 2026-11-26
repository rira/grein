use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub const PRESET_PREF_KEY: &str = "preset";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum KeymapPreset {
    #[default]
    Standard,
    MacOs,
}

impl KeymapPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::MacOs => "macos",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::MacOs => "macOS",
        }
    }

    pub fn platform_default() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Standard
        }
    }

    pub fn status_notice(self) -> String {
        match self {
            Self::Standard => "Keymap preset: standard (Alt+ & Shift+Arrows hints)".to_string(),
            Self::MacOs => "Keymap preset: macos (Ctrl+G leader hints)".to_string(),
        }
    }

    pub fn current_status_notice(self) -> String {
        format!(
            "Keymap preset: {} (available: standard, macos)",
            self.as_str()
        )
    }
}

impl fmt::Display for KeymapPreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for KeymapPreset {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "standard" | "default" | "linux" => Ok(Self::Standard),
            "macos" | "mac" | "leader" => Ok(Self::MacOs),
            other => Err(format!(
                "Unknown keymap preset '{other}'. Available: standard, macos"
            )),
        }
    }
}

pub trait KeymapPrefs {
    fn collect_prefs(&self) -> Vec<(&str, &str)>;
}

impl KeymapPrefs for BTreeMap<String, String> {
    fn collect_prefs(&self) -> Vec<(&str, &str)> {
        self.iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }
}

impl KeymapPrefs for [(String, String)] {
    fn collect_prefs(&self) -> Vec<(&str, &str)> {
        self.iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }
}

impl<const N: usize> KeymapPrefs for [(String, String); N] {
    fn collect_prefs(&self) -> Vec<(&str, &str)> {
        self.iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }
}

impl KeymapPrefs for Vec<(String, String)> {
    fn collect_prefs(&self) -> Vec<(&str, &str)> {
        self.iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }
}

impl KeymapPrefs for [(&str, &str)] {
    fn collect_prefs(&self) -> Vec<(&str, &str)> {
        self.to_vec()
    }
}

impl<const N: usize> KeymapPrefs for [(&str, &str); N] {
    fn collect_prefs(&self) -> Vec<(&str, &str)> {
        self.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_preset_parse_format_and_platform_default() {
        assert_eq!(KeymapPreset::default(), KeymapPreset::Standard);
        assert_eq!(KeymapPreset::Standard.as_str(), "standard");
        assert_eq!(KeymapPreset::MacOs.as_str(), "macos");
        assert_eq!(KeymapPreset::Standard.to_string(), "standard");
        assert_eq!(KeymapPreset::MacOs.to_string(), "macos");

        for alias in ["standard", "DEFAULT", "linux"] {
            assert_eq!(alias.parse::<KeymapPreset>().unwrap(), KeymapPreset::Standard);
        }
        for alias in ["macos", "MAC", "leader"] {
            assert_eq!(alias.parse::<KeymapPreset>().unwrap(), KeymapPreset::MacOs);
        }
        assert!("bogus".parse::<KeymapPreset>().is_err());

        let expected = if cfg!(target_os = "macos") {
            KeymapPreset::MacOs
        } else {
            KeymapPreset::Standard
        };
        assert_eq!(KeymapPreset::platform_default(), expected);
    }
}
