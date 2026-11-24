use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

pub(crate) const UNSUPPORTED_KEY_MODIFIERS: KeyModifiers = KeyModifiers::SUPER
    .union(KeyModifiers::HYPER)
    .union(KeyModifiers::META);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyChord {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self::normalize(code, modifiers)
    }

    fn normalize(code: KeyCode, modifiers: KeyModifiers) -> Self {
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        let alt = modifiers.contains(KeyModifiers::ALT);
        let shift = modifiers.contains(KeyModifiers::SHIFT);
        let extra = modifiers.intersection(UNSUPPORTED_KEY_MODIFIERS);
        let mut norm_mods = extra;
        if ctrl {
            norm_mods |= KeyModifiers::CONTROL;
        }
        if alt {
            norm_mods |= KeyModifiers::ALT;
        }
        match code {
            KeyCode::BackTab => Self {
                code: KeyCode::BackTab,
                modifiers: norm_mods,
            },
            KeyCode::Tab => {
                if shift && !ctrl && !alt && extra.is_empty() {
                    Self {
                        code: KeyCode::BackTab,
                        modifiers: KeyModifiers::NONE,
                    }
                } else {
                    if shift {
                        norm_mods |= KeyModifiers::SHIFT;
                    }
                    Self {
                        code: KeyCode::Tab,
                        modifiers: norm_mods,
                    }
                }
            }
            KeyCode::Char(c) => {
                if ctrl || alt || !extra.is_empty() {
                    if shift {
                        norm_mods |= KeyModifiers::SHIFT;
                        Self {
                            code: KeyCode::Char(c.to_ascii_uppercase()),
                            modifiers: norm_mods,
                        }
                    } else {
                        Self {
                            code: KeyCode::Char(c.to_ascii_lowercase()),
                            modifiers: norm_mods,
                        }
                    }
                } else {
                    let ch = if shift { c.to_ascii_uppercase() } else { c };
                    Self {
                        code: KeyCode::Char(ch),
                        modifiers: KeyModifiers::NONE,
                    }
                }
            }
            other => {
                if shift {
                    norm_mods |= KeyModifiers::SHIFT;
                }
                Self {
                    code: other,
                    modifiers: norm_mods,
                }
            }
        }
    }

    pub fn from_event(key: KeyEvent) -> Option<Self> {
        if key.modifiers.intersects(UNSUPPORTED_KEY_MODIFIERS) {
            return None;
        }
        Some(Self::normalize(key.code, key.modifiers))
    }

    pub fn matches(&self, key: KeyEvent) -> bool {
        Self::from_event(key).is_some_and(|ev| *self == ev)
    }

    pub fn parse(spec: &str) -> Result<Self, String> {
        let trimmed = spec.trim();
        if trimmed.is_empty() {
            return Err("Empty key chord".to_string());
        }
        if trimmed == "+" {
            return Ok(Self::new(KeyCode::Char('+'), KeyModifiers::NONE));
        }

        let (mod_parts, key_part): (Vec<&str>, &str) = if let Some(prefix) = trimmed.strip_suffix("++") {
            let parts: Vec<&str> = prefix.split('+').collect();
            (parts, "+")
        } else {
            let mut parts: Vec<&str> = trimmed.split('+').collect();
            let last = parts.pop().unwrap_or("");
            (parts, last)
        };

        if key_part.is_empty() {
            return Err(format!("Missing key in chord '{spec}'"));
        }

        let mut mods = KeyModifiers::NONE;
        for m in mod_parts {
            if m.is_empty() {
                return Err(format!("Invalid '+' separator in chord '{spec}'"));
            }
            match m.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "c" => mods |= KeyModifiers::CONTROL,
                "alt" | "meta" | "opt" | "option" | "m" => mods |= KeyModifiers::ALT,
                "shift" | "s" => mods |= KeyModifiers::SHIFT,
                other => return Err(format!("Unknown modifier '{other}' in chord '{spec}'")),
            }
        }

        let key_lower = key_part.trim().to_ascii_lowercase();
        let code = match key_lower.as_str() {
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "enter" | "return" | "ret" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "space" | "spc" => KeyCode::Char(' '),
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" | "pgdown" => KeyCode::PageDown,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            _ if key_lower.starts_with('f') && key_lower.len() >= 2 && key_lower[1..].bytes().all(|b| b.is_ascii_digit()) => {
                let n: u8 = key_lower[1..]
                    .parse()
                    .map_err(|_| format!("Invalid function key '{key_part}' in '{spec}'"))?;
                if !(1..=12).contains(&n) {
                    return Err(format!("Function key out of range (F1..F12): '{key_part}'"));
                }
                KeyCode::F(n)
            }
            _ => {
                let mut chars = key_part.chars();
                let Some(ch) = chars.next() else {
                    return Err(format!("Empty key in chord '{spec}'"));
                };
                if chars.next().is_some() {
                    return Err(format!("Invalid key '{key_part}' in chord '{spec}'"));
                }
                KeyCode::Char(ch)
            }
        };

        Ok(Self::normalize(code, mods))
    }

    pub fn canonical_string(&self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("ctrl".to_string());
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("alt".to_string());
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("shift".to_string());
        }
        let key_str = match self.code {
            KeyCode::Tab => "tab".to_string(),
            KeyCode::BackTab => "backtab".to_string(),
            KeyCode::Enter => "enter".to_string(),
            KeyCode::Esc => "esc".to_string(),
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char(c) => {
                if self.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                    c.to_ascii_lowercase().to_string()
                } else {
                    c.to_string()
                }
            }
            KeyCode::Backspace => "backspace".to_string(),
            KeyCode::Delete => "delete".to_string(),
            KeyCode::Insert => "insert".to_string(),
            KeyCode::PageUp => "pageup".to_string(),
            KeyCode::PageDown => "pagedown".to_string(),
            KeyCode::Home => "home".to_string(),
            KeyCode::End => "end".to_string(),
            KeyCode::Up => "up".to_string(),
            KeyCode::Down => "down".to_string(),
            KeyCode::Left => "left".to_string(),
            KeyCode::Right => "right".to_string(),
            KeyCode::F(n) => format!("f{n}"),
            other => format!("{other:?}").to_ascii_lowercase(),
        };
        parts.push(key_str);
        parts.join("+")
    }

    pub fn display_label(&self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("Ctrl".to_string());
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("Alt".to_string());
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("Shift".to_string());
        }
        let key_str = match self.code {
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::BackTab => "Shift+Tab".to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char(c) => {
                if self.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                    c.to_ascii_uppercase().to_string()
                } else {
                    c.to_string()
                }
            }
            KeyCode::Backspace => "Backspace".to_string(),
            KeyCode::Delete => "Del".to_string(),
            KeyCode::Insert => "Ins".to_string(),
            KeyCode::PageUp => "PgUp".to_string(),
            KeyCode::PageDown => "PgDn".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::Up => "↑".to_string(),
            KeyCode::Down => "↓".to_string(),
            KeyCode::Left => "←".to_string(),
            KeyCode::Right => "→".to_string(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        };
        parts.push(key_str);
        parts.join("+")
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_label())
    }
}

impl FromStr for KeyChord {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BindableAction {
    EnterManageMode,
    ToggleZoom,
    ScrollUp,
    ScrollDown,
    PrevSibling,
    NextSibling,
    TreeUp,
    TreeDown,
    TreeParent,
    TreeChild,
    ToggleWorktrees,
    JumpTerminal,
    NewAgent,
    ForkThread,
    Worktree,
    CommandPalette,
    Help,
    ToggleCards,
    CopyReply,
    Quit,
    AcceptCompletion,
    NextCompletion,
    PrevCompletion,
    DismissCompletion,
}

impl BindableAction {
    pub fn all() -> &'static [BindableAction] {
        &[
            Self::EnterManageMode,
            Self::ToggleZoom,
            Self::ScrollUp,
            Self::ScrollDown,
            Self::PrevSibling,
            Self::NextSibling,
            Self::TreeUp,
            Self::TreeDown,
            Self::TreeParent,
            Self::TreeChild,
            Self::ToggleWorktrees,
            Self::JumpTerminal,
            Self::NewAgent,
            Self::ForkThread,
            Self::Worktree,
            Self::CommandPalette,
            Self::Help,
            Self::ToggleCards,
            Self::CopyReply,
            Self::Quit,
            Self::AcceptCompletion,
            Self::NextCompletion,
            Self::PrevCompletion,
            Self::DismissCompletion,
        ]
    }

    pub fn is_composer_completion(self) -> bool {
        matches!(
            self,
            Self::AcceptCompletion
                | Self::NextCompletion
                | Self::PrevCompletion
                | Self::DismissCompletion
        )
    }

    pub fn is_write_action(self) -> bool {
        !self.is_composer_completion()
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EnterManageMode => "enter_manage_mode",
            Self::ToggleZoom => "toggle_zoom",
            Self::ScrollUp => "scroll_up",
            Self::ScrollDown => "scroll_down",
            Self::PrevSibling => "prev_sibling",
            Self::NextSibling => "next_sibling",
            Self::TreeUp => "tree_up",
            Self::TreeDown => "tree_down",
            Self::TreeParent => "tree_parent",
            Self::TreeChild => "tree_child",
            Self::ToggleWorktrees => "toggle_worktrees",
            Self::JumpTerminal => "jump_terminal",
            Self::NewAgent => "new_agent",
            Self::ForkThread => "fork_thread",
            Self::Worktree => "worktree",
            Self::CommandPalette => "command_palette",
            Self::Help => "help",
            Self::ToggleCards => "toggle_cards",
            Self::CopyReply => "copy_reply",
            Self::Quit => "quit",
            Self::AcceptCompletion => "accept_completion",
            Self::NextCompletion => "next_completion",
            Self::PrevCompletion => "prev_completion",
            Self::DismissCompletion => "dismiss_completion",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::EnterManageMode => "Toggle WRITE <-> MANAGE sidebar",
            Self::ToggleZoom => "Toggle sidebar zoom",
            Self::ScrollUp => "Scroll terminal/transcript up",
            Self::ScrollDown => "Scroll terminal/transcript down",
            Self::PrevSibling => "Switch to previous sibling thread",
            Self::NextSibling => "Switch to next sibling thread",
            Self::TreeUp => "Switch to previous sibling thread in tree",
            Self::TreeDown => "Switch to next sibling thread in tree",
            Self::TreeParent => "Switch to parent thread in tree",
            Self::TreeChild => "Switch to child thread in tree",
            Self::ToggleWorktrees => "Toggle Grein <-> Worktrees view",
            Self::JumpTerminal => "Open or jump to terminal",
            Self::NewAgent => "Create new root agent thread",
            Self::ForkThread => "Fork active thread",
            Self::Worktree => "Ensure thread worktree",
            Self::CommandPalette => "Open sidebar command palette",
            Self::Help => "Open help modal",
            Self::ToggleCards => "Toggle tool/shell cards",
            Self::CopyReply => "Copy last reply or terminal screen",
            Self::Quit => "Quit Grein",
            Self::AcceptCompletion => "Accept inline Composer completion",
            Self::NextCompletion => "Next inline Composer completion",
            Self::PrevCompletion => "Previous inline Composer completion",
            Self::DismissCompletion => "Dismiss inline Composer completion",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "enter_manage_mode" | "manage" | "tree" | "toggle_tree" | "manage_mode" => {
                Some(Self::EnterManageMode)
            }
            "toggle_zoom" | "zoom" => Some(Self::ToggleZoom),
            "scroll_up" | "page_up" | "scrollback_up" => Some(Self::ScrollUp),
            "scroll_down" | "page_down" | "scrollback_down" => Some(Self::ScrollDown),
            "prev_sibling" | "previous_sibling" | "sibling_prev" => Some(Self::PrevSibling),
            "next_sibling" | "sibling_next" => Some(Self::NextSibling),
            "tree_up" | "tree_prev" | "sibling_up" => Some(Self::TreeUp),
            "tree_down" | "tree_next" | "sibling_down" => Some(Self::TreeDown),
            "tree_parent" | "tree_up_level" | "parent_thread" => Some(Self::TreeParent),
            "tree_child" | "tree_down_level" | "child_thread" => Some(Self::TreeChild),
            "toggle_worktrees" | "worktrees" | "wt_view" => Some(Self::ToggleWorktrees),
            "jump_terminal" | "terminal" | "term" => Some(Self::JumpTerminal),
            "new_agent" | "create_root" | "new_root" | "new" => Some(Self::NewAgent),
            "fork_thread" | "fork" | "branch" => Some(Self::ForkThread),
            "worktree" | "ensure_worktree" | "wt" => Some(Self::Worktree),
            "command_palette" | "palette" | "cmd" => Some(Self::CommandPalette),
            "help" | "show_help" => Some(Self::Help),
            "toggle_cards" | "cards" => Some(Self::ToggleCards),
            "copy_reply" | "copy" => Some(Self::CopyReply),
            "quit" | "exit" => Some(Self::Quit),
            "accept_completion" | "complete" | "completion_accept" => Some(Self::AcceptCompletion),
            "next_completion" | "completion_next" => Some(Self::NextCompletion),
            "prev_completion" | "previous_completion" | "completion_prev" => {
                Some(Self::PrevCompletion)
            }
            "dismiss_completion" | "completion_dismiss" => Some(Self::DismissCompletion),
            _ => None,
        }
    }

    pub fn default_chords(&self) -> Vec<KeyChord> {
        match self {
            Self::EnterManageMode => vec![KeyChord::new(KeyCode::Char('g'), KeyModifiers::CONTROL)],
            Self::ToggleZoom => vec![KeyChord::new(KeyCode::Char('z'), KeyModifiers::ALT)],
            Self::ScrollUp => vec![KeyChord::new(KeyCode::Char('k'), KeyModifiers::ALT)],
            Self::ScrollDown => vec![KeyChord::new(KeyCode::Char('j'), KeyModifiers::ALT)],
            Self::PrevSibling => vec![KeyChord::new(KeyCode::Char('h'), KeyModifiers::ALT)],
            Self::NextSibling => vec![KeyChord::new(KeyCode::Char('l'), KeyModifiers::ALT)],
            Self::TreeUp => vec![KeyChord::new(KeyCode::Up, KeyModifiers::SHIFT)],
            Self::TreeDown => vec![KeyChord::new(KeyCode::Down, KeyModifiers::SHIFT)],
            Self::TreeParent => vec![KeyChord::new(KeyCode::Left, KeyModifiers::SHIFT)],
            Self::TreeChild => vec![KeyChord::new(KeyCode::Right, KeyModifiers::SHIFT)],
            Self::AcceptCompletion => vec![
                KeyChord::new(KeyCode::Tab, KeyModifiers::NONE),
                KeyChord::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
            ],
            Self::NextCompletion => vec![
                KeyChord::new(KeyCode::BackTab, KeyModifiers::NONE),
                KeyChord::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            ],
            Self::PrevCompletion => vec![KeyChord::new(KeyCode::Char('p'), KeyModifiers::CONTROL)],
            Self::DismissCompletion => {
                vec![KeyChord::new(KeyCode::Char('e'), KeyModifiers::CONTROL)]
            }
            _ => Vec::new(),
        }
    }
}

impl FromStr for BindableAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s).ok_or_else(|| format!("Unknown bindable action '{s}'"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    bindings: BTreeMap<BindableAction, Vec<KeyChord>>,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut bindings = BTreeMap::new();
        for &action in BindableAction::all() {
            let defaults = action.default_chords();
            if !defaults.is_empty() {
                bindings.insert(action, defaults);
            }
        }
        Self { bindings }
    }
}

impl Keymap {
    pub fn from_prefs(prefs: &BTreeMap<String, String>) -> Self {
        let mut keymap = Self::default();
        for (action_name, spec) in prefs {
            if let Some(action) = BindableAction::from_str(action_name) {
                let _ = keymap.bind(action, spec);
            }
        }
        keymap
    }

    pub fn parse_chord_list(spec: &str) -> Result<Vec<KeyChord>, String> {
        let trimmed = spec.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("none")
            || trimmed.eq_ignore_ascii_case("unbound")
        {
            return Ok(Vec::new());
        }
        let mut chords = Vec::new();
        for part in trimmed.split(',') {
            let chord = KeyChord::parse(part)?;
            if !chords.contains(&chord) {
                chords.push(chord);
            }
        }
        Ok(chords)
    }

    pub fn bind(&mut self, action: BindableAction, spec: &str) -> Result<String, String> {
        let trimmed = spec.trim();
        if trimmed.eq_ignore_ascii_case("default") {
            self.unbind(action);
            return Ok(self.canonical_for(action));
        }
        let chords = Self::parse_chord_list(trimmed)?;
        if action == BindableAction::EnterManageMode && chords.is_empty() {
            return Err("enter_manage_mode must keep at least one binding".to_string());
        }
        for chord in &chords {
            if let Some(manage_chords) = self.bindings.get(&BindableAction::EnterManageMode) {
                if action != BindableAction::EnterManageMode
                    && manage_chords.contains(chord)
                    && manage_chords.len() == 1
                {
                    return Err("enter_manage_mode must keep at least one binding".to_string());
                }
            }
        }
        for chord in &chords {
            for (&other_action, other_chords) in &mut self.bindings {
                if other_action != action {
                    other_chords.retain(|c| c != chord);
                }
            }
        }
        self.bindings.retain(|_, list| !list.is_empty());
        if chords.is_empty() {
            self.bindings.remove(&action);
        } else {
            self.bindings.insert(action, chords);
        }
        Ok(self.canonical_for(action))
    }

    pub fn unbind(&mut self, action: BindableAction) {
        let defaults = action.default_chords();
        if defaults.is_empty() {
            self.bindings.remove(&action);
        } else {
            self.bindings.insert(action, defaults);
        }
    }

    fn match_key_where<F>(&self, key: KeyEvent, predicate: F) -> Option<BindableAction>
    where
        F: Fn(BindableAction) -> bool,
    {
        let ev = KeyChord::from_event(key)?;
        for (&action, chords) in &self.bindings {
            if predicate(action) && chords.contains(&ev) {
                return Some(action);
            }
        }
        None
    }

    pub fn match_write_key(&self, key: KeyEvent) -> Option<BindableAction> {
        self.match_key_where(key, BindableAction::is_write_action)
    }

    pub fn match_composer_completion_key(&self, key: KeyEvent) -> Option<BindableAction> {
        self.match_key_where(key, BindableAction::is_composer_completion)
    }

    pub fn chords_for(&self, action: BindableAction) -> &[KeyChord] {
        self.bindings
            .get(&action)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn display_for(&self, action: BindableAction) -> String {
        let chords = self.chords_for(action);
        if chords.is_empty() {
            return "unbound".to_string();
        }
        chords
            .iter()
            .map(KeyChord::display_label)
            .collect::<Vec<_>>()
            .join(" / ")
    }

    pub fn canonical_for(&self, action: BindableAction) -> String {
        let chords = self.chords_for(action);
        if chords.is_empty() {
            return "none".to_string();
        }
        chords
            .iter()
            .map(KeyChord::canonical_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn all_entries(&self) -> Vec<(BindableAction, String)> {
        BindableAction::all()
            .iter()
            .filter_map(|&action| {
                let chords = self.chords_for(action);
                if chords.is_empty() {
                    None
                } else {
                    Some((action, self.display_for(action)))
                }
            })
            .collect()
    }

    pub fn custom_entries(&self) -> Vec<(BindableAction, String)> {
        BindableAction::all()
            .iter()
            .filter_map(|&action| {
                let current = self.chords_for(action);
                let defaults = action.default_chords();
                if current != defaults.as_slice() {
                    Some((action, self.display_for(action)))
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_event_rejects_super_hyper_meta_and_preserves_normalize_flags() {
        let shift_up = KeyChord::new(KeyCode::Up, KeyModifiers::SHIFT);
        let plain_tab = KeyChord::new(KeyCode::Tab, KeyModifiers::NONE);
        let backtab = KeyChord::new(KeyCode::BackTab, KeyModifiers::NONE);
        let upper_a = KeyChord::new(KeyCode::Char('A'), KeyModifiers::NONE);

        for extra in [KeyModifiers::SUPER, KeyModifiers::HYPER, KeyModifiers::META] {
            let mod_shift_up = KeyEvent::new(KeyCode::Up, extra | KeyModifiers::SHIFT);
            assert_eq!(KeyChord::from_event(mod_shift_up), None);
            assert!(!shift_up.matches(mod_shift_up));
            assert_ne!(KeyChord::new(KeyCode::Up, extra | KeyModifiers::SHIFT), shift_up);

            let mod_tab = KeyEvent::new(KeyCode::Tab, extra);
            assert_eq!(KeyChord::from_event(mod_tab), None);
            assert!(!plain_tab.matches(mod_tab));
            assert_ne!(KeyChord::new(KeyCode::Tab, extra), plain_tab);

            let mod_shift_tab = KeyEvent::new(KeyCode::Tab, extra | KeyModifiers::SHIFT);
            assert_eq!(KeyChord::from_event(mod_shift_tab), None);
            assert!(!backtab.matches(mod_shift_tab));
            assert_ne!(KeyChord::new(KeyCode::Tab, extra | KeyModifiers::SHIFT), backtab);

            let mod_shift_a = KeyEvent::new(KeyCode::Char('a'), extra | KeyModifiers::SHIFT);
            assert_eq!(KeyChord::from_event(mod_shift_a), None);
            assert!(!upper_a.matches(mod_shift_a));
            assert_ne!(KeyChord::new(KeyCode::Char('a'), extra | KeyModifiers::SHIFT), upper_a);
        }
    }

    #[test]
    fn from_event_preserves_shift_chars_tab_and_altgr() {
        assert_eq!(
            KeyChord::from_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SHIFT)),
            Some(KeyChord {
                code: KeyCode::Char('A'),
                modifiers: KeyModifiers::NONE,
            })
        );
        assert_eq!(
            KeyChord::from_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)),
            Some(KeyChord {
                code: KeyCode::BackTab,
                modifiers: KeyModifiers::NONE,
            })
        );
        let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;
        assert_eq!(
            KeyChord::from_event(KeyEvent::new(KeyCode::Char('@'), altgr)),
            Some(KeyChord {
                code: KeyCode::Char('@'),
                modifiers: altgr,
            })
        );
        assert_eq!(
            KeyChord::from_event(KeyEvent::new(
                KeyCode::Char('{'),
                altgr | KeyModifiers::SHIFT
            )),
            Some(KeyChord {
                code: KeyCode::Char('{'),
                modifiers: altgr | KeyModifiers::SHIFT,
            })
        );
    }
}
