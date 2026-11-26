use std::collections::{BTreeMap, BTreeSet};

use grein_tui::app::{App, FocusedPane};
use grein_tui::render::{render_app_mut_with_color_support, render_with_color_support};
use grein_tui::theme::{
    rgb_to_xterm256, supports_truecolor_from_env, ThemePalette, ThemePreset, ThemeTone,
};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;

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

fn contrast_ratio(a: Color, b: Color) -> f64 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
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

    let mut ghost_overrides = BTreeMap::new();
    ghost_overrides.insert("ghost_text".to_string(), "#778899".to_string());
    let custom_ghost = ThemePalette::from_prefs(Some("ansi"), &ghost_overrides);
    assert_eq!(custom_ghost.ghost_text, Color::Rgb(0x77, 0x88, 0x99));
    assert_eq!(
        custom_ghost
            .to_color_map("ansi")
            .get("ghost_text")
            .map(String::as_str),
        Some("#778899")
    );
}

#[test]
fn all_presets_have_readable_distinct_ghost_text_and_256color_separation() {
    assert_eq!(
        ThemePalette::from_preset(ThemePreset::Ansi).ghost_text,
        Color::Indexed(244)
    );

    for &preset in ThemePreset::all() {
        let palette = ThemePalette::from_preset(preset);
        assert_eq!(palette.adapt_for_color_support(true), palette);

        let adapted = palette.adapt_for_color_support(false);
        let exported = adapted.to_color_map(preset.as_str());
        for (key, value) in &exported {
            if key != "name" {
                assert!(
                    !value.starts_with('#'),
                    "Preset {preset:?} field {key} remained RGB ({value}) after 256-color adaptation"
                );
            }
        }

        if preset == ThemePreset::Ansi {
            assert_eq!(adapted, palette);
            continue;
        }

        let cr_text_ghost = contrast_ratio(palette.text, palette.ghost_text);
        let cr_ghost_panel = contrast_ratio(palette.ghost_text, palette.panel_bg);
        assert!(
            cr_text_ghost >= 1.9,
            "Preset {preset:?} CR(text, ghost_text) = {cr_text_ghost:.2} < 1.9"
        );
        assert!(
            cr_ghost_panel >= 2.2,
            "Preset {preset:?} CR(ghost_text, panel_bg) = {cr_ghost_panel:.2} < 2.2"
        );

        let text_lum = relative_luminance(palette.text);
        let ghost_lum = relative_luminance(palette.ghost_text);
        let panel_lum = relative_luminance(palette.panel_bg);
        match preset.tone() {
            ThemeTone::Dark => assert!(
                panel_lum < ghost_lum && ghost_lum < text_lum,
                "Dark preset {preset:?} luminance must satisfy panel_bg < ghost_text < text"
            ),
            ThemeTone::Light => assert!(
                text_lum < ghost_lum && ghost_lum < panel_lum,
                "Light preset {preset:?} luminance must satisfy text < ghost_text < panel_bg"
            ),
            ThemeTone::Terminal => unreachable!(),
        }

        assert!(matches!(adapted.text, Color::Indexed(16..=255)));
        assert!(matches!(adapted.ghost_text, Color::Indexed(16..=255)));
        assert!(matches!(adapted.panel_bg, Color::Indexed(16..=255)));
        assert_ne!(
            adapted.text, adapted.ghost_text,
            "Preset {preset:?} text and ghost_text collided under 256-color fallback"
        );
        assert_ne!(
            adapted.ghost_text, adapted.panel_bg,
            "Preset {preset:?} ghost_text and panel_bg collided under 256-color fallback"
        );
    }
}

#[test]
fn color_caps_detection_and_render_256color_fallback() {
    assert!(!supports_truecolor_from_env(
        None,
        Some("Apple_Terminal"),
        Some("xterm-256color"),
        false,
        false
    ));
    assert!(supports_truecolor_from_env(
        Some("truecolor"),
        Some("Apple_Terminal"),
        Some("xterm-256color"),
        false,
        false
    ));
    assert!(supports_truecolor_from_env(
        Some("24BIT"),
        None,
        None,
        false,
        false
    ));
    assert!(!supports_truecolor_from_env(
        Some(""),
        None,
        Some("xterm-256color"),
        false,
        false
    ));
    for tp in ["iTerm.app", "WezTerm", "ghostty", "vscode"] {
        assert!(
            supports_truecolor_from_env(None, Some(tp), None, false, false),
            "expected {tp} to support truecolor"
        );
    }
    assert!(supports_truecolor_from_env(None, None, None, true, false));
    assert!(supports_truecolor_from_env(None, None, None, false, true));
    assert!(supports_truecolor_from_env(
        None,
        None,
        Some("xterm-truecolor"),
        false,
        false
    ));

    assert_eq!(rgb_to_xterm256(0, 0, 0), 16);
    assert_eq!(rgb_to_xterm256(255, 255, 255), 231);
    assert_eq!(rgb_to_xterm256(128, 128, 128), 244);

    let mut app = App::new();
    app.set_focus(FocusedPane::Composer);
    app.input_buffer = "hello".to_string();
    app.cursor_position = 5;
    let orig_palette = app.theme_palette.clone();

    let mut tc_term = Terminal::new(TestBackend::new(80, 20)).unwrap();
    tc_term
        .draw(|f| render_with_color_support(f, &app, true))
        .unwrap();
    let has_rgb_in_tc = tc_term
        .backend()
        .buffer()
        .content()
        .iter()
        .any(|c| matches!(c.fg, Color::Rgb(..)) || matches!(c.bg, Color::Rgb(..)));
    assert!(has_rgb_in_tc, "Truecolor render must emit Color::Rgb cells");

    let mut fallback_term = Terminal::new(TestBackend::new(80, 20)).unwrap();
    fallback_term
        .draw(|f| render_with_color_support(f, &app, false))
        .unwrap();
    let has_rgb_in_256 = fallback_term
        .backend()
        .buffer()
        .content()
        .iter()
        .any(|c| matches!(c.fg, Color::Rgb(..)) || matches!(c.bg, Color::Rgb(..)));
    assert!(
        !has_rgb_in_256,
        "256-color fallback render must not emit Color::Rgb cells"
    );

    let mut mut_term = Terminal::new(TestBackend::new(80, 20)).unwrap();
    mut_term
        .draw(|f| render_app_mut_with_color_support(f, &mut app, false))
        .unwrap();
    assert_eq!(
        app.theme_palette, orig_palette,
        "render_app_mut_with_color_support must restore original RGB theme_palette"
    );
    let has_rgb_in_mut_256 = mut_term
        .backend()
        .buffer()
        .content()
        .iter()
        .any(|c| matches!(c.fg, Color::Rgb(..)) || matches!(c.bg, Color::Rgb(..)));
    assert!(
        !has_rgb_in_mut_256,
        "render_app_mut_with_color_support(false) must not emit Color::Rgb cells"
    );
}
