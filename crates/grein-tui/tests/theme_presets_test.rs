use std::collections::{BTreeMap, BTreeSet};

use grein_tui::theme::{ThemePalette, ThemePreset, ThemeTone};
use ratatui::style::Color;

fn relative_luminance(color: Color) -> f64 {
    let Color::Rgb(r, g, b) = color else {
        panic!("Expected RGB color, got {color:?}");
    };
    let channel = |c: u8| {
        let v = f64::from(c) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

#[test]
fn all_16_presets_have_unique_ids_and_display_names() {
    let presets = ThemePreset::all();
    assert_eq!(presets.len(), 16);

    let mut ids = BTreeSet::new();
    let mut display_names = BTreeSet::new();
    for preset in presets {
        assert!(
            ids.insert(preset.as_str()),
            "Duplicate preset id: {}",
            preset.as_str()
        );
        assert!(
            display_names.insert(preset.display_name()),
            "Duplicate display name: {}",
            preset.display_name()
        );
    }
    assert_eq!(ids.len(), 16);
    assert_eq!(display_names.len(), 16);
}

#[test]
fn canonical_roundtrip_and_aliases_parse_correctly() {
    for &preset in ThemePreset::all() {
        assert_eq!(ThemePreset::from_str(preset.as_str()), Some(preset));
        assert_eq!(preset.as_str().parse::<ThemePreset>(), Ok(preset));

        let upper_padded = format!("  {}  ", preset.as_str().to_uppercase());
        assert_eq!(ThemePreset::from_str(&upper_padded), Some(preset));
    }

    let alias_cases = [
        ("tokyonight", ThemePreset::TokyoNight),
        ("tokyo_night", ThemePreset::TokyoNight),
        ("tokyo", ThemePreset::TokyoNight),
        ("catppuccin_mocha", ThemePreset::CatppuccinMocha),
        ("catppuccin", ThemePreset::CatppuccinMocha),
        ("mocha", ThemePreset::CatppuccinMocha),
        ("default", ThemePreset::Ansi),
        ("terminal", ThemePreset::Ansi),
        ("gruvbox", ThemePreset::GruvboxDark),
        ("gruvbox_dark", ThemePreset::GruvboxDark),
        ("rose_pine", ThemePreset::RosePine),
        ("rosepine", ThemePreset::RosePine),
        ("rosé-pine", ThemePreset::RosePine),
        ("ROSÉ-PINE", ThemePreset::RosePine),
        ("kanagawa-wave", ThemePreset::Kanagawa),
        ("dracula", ThemePreset::Dracula),
        ("everforest-dark", ThemePreset::Everforest),
        ("onedark", ThemePreset::OneDark),
        ("one_dark", ThemePreset::OneDark),
        ("solarized", ThemePreset::SolarizedDark),
        ("solarized_dark", ThemePreset::SolarizedDark),
        ("github", ThemePreset::GitHubDark),
        ("github_dark", ThemePreset::GitHubDark),
        ("monokai-pro", ThemePreset::Monokai),
        ("latte", ThemePreset::CatppuccinLatte),
        ("catppuccin_latte", ThemePreset::CatppuccinLatte),
        ("gruvbox_light", ThemePreset::GruvboxLight),
        ("rose_pine_dawn", ThemePreset::RosePineDawn),
        ("dawn", ThemePreset::RosePineDawn),
    ];

    for (alias, expected) in alias_cases {
        assert_eq!(
            ThemePreset::from_str(alias),
            Some(expected),
            "Failed alias '{alias}'"
        );
    }

    assert_eq!(ThemePreset::from_str("nonexistent-theme"), None);
    assert_eq!(ThemePreset::from_str("dracula-dark"), None);
}

#[test]
fn preset_tones_and_labels_match_classification() {
    assert_eq!(ThemeTone::Dark.label(), "dark");
    assert_eq!(ThemeTone::Light.label(), "light");
    assert_eq!(ThemeTone::Terminal.label(), "term");

    let mut dark_count = 0usize;
    let mut light_count = 0usize;
    let mut term_count = 0usize;

    for preset in ThemePreset::all() {
        match preset.tone() {
            ThemeTone::Dark => dark_count += 1,
            ThemeTone::Light => light_count += 1,
            ThemeTone::Terminal => term_count += 1,
        }
    }

    assert_eq!(dark_count, 12);
    assert_eq!(light_count, 3);
    assert_eq!(term_count, 1);

    assert_eq!(ThemePreset::Ansi.tone(), ThemeTone::Terminal);
    assert_eq!(ThemePreset::CatppuccinLatte.tone(), ThemeTone::Light);
    assert_eq!(ThemePreset::GruvboxLight.tone(), ThemeTone::Light);
    assert_eq!(ThemePreset::RosePineDawn.tone(), ThemeTone::Light);

    for dark in [
        ThemePreset::TokyoNight,
        ThemePreset::CatppuccinMocha,
        ThemePreset::Nord,
        ThemePreset::GruvboxDark,
        ThemePreset::RosePine,
        ThemePreset::Kanagawa,
        ThemePreset::Dracula,
        ThemePreset::Everforest,
        ThemePreset::OneDark,
        ThemePreset::SolarizedDark,
        ThemePreset::GitHubDark,
        ThemePreset::Monokai,
    ] {
        assert_eq!(dark.tone(), ThemeTone::Dark);
    }
}

#[test]
fn palettes_have_contrast_and_valid_swatch_colors() {
    for &preset in ThemePreset::all() {
        let palette = ThemePalette::from_preset(preset);
        let swatch = palette.swatch_colors();
        assert_eq!(
            swatch,
            [
                palette.accent,
                palette.accent_secondary,
                palette.success,
                palette.warning,
                palette.error,
            ]
        );
        for color in swatch {
            assert_ne!(
                color,
                Color::Reset,
                "Preset {:?} has Color::Reset in swatch",
                preset
            );
        }

        if preset != ThemePreset::Ansi {
            assert_ne!(palette.bg, palette.text, "Preset {:?} bg == text", preset);
            assert_ne!(
                palette.panel_bg, palette.text,
                "Preset {:?} panel_bg == text",
                preset
            );
            assert_ne!(
                palette.selection_bg, palette.selection_fg,
                "Preset {:?} selection_bg == selection_fg",
                preset
            );
            assert_ne!(
                palette.pill_write_bg, palette.pill_write_fg,
                "Preset {:?} pill_write_bg == pill_write_fg",
                preset
            );
            assert_ne!(
                palette.pill_manage_bg, palette.pill_manage_fg,
                "Preset {:?} pill_manage_bg == pill_manage_fg",
                preset
            );

            let bg_lum = relative_luminance(palette.bg);
            let text_lum = relative_luminance(palette.text);
            match preset.tone() {
                ThemeTone::Dark => assert!(
                    bg_lum < text_lum,
                    "Dark preset {:?} should have darker bg than text",
                    preset
                ),
                ThemeTone::Light => assert!(
                    bg_lum > text_lum,
                    "Light preset {:?} should have lighter bg than text",
                    preset
                ),
                ThemeTone::Terminal => unreachable!(),
            }
        }
    }
}

#[test]
fn from_prefs_overrides_work_on_new_presets() {
    let mut overrides = BTreeMap::new();
    overrides.insert("accent".to_string(), "#123456".to_string());
    overrides.insert("error".to_string(), "light_red".to_string());
    overrides.insert("selection_bg".to_string(), "#abcdef".to_string());

    let palette = ThemePalette::from_prefs(Some("catppuccin-latte"), &overrides);
    let base = ThemePalette::from_preset(ThemePreset::CatppuccinLatte);
    assert_eq!(palette.bg, base.bg);
    assert_eq!(palette.text, base.text);
    assert_eq!(palette.accent, Color::Rgb(0x12, 0x34, 0x56));
    assert_eq!(palette.error, Color::LightRed);
    assert_eq!(palette.selection_bg, Color::Rgb(0xab, 0xcd, 0xef));
    assert_eq!(palette.swatch_colors()[0], Color::Rgb(0x12, 0x34, 0x56));
    assert_eq!(palette.swatch_colors()[4], Color::LightRed);

    let mut name_in_overrides = BTreeMap::new();
    name_in_overrides.insert("name".to_string(), "rosé-pine".to_string());
    name_in_overrides.insert("warning".to_string(), "#fedcba".to_string());
    assert_eq!(
        ThemePalette::resolve_preset(None, &name_in_overrides),
        ThemePreset::RosePine
    );
    let rose = ThemePalette::from_prefs(None, &name_in_overrides);
    assert_eq!(rose.bg, ThemePalette::from_preset(ThemePreset::RosePine).bg);
    assert_eq!(rose.warning, Color::Rgb(0xfe, 0xdc, 0xba));
}
