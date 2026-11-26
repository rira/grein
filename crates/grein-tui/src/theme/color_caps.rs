const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub fn supports_truecolor_from_env(
    colorterm: Option<&str>,
    term_program: Option<&str>,
    term: Option<&str>,
    has_kitty: bool,
    has_ghostty: bool,
) -> bool {
    if let Some(ct) = colorterm.map(str::trim).filter(|v| !v.is_empty()) {
        let lower = ct.to_ascii_lowercase();
        if lower.contains("truecolor") || lower.contains("24bit") {
            return true;
        }
    }
    if let Some(tp) = term_program.map(str::trim).filter(|v| !v.is_empty()) {
        if matches!(
            tp.to_ascii_lowercase().as_str(),
            "iterm.app" | "wezterm" | "ghostty" | "vscode"
        ) {
            return true;
        }
    }
    if has_kitty || has_ghostty {
        return true;
    }
    if let Some(t) = term.map(str::trim).filter(|v| !v.is_empty()) {
        let lower = t.to_ascii_lowercase();
        if lower.contains("truecolor") || lower.contains("24bit") {
            return true;
        }
    }
    false
}

pub fn detect_terminal_truecolor() -> bool {
    let colorterm = env_non_empty("COLORTERM");
    let term_program = env_non_empty("TERM_PROGRAM");
    let term = env_non_empty("TERM");
    let has_kitty = env_non_empty("KITTY_WINDOW_ID").is_some();
    let has_ghostty = env_non_empty("GHOSTTY_RESOURCES_DIR").is_some();
    supports_truecolor_from_env(
        colorterm.as_deref(),
        term_program.as_deref(),
        term.as_deref(),
        has_kitty,
        has_ghostty,
    )
}

fn sq_dist(r1: u8, g1: u8, b1: u8, r2: u8, g2: u8, b2: u8) -> u32 {
    let dr = i32::from(r1) - i32::from(r2);
    let dg = i32::from(g1) - i32::from(g2);
    let db = i32::from(b1) - i32::from(b2);
    (dr * dr + dg * dg + db * db) as u32
}

pub fn rgb_to_xterm256(r: u8, g: u8, b: u8) -> u8 {
    let mut best_idx = 16u8;
    let mut best_dist = u32::MAX;

    for (ri, &cr) in CUBE_LEVELS.iter().enumerate() {
        for (gi, &cg) in CUBE_LEVELS.iter().enumerate() {
            for (bi, &cb) in CUBE_LEVELS.iter().enumerate() {
                let dist = sq_dist(r, g, b, cr, cg, cb);
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = 16 + (ri as u8) * 36 + (gi as u8) * 6 + (bi as u8);
                }
            }
        }
    }

    for i in 0u8..24 {
        let v = 8 + 10 * i;
        let dist = sq_dist(r, g, b, v, v, v);
        if dist < best_dist {
            best_dist = dist;
            best_idx = 232 + i;
        }
    }

    best_idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_truecolor_from_env_matrix() {
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
            Some("TrueColor"),
            None,
            None,
            false,
            false
        ));
        assert!(supports_truecolor_from_env(
            Some("24bit"),
            None,
            None,
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

        for tp in ["iTerm.app", "WezTerm", "ghostty", "vscode"] {
            assert!(
                supports_truecolor_from_env(None, Some(tp), None, false, false),
                "expected {tp} -> true"
            );
        }

        assert!(supports_truecolor_from_env(None, None, None, true, false));
        assert!(supports_truecolor_from_env(None, None, None, false, true));

        assert!(!supports_truecolor_from_env(
            Some(""),
            Some("   "),
            Some("xterm-256color"),
            false,
            false
        ));
        assert!(supports_truecolor_from_env(
            Some(""),
            None,
            Some("tmux-24bit"),
            false,
            false
        ));
        assert!(supports_truecolor_from_env(
            None,
            None,
            Some("xterm-TrueColor"),
            false,
            false
        ));
        assert!(!supports_truecolor_from_env(None, None, None, false, false));
    }

    #[test]
    fn test_rgb_to_xterm256_cube_grayscale_and_range() {
        assert_eq!(rgb_to_xterm256(0, 0, 0), 16);
        assert_eq!(rgb_to_xterm256(255, 255, 255), 231);
        assert_eq!(rgb_to_xterm256(255, 0, 0), 196);
        assert_eq!(rgb_to_xterm256(0, 255, 0), 46);
        assert_eq!(rgb_to_xterm256(0, 0, 255), 21);
        assert_eq!(rgb_to_xterm256(95, 135, 175), 67);

        assert_eq!(rgb_to_xterm256(8, 8, 8), 232);
        assert_eq!(rgb_to_xterm256(48, 48, 48), 236);
        assert_eq!(rgb_to_xterm256(128, 128, 128), 244);
        assert_eq!(rgb_to_xterm256(238, 238, 238), 255);
        assert_eq!(rgb_to_xterm256(90, 90, 90), 240);

        for r in (0..=255).step_by(51) {
            for g in (0..=255).step_by(51) {
                for b in (0..=255).step_by(51) {
                    let idx = rgb_to_xterm256(r as u8, g as u8, b as u8);
                    assert!(
                        idx >= 16,
                        "rgb_to_xterm256({r}, {g}, {b}) mapped into mutable ANSI 0..=15: {idx}"
                    );
                }
            }
        }
    }
}
