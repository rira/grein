use std::collections::BTreeMap;
use std::io;
use std::sync::Mutex;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use grein_core::ThreadKind;
use grein_tui::app::{
    App, AppAction, ConfigPickerModal, FocusedPane, PendingPermission, PendingQuitConfirm,
    PendingThreadDelete, PendingWorktreeDelete, RenamePromptState, SidebarLayoutConfig,
    SidebarMode, SidebarView, ThreadItem,
};
use grein_tui::clipboard::{encode_base64, encode_osc52};
use grein_tui::diff_card::{DiffCard, DiffCardAction, DiffLineKind};
use grein_tui::engine_port::{
    AgentConfigChoice, PtyCell, PtyColor, PtyScreenSnapshot, SubagentSnapshot, SubagentStatus,
    ThreadMeta, UserPreferencesSnapshot, WorktreeEntry,
};
use grein_tui::focus::{FocusManager, FocusNavigationAction, PaneFocus as CorePaneFocus};
use grein_tui::keymap::{BindableAction, KeyChord, Keymap};
use grein_tui::markdown::{
    parse_inline_spans, render_ansi_text, render_markdown_lines, render_markdown_lines_with_width,
    wrap_styled_line, wrap_styled_lines,
};
use grein_tui::pager::{DefaultPagerRunner, PagerRunner};
use grein_tui::pane::focus::PaneFocus as LegacyPaneFocus;
use grein_tui::render::{compute_app_layout, render_app, render_app_mut};
use grein_tui::search::{
    highlight_lines_with_matches, SearchDirection, SearchMatch, TranscriptSearchState,
};
use grein_tui::syntax::{highlight_code_line, highlight_code_lines, safe_fence_for};
use grein_tui::theme::{ThemePalette, ThemePickerModal, ThemePreset, ThemeTone};
use grein_tui::transcript::{ThreadTranscript, TranscriptBlock};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use ratatui::Terminal;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn plain(code: KeyCode) -> KeyEvent {
    key(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    key(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn alt(c: char) -> KeyEvent {
    key(KeyCode::Char(c), KeyModifiers::ALT)
}

fn make_meta(id: &str, parent: Option<&str>, kind: ThreadKind) -> ThreadMeta {
    ThreadMeta {
        thread_id: id.to_string(),
        parent_thread_id: parent.map(str::to_string),
        inherited_turn_count: 0,
        merged: false,
        kind,
        cwd: None,
        git_branch: None,
        git_dirty: 0,
        custom_name: None,
    }
}

fn seed_two_threads(app: &mut App) {
    app.active_thread_id = "t1".to_string();
    app.threads = vec![
        ThreadItem {
            id: "t1".to_string(),
            title: "Root One".to_string(),
            custom_name: None,
            anchors: Vec::new(),
        },
        ThreadItem {
            id: "t2".to_string(),
            title: "Child Branch".to_string(),
            custom_name: None,
            anchors: Vec::new(),
        },
    ];
    app.thread_meta
        .insert("t1".to_string(), make_meta("t1", None, ThreadKind::Agent));
    app.thread_meta.insert(
        "t2".to_string(),
        make_meta("t2", Some("t1"), ThreadKind::Agent),
    );
    app.thread_transcripts
        .insert("t1".to_string(), "# User: hi\n\n## Assistant\nHello `world`\n```rs\nfn ok() {}\n```\n".to_string());
    app.active_transcript_md = app.thread_transcripts["t1"].clone();
    app.sync_anchors();
    app.select_active_thread_in_tree();
}

fn render_to_string(app: &mut App, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_app_mut(f, app)).unwrap();
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn test_keymap_parsing_aliases_formatting_and_prefs() {
    let plus = KeyChord::parse("+").unwrap();
    assert_eq!(plus, KeyChord::new(KeyCode::Char('+'), KeyModifiers::NONE));
    assert_eq!(plus.canonical_string(), "+");
    assert_eq!(plus.display_label(), "+");
    assert_eq!(plus.to_string(), "+");

    let ctrl_plus = KeyChord::parse("ctrl++").unwrap();
    assert_eq!(
        ctrl_plus,
        KeyChord::new(KeyCode::Char('+'), KeyModifiers::CONTROL)
    );
    assert_eq!(ctrl_plus.canonical_string(), "ctrl++");
    assert_eq!(ctrl_plus.display_label(), "Ctrl++");

    let shift_tab = KeyChord::parse("shift+tab").unwrap();
    assert_eq!(shift_tab.code, KeyCode::BackTab);
    assert_eq!(shift_tab.modifiers, KeyModifiers::NONE);
    assert_eq!(shift_tab.canonical_string(), "backtab");
    assert_eq!(shift_tab.display_label(), "Shift+Tab");
    assert!(shift_tab.matches(key(KeyCode::Tab, KeyModifiers::SHIFT)));
    assert!(shift_tab.matches(plain(KeyCode::BackTab)));

    let ctrl_shift_tab = KeyChord::parse("ctrl+shift+tab").unwrap();
    assert_eq!(ctrl_shift_tab.code, KeyCode::Tab);
    assert_eq!(
        ctrl_shift_tab.modifiers,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT
    );
    assert_eq!(ctrl_shift_tab.canonical_string(), "ctrl+shift+tab");
    assert_eq!(ctrl_shift_tab.display_label(), "Ctrl+Shift+Tab");

    assert_eq!(
        KeyChord::parse("control+c").unwrap(),
        KeyChord::parse("c+c").unwrap()
    );
    assert_eq!(
        KeyChord::parse("meta+x").unwrap(),
        KeyChord::parse("opt+x").unwrap()
    );
    assert_eq!(
        KeyChord::parse("option+x").unwrap(),
        KeyChord::parse("m+x").unwrap()
    );
    let s_upper = KeyChord::parse("s+a").unwrap();
    assert_eq!(s_upper.code, KeyCode::Char('A'));
    assert_eq!(s_upper.modifiers, KeyModifiers::NONE);

    let ctrl_shift_a = KeyChord::parse("ctrl+shift+a").unwrap();
    assert_eq!(ctrl_shift_a.canonical_string(), "ctrl+shift+a");
    assert_eq!(ctrl_shift_a.display_label(), "Ctrl+Shift+A");

    let special_cases = [
        ("return", KeyCode::Enter, "enter", "Enter"),
        ("ret", KeyCode::Enter, "enter", "Enter"),
        ("escape", KeyCode::Esc, "esc", "Esc"),
        ("space", KeyCode::Char(' '), "space", "Space"),
        ("spc", KeyCode::Char(' '), "space", "Space"),
        ("bs", KeyCode::Backspace, "backspace", "Backspace"),
        ("del", KeyCode::Delete, "delete", "Del"),
        ("ins", KeyCode::Insert, "insert", "Ins"),
        ("pgup", KeyCode::PageUp, "pageup", "PgUp"),
        ("pgdn", KeyCode::PageDown, "pagedown", "PgDn"),
        ("home", KeyCode::Home, "home", "Home"),
        ("end", KeyCode::End, "end", "End"),
        ("up", KeyCode::Up, "up", "↑"),
        ("down", KeyCode::Down, "down", "↓"),
        ("left", KeyCode::Left, "left", "←"),
        ("right", KeyCode::Right, "right", "→"),
    ];
    for (spec, code, canon, disp) in special_cases {
        let parsed: KeyChord = spec.parse().unwrap();
        assert_eq!(parsed.code, code, "spec {spec}");
        assert_eq!(parsed.canonical_string(), canon, "canon {spec}");
        assert_eq!(parsed.display_label(), disp, "disp {spec}");
    }

    for n in 1..=12u8 {
        let chord = KeyChord::parse(&format!("f{n}")).unwrap();
        assert_eq!(chord.code, KeyCode::F(n));
        assert_eq!(chord.canonical_string(), format!("f{n}"));
        assert_eq!(chord.display_label(), format!("F{n}"));
    }

    for invalid in [
        "",
        "   ",
        "ctrl+",
        "+a",
        "ctrl++a",
        "hyper+a",
        "f0",
        "f13",
        "f999",
        "ctrl+ab",
    ] {
        assert!(KeyChord::parse(invalid).is_err(), "expected err for {invalid:?}");
    }

    assert!(Keymap::parse_chord_list("").unwrap().is_empty());
    assert!(Keymap::parse_chord_list("none").unwrap().is_empty());
    assert!(Keymap::parse_chord_list("UNBOUND").unwrap().is_empty());
    let dedup = Keymap::parse_chord_list("ctrl+a, c+a, alt+b").unwrap();
    assert_eq!(dedup.len(), 2);
    assert!(Keymap::parse_chord_list("ctrl+a, bad_mod+b").is_err());

    let action_aliases = [
        ("enter_manage_mode", BindableAction::EnterManageMode),
        ("manage", BindableAction::EnterManageMode),
        ("tree", BindableAction::EnterManageMode),
        ("toggle_tree", BindableAction::EnterManageMode),
        ("manage_mode", BindableAction::EnterManageMode),
        ("zoom", BindableAction::ToggleZoom),
        ("page_up", BindableAction::ScrollUp),
        ("scrollback_up", BindableAction::ScrollUp),
        ("page_down", BindableAction::ScrollDown),
        ("scrollback_down", BindableAction::ScrollDown),
        ("previous_sibling", BindableAction::PrevSibling),
        ("sibling_prev", BindableAction::PrevSibling),
        ("sibling_next", BindableAction::NextSibling),
        ("worktrees", BindableAction::ToggleWorktrees),
        ("wt_view", BindableAction::ToggleWorktrees),
        ("terminal", BindableAction::JumpTerminal),
        ("term", BindableAction::JumpTerminal),
        ("create_root", BindableAction::NewAgent),
        ("new_root", BindableAction::NewAgent),
        ("new", BindableAction::NewAgent),
        ("fork", BindableAction::ForkThread),
        ("branch", BindableAction::ForkThread),
        ("ensure_worktree", BindableAction::Worktree),
        ("wt", BindableAction::Worktree),
        ("palette", BindableAction::CommandPalette),
        ("cmd", BindableAction::CommandPalette),
        ("show_help", BindableAction::Help),
        ("cards", BindableAction::ToggleCards),
        ("copy", BindableAction::CopyReply),
        ("exit", BindableAction::Quit),
        ("complete", BindableAction::AcceptCompletion),
        ("completion_accept", BindableAction::AcceptCompletion),
        ("completion_next", BindableAction::NextCompletion),
        ("previous_completion", BindableAction::PrevCompletion),
        ("completion_prev", BindableAction::PrevCompletion),
        ("completion_dismiss", BindableAction::DismissCompletion),
    ];
    for (alias, expected) in action_aliases {
        assert_eq!(BindableAction::from_str(alias), Some(expected), "{alias}");
        assert_eq!(alias.parse::<BindableAction>().unwrap(), expected, "{alias}");
        assert!(!expected.description().is_empty());
    }
    assert!("not_an_action".parse::<BindableAction>().is_err());

    let mut prefs = BTreeMap::new();
    prefs.insert("unknown_action".to_string(), "ctrl+x".to_string());
    prefs.insert("new_agent".to_string(), "invalid++key".to_string());
    prefs.insert("help".to_string(), "f1".to_string());
    let mut km = Keymap::from_prefs(&prefs);
    assert_eq!(km.display_for(BindableAction::NewAgent), "unbound");
    assert_eq!(km.canonical_for(BindableAction::NewAgent), "none");
    assert_eq!(km.display_for(BindableAction::Help), "F1");
    assert!(km
        .all_entries()
        .iter()
        .any(|(a, d)| *a == BindableAction::Help && d == "F1"));
    assert_eq!(
        km.custom_entries(),
        vec![(BindableAction::Help, "F1".to_string())]
    );

    assert!(km.bind(BindableAction::EnterManageMode, "none").is_err());
    assert!(km.bind(BindableAction::Help, "ctrl+g").is_err());
    assert_eq!(km.bind(BindableAction::Help, "default").unwrap(), "none");
    assert!(km.custom_entries().is_empty());
}

#[test]
fn test_theme_color_parsing_formatting_and_presets() {
    assert_eq!(ThemePalette::parse_color("#abc"), Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
    assert_eq!(
        ThemePalette::parse_color(" #1a2b3c "),
        Some(Color::Rgb(0x1a, 0x2b, 0x3c))
    );
    assert_eq!(ThemePalette::parse_color("indexed(0)"), Some(Color::Indexed(0)));
    assert_eq!(
        ThemePalette::parse_color("indexed( 255 )"),
        Some(Color::Indexed(255))
    );
    assert_eq!(ThemePalette::parse_color("indexed(256)"), None);
    assert_eq!(ThemePalette::parse_color("indexed(-1)"), None);
    assert_eq!(ThemePalette::parse_color("indexed(abc)"), None);

    for bad_hex in ["#", "#12", "#1234", "#1234567", "#zzzzzz", "#ggg", "#ééé"] {
        assert_eq!(ThemePalette::parse_color(bad_hex), None, "{bad_hex}");
    }

    let named = [
        ("black", Color::Black, "black"),
        ("red", Color::Red, "red"),
        ("green", Color::Green, "green"),
        ("yellow", Color::Yellow, "yellow"),
        ("blue", Color::Blue, "blue"),
        ("magenta", Color::Magenta, "magenta"),
        ("cyan", Color::Cyan, "cyan"),
        ("white", Color::White, "white"),
        ("gray", Color::Gray, "gray"),
        ("grey", Color::Gray, "gray"),
        ("dark_gray", Color::DarkGray, "dark_gray"),
        ("darkgray", Color::DarkGray, "dark_gray"),
        ("dark_grey", Color::DarkGray, "dark_gray"),
        ("darkgrey", Color::DarkGray, "dark_gray"),
        ("light_red", Color::LightRed, "light_red"),
        ("lightred", Color::LightRed, "light_red"),
        ("light_green", Color::LightGreen, "light_green"),
        ("lightgreen", Color::LightGreen, "light_green"),
        ("light_yellow", Color::LightYellow, "light_yellow"),
        ("lightyellow", Color::LightYellow, "light_yellow"),
        ("light_blue", Color::LightBlue, "light_blue"),
        ("lightblue", Color::LightBlue, "light_blue"),
        ("light_magenta", Color::LightMagenta, "light_magenta"),
        ("lightmagenta", Color::LightMagenta, "light_magenta"),
        ("light_cyan", Color::LightCyan, "light_cyan"),
        ("lightcyan", Color::LightCyan, "light_cyan"),
        ("reset", Color::Reset, "reset"),
        ("default", Color::Reset, "reset"),
    ];
    for (input, color, formatted) in named {
        assert_eq!(ThemePalette::parse_color(input), Some(color), "{input}");
        assert_eq!(ThemePalette::format_color(color), formatted, "{input}");
    }
    assert_eq!(ThemePalette::parse_color("chartreuse"), None);
    assert_eq!(
        ThemePalette::format_color(Color::Indexed(42)),
        "indexed(42)"
    );
    assert_eq!(
        ThemePalette::format_color(Color::Rgb(1, 2, 3)),
        "#010203"
    );

    let mut overrides = BTreeMap::new();
    overrides.insert("name".to_string(), "nord".to_string());
    overrides.insert("bg".to_string(), "#112233".to_string());
    overrides.insert("text".to_string(), "invalid_color".to_string());
    overrides.insert("unknown_field".to_string(), "#ffffff".to_string());
    let pal = ThemePalette::from_prefs(None, &overrides);
    assert_eq!(pal.bg, Color::Rgb(0x11, 0x22, 0x33));
    assert_eq!(pal.to_color_map("nord").get("bg").map(String::as_str), Some("#112233"));

    assert_eq!("ansi".parse::<ThemePreset>().unwrap().tone(), ThemeTone::Terminal);
    assert_eq!("dawn".parse::<ThemePreset>().unwrap().tone(), ThemeTone::Light);
    assert!("nope".parse::<ThemePreset>().is_err());
    assert_eq!(ThemePalette::from_preset(ThemePreset::Ansi).swatch_colors().len(), 5);
}

#[test]
fn test_sidebar_commands_layout_prefs_and_bind_actions() {
    let _guard = ENV_LOCK.lock().unwrap();
    let mut app = App::new();
    seed_two_threads(&mut app);

    app.execute_sidebar_command("sidebar");
    assert!(app.status_notice.as_deref().unwrap().contains("dynamic"));

    app.execute_sidebar_command("sidebar dynamic 45% 20%");
    assert_eq!(app.sidebar_layout.mode, SidebarMode::Dynamic);
    assert_eq!(app.sidebar_layout.min_pct, 45);
    assert_eq!(app.sidebar_layout.max_pct, 45);

    app.execute_sidebar_command("sidebar auto 5 90");
    assert_eq!(app.sidebar_layout.min_pct, 10);
    assert_eq!(app.sidebar_layout.max_pct, 60);

    app.execute_sidebar_command("sidebar fixed 99%");
    assert_eq!(app.sidebar_layout.mode, SidebarMode::Fixed);
    assert_eq!(app.sidebar_layout.fixed_pct, Some(60));

    app.execute_sidebar_command("sidebar fixed 12");
    assert_eq!(app.sidebar_layout.fixed_cols, 16);
    assert_eq!(app.sidebar_layout.fixed_pct, None);

    app.execute_sidebar_command("sidebar min");
    assert!(app.status_notice.as_deref().unwrap().contains("Usage"));
    app.execute_sidebar_command("sidebar min bad");
    assert!(app.status_notice.as_deref().unwrap().contains("Usage"));
    app.execute_sidebar_command("sidebar min 55%");
    assert_eq!(app.sidebar_layout.min_pct, 50);

    app.execute_sidebar_command("sidebar max");
    assert!(app.status_notice.as_deref().unwrap().contains("Usage"));
    app.execute_sidebar_command("sidebar max bad");
    assert!(app.status_notice.as_deref().unwrap().contains("Usage"));
    app.execute_sidebar_command("sidebar max 20%");
    assert_eq!(app.sidebar_layout.max_pct, 50);

    app.execute_sidebar_command("sidebar overflow off");
    assert!(!app.sidebar_layout.overflow);
    app.execute_sidebar_command("sidebar overflow");
    assert!(app.sidebar_layout.overflow);
    app.execute_sidebar_command("sidebar overflow bogus");
    assert!(app.status_notice.as_deref().unwrap().contains("Usage"));

    app.execute_sidebar_command("sidebar 35%");
    assert_eq!(app.sidebar_layout.fixed_pct, Some(35));
    app.execute_sidebar_command("sidebar 28");
    assert_eq!(app.sidebar_layout.fixed_cols, 28);
    assert_eq!(app.sidebar_layout.fixed_pct, None);
    app.execute_sidebar_command("sidebar bad%");
    assert!(app.status_notice.as_deref().unwrap().contains("Usage"));
    app.execute_sidebar_command("sidebar bogus");
    assert!(app.status_notice.as_deref().unwrap().contains("Usage"));

    let mut cfg = SidebarLayoutConfig::default();
    let mut ui = BTreeMap::new();
    ui.insert("sidebar_mode".to_string(), "fixed".to_string());
    ui.insert("sidebar_min_pct".to_string(), "40%".to_string());
    ui.insert("sidebar_max_pct".to_string(), "20%".to_string());
    ui.insert("sidebar_fixed_cols".to_string(), "200".to_string());
    ui.insert("sidebar_fixed_pct".to_string(), "25%".to_string());
    ui.insert("sidebar_overflow".to_string(), "off".to_string());
    cfg.apply_ui_prefs(&ui);
    assert_eq!(cfg.mode, SidebarMode::Fixed);
    assert_eq!(cfg.min_pct, 40);
    assert_eq!(cfg.max_pct, 40);
    assert_eq!(cfg.fixed_cols, 80);
    assert_eq!(cfg.fixed_pct, Some(25));
    assert!(!cfg.overflow);
    ui.insert("sidebar_fixed_pct".to_string(), "".to_string());
    ui.insert("sidebar_mode".to_string(), "dynamic".to_string());
    ui.insert("sidebar_overflow".to_string(), "true".to_string());
    cfg.apply_ui_prefs(&ui);
    assert_eq!(cfg.fixed_pct, None);
    assert_eq!(cfg.mode, SidebarMode::Dynamic);
    assert!(cfg.overflow);

    app.pending_actions.clear();
    app.execute_sidebar_command("bind");
    assert!(app.status_notice.as_deref().unwrap().contains("WRITE keybinds"));
    app.execute_sidebar_command("keybinds custom");
    assert_eq!(
        app.status_notice.as_deref(),
        Some("No custom keybindings; defaults active")
    );
    app.execute_sidebar_command("bind nope alt+n");
    assert!(app.status_notice.as_deref().unwrap().contains("Unknown action"));
    app.execute_sidebar_command("bind help");
    assert!(app.status_notice.as_deref().unwrap().contains("usage: :bind help"));
    app.execute_sidebar_command("bind enter_manage_mode none");
    assert!(app.status_notice.as_deref().unwrap().contains("Invalid keybind"));
    app.execute_sidebar_command("unbind");
    assert_eq!(app.status_notice.as_deref(), Some("Usage: :unbind <action>"));
    app.execute_sidebar_command("unbind nope");
    assert_eq!(app.status_notice.as_deref(), Some("Unknown action 'nope'"));

    app.execute_sidebar_command("bind new_agent f2");
    app.execute_sidebar_command("bind fork_thread f3");
    app.execute_sidebar_command("bind worktree f4");
    app.execute_sidebar_command("bind help f5");
    app.execute_sidebar_command("bind toggle_cards f6");
    app.execute_sidebar_command("bind copy_reply f7");
    app.execute_sidebar_command("bind quit f8");
    app.execute_sidebar_command("keybinds custom");
    assert!(app.status_notice.as_deref().unwrap().contains("Custom keybinds:"));
    app.pending_actions.clear();

    app.set_focus(FocusedPane::Composer);
    assert!(app.handle_key_event(plain(KeyCode::F(2))));
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::CreateRoot { .. })
    ));
    assert!(app.handle_key_event(plain(KeyCode::F(3))));
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::ForkBranch { .. })
    ));
    assert!(app.handle_key_event(plain(KeyCode::F(4))));
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::EnsureThreadWorktree { .. })
    ));
    assert!(app.handle_key_event(plain(KeyCode::F(5))));
    assert!(app.show_help_modal);
    assert!(app.handle_key_event(plain(KeyCode::Esc)));
    assert!(!app.show_help_modal);
    assert!(app.handle_key_event(plain(KeyCode::F(6))));
    assert_eq!(
        app.status_notice.as_deref(),
        Some("No collapsible tool/shell cards")
    );
    assert!(app.handle_key_event(plain(KeyCode::F(7))));
    assert_eq!(app.last_copied_text.as_deref(), Some("Hello `world`\n```rs\nfn ok() {}\n```"));
    assert!(app.handle_key_event(plain(KeyCode::F(8))));
    assert!(app.should_quit());

    app.should_quit = false;
    app.execute_sidebar_command("unbind new_agent");
    assert_eq!(app.keymap.display_for(BindableAction::NewAgent), "unbound");
}

#[test]
fn test_palette_paste_modals_rename_theme_picker_mouse_and_legacy_transcript() {
    let mut app = App::new();
    seed_two_threads(&mut app);

    // Palette readline editing, UTF-8/AltGr, completion, cycling, backspace-close
    app.open_command_palette("th");
    assert!(app.palette_active);
    app.handle_key_event(ctrl('a'));
    assert_eq!(app.palette_cursor, 0);
    app.handle_key_event(ctrl('e'));
    assert_eq!(app.palette_cursor, 2);
    app.handle_key_event(plain(KeyCode::Left));
    assert_eq!(app.palette_cursor, 1);
    app.handle_key_event(plain(KeyCode::Right));
    assert_eq!(app.palette_cursor, 2);
    app.handle_key_event(key(KeyCode::Char('@'), KeyModifiers::CONTROL | KeyModifiers::ALT));
    assert_eq!(app.palette_input, "th@");
    app.handle_key_event(plain(KeyCode::Left));
    app.handle_key_event(plain(KeyCode::Delete));
    assert_eq!(app.palette_input, "th");
    app.handle_key_event(plain(KeyCode::Down));
    app.handle_key_event(ctrl('n'));
    app.handle_key_event(plain(KeyCode::Up));
    app.handle_key_event(ctrl('p'));
    app.handle_key_event(plain(KeyCode::Tab));
    assert_eq!(app.palette_input, "theme");
    app.handle_key_event(ctrl('w'));
    assert_eq!(app.palette_input, "");
    app.handle_key_event(plain(KeyCode::Char('ä')));
    app.handle_key_event(plain(KeyCode::Char('b')));
    app.handle_key_event(ctrl('u'));
    assert_eq!(app.palette_input, "");
    app.handle_key_event(plain(KeyCode::Backspace));
    assert!(!app.palette_active);

    // Paste event: modal blocks, Composer mid-UTF-8 clamps, Tree ignores
    app.set_focus(FocusedPane::Composer);
    app.input_buffer = "aåb".to_string();
    app.cursor_position = 2; // inside 2-byte 'å'
    app.show_help_modal = true;
    assert!(!app.handle_paste_event("X"));
    assert_eq!(app.input_buffer, "aåb");
    app.show_help_modal = false;
    assert!(app.handle_paste_event("Z"));
    assert_eq!(app.input_buffer, "aZåb");
    app.set_focus(FocusedPane::Tree);
    assert!(!app.handle_paste_event("nope"));

    // PendingThreadDelete (confirm & cancel)
    app.handle_key_event(plain(KeyCode::Char('d')));
    assert!(app.pending_thread_delete.is_some());
    app.handle_key_event(plain(KeyCode::Esc));
    assert!(app.pending_thread_delete.is_none());
    assert_eq!(app.status_notice.as_deref(), Some("Delete cancelled"));
    app.pending_thread_delete = Some(PendingThreadDelete {
        thread_id: "t2".to_string(),
        title: "Child Branch".to_string(),
    });
    app.handle_key_event(ctrl('c'));
    assert!(app.pending_thread_delete.is_none());
    app.pending_thread_delete = Some(PendingThreadDelete {
        thread_id: "t2".to_string(),
        title: "Child Branch".to_string(),
    });
    app.handle_key_event(plain(KeyCode::Char('y')));
    assert_eq!(
        app.pending_actions.pop_front(),
        Some(AppAction::DeleteThread("t2".to_string()))
    );

    // PendingQuitConfirm with both busy agent and live terminal
    app.thread_statuses.insert("t1".to_string(), "streaming".to_string());
    app.thread_meta
        .insert("term-1".to_string(), make_meta("term-1", Some("t1"), ThreadKind::Terminal));
    app.request_quit_with_confirm();
    assert_eq!(
        app.pending_quit_confirm,
        Some(PendingQuitConfirm {
            active_terminals: 1,
            busy_agents: 1,
        })
    );
    app.handle_key_event(plain(KeyCode::Char('n')));
    assert!(!app.should_quit());
    app.request_quit_with_confirm();
    app.handle_key_event(plain(KeyCode::Char('y')));
    assert!(app.should_quit());
    app.should_quit = false;
    app.thread_statuses.clear();
    app.thread_meta.remove("term-1");

    // RenamePromptState readline & UTF-8 + rendering
    app.set_focus(FocusedPane::Tree);
    app.rename_prompt = Some(RenamePromptState {
        thread_id: "t1".to_string(),
        buffer: "seed".to_string(),
        cursor: 4,
    });
    let _ = render_to_string(&mut app, 40, 12);
    app.rename_prompt = None;
    app.begin_inline_rename();
    assert!(app.rename_prompt.is_some());
    app.handle_key_event(plain(KeyCode::Char('ö')));
    app.handle_key_event(plain(KeyCode::Char('k')));
    app.handle_key_event(ctrl('a'));
    app.handle_key_event(plain(KeyCode::Delete));
    app.handle_key_event(ctrl('e'));
    app.handle_key_event(key(KeyCode::Char('€'), KeyModifiers::CONTROL | KeyModifiers::ALT));
    app.handle_key_event(plain(KeyCode::Left));
    app.handle_key_event(plain(KeyCode::Right));
    app.handle_key_event(plain(KeyCode::Backspace));
    app.handle_key_event(ctrl('w'));
    app.handle_key_event(plain(KeyCode::Char('x')));
    app.handle_key_event(ctrl('u'));
    app.handle_key_event(plain(KeyCode::Char('n')));
    app.handle_key_event(plain(KeyCode::Enter));
    assert!(matches!(
        app.pending_actions.pop_front(),
        Some(AppAction::RenameThread { ref title, .. }) if title == "n"
    ));

    // ThemePickerModal Home/End/q/Ctrl+C + override restore
    let mut theme_overrides = BTreeMap::new();
    theme_overrides.insert("bg".to_string(), "#010203".to_string());
    let prefs = UserPreferencesSnapshot {
        theme: Some("nord".to_string()),
        theme_overrides,
        ..Default::default()
    };
    app.apply_preferences(&prefs);
    app.open_theme_picker();
    app.handle_key_event(plain(KeyCode::End));
    assert_eq!(app.theme_preset, ThemePreset::RosePineDawn);
    app.handle_key_event(plain(KeyCode::Home));
    assert_eq!(app.theme_preset, ThemePreset::TokyoNight);
    app.handle_key_event(plain(KeyCode::Char('q')));
    assert_eq!(app.theme_preset, ThemePreset::Nord);
    assert_eq!(app.theme_palette.bg, Color::Rgb(0x01, 0x02, 0x03));
    app.open_theme_picker();
    app.handle_key_event(plain(KeyCode::Down));
    app.handle_key_event(ctrl('c'));
    assert_eq!(app.theme_preset, ThemePreset::Nord);
    assert_eq!(app.theme_palette.bg, Color::Rgb(0x01, 0x02, 0x03));

    // Mouse event modal blocking & hit testing
    let _ = render_to_string(&mut app, 80, 24);
    app.pending_quit_confirm = Some(PendingQuitConfirm {
        active_terminals: 1,
        busy_agents: 0,
    });
    assert!(!app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 2,
        row: 2,
        modifiers: KeyModifiers::NONE,
    }));
    app.pending_quit_confirm = None;
    app.set_focus(FocusedPane::Tree);
    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 2,
        row: 2,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 60,
        row: 5,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(!app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 5,
        row: 5,
        modifiers: KeyModifiers::NONE,
    }));

    // Legacy FocusedPane::Transcript paths
    app.transcript_viewport_mut("t1").set_max_scroll(50);
    app.set_focus(FocusedPane::Transcript);
    assert!(app.handle_key_event(plain(KeyCode::Char('/'))));
    assert!(app.transcript_search.editing);
    assert!(app.handle_key_event(plain(KeyCode::Char('H'))));
    assert!(app.handle_key_event(plain(KeyCode::Char('x'))));
    assert!(app.handle_key_event(plain(KeyCode::Backspace)));
    assert!(app.handle_key_event(plain(KeyCode::Enter)));
    assert!(!app.transcript_search.editing);
    assert!(app.handle_key_event(plain(KeyCode::Char('n'))));
    assert!(app.handle_key_event(plain(KeyCode::Char('N'))));
    assert!(app.handle_key_event(plain(KeyCode::Esc)));
    assert!(app.transcript_search.query.is_empty());

    assert!(app.handle_key_event(plain(KeyCode::Char('?'))));
    assert!(app.show_help_modal);
    assert!(app.handle_key_event(plain(KeyCode::Esc)));
    assert!(app.handle_key_event(plain(KeyCode::Char('o'))));
    app.transcript_viewport_mut("t1").set_max_scroll(50);
    assert!(app.handle_key_event(plain(KeyCode::Home)));
    assert_eq!(app.transcript_viewport("t1").scroll_offset(), 0);
    assert!(app.handle_key_event(plain(KeyCode::End)));
    assert_eq!(app.transcript_viewport("t1").scroll_offset(), 50);
    assert!(app.handle_key_event(plain(KeyCode::Char('g'))));
    assert!(app.handle_key_event(plain(KeyCode::Char('g'))));
    assert_eq!(app.transcript_viewport("t1").scroll_offset(), 0);
    assert!(app.handle_key_event(plain(KeyCode::Char('G'))));
    assert_eq!(app.transcript_viewport("t1").scroll_offset(), 50);
    assert!(app.handle_key_event(plain(KeyCode::PageUp)));
    assert!(app.handle_key_event(plain(KeyCode::PageDown)));
    assert!(app.handle_key_event(plain(KeyCode::Char('k'))));
    assert!(app.handle_key_event(plain(KeyCode::Char('j'))));
    assert!(app.handle_key_event(alt('1')));
    assert!(app.handle_key_event(plain(KeyCode::Enter)));
    assert_eq!(app.focused_pane(), FocusedPane::Composer);
    app.set_focus(FocusedPane::Transcript);
    assert!(app.handle_key_event(plain(KeyCode::Char('q'))));
    assert!(app.should_quit());
}

#[test]
fn test_narrow_and_short_terminal_geometry_sweep_with_all_modals() {
    let sizes = [(0, 0), (15, 10), (20, 6), (40, 12), (80, 14)];

    for (w, h) in sizes {
        let mut app = App::new();
        seed_two_threads(&mut app);
        app.status_notice = Some("Compact status notice for narrow terminal sweep".to_string());
        app.input_buffer = "a中😀\tbcdef".to_string();
        app.cursor_position = app.input_buffer.len();
        app.thread_subagents.insert(
            "t1".to_string(),
            vec![SubagentSnapshot {
                id: "sa-1".to_string(),
                role: "Explorer".to_string(),
                agent_type: "research".to_string(),
                model: Some("sonnet".to_string()),
                activity: Some("reading files".to_string()),
                status: SubagentStatus::Running,
                started_at_ms: 1000,
            }],
        );
        let _ = compute_app_layout(&app, Rect::new(0, 0, w, h), 3, false);
        let backend = TestBackend::new(w, h);
        let mut immut_term = Terminal::new(backend).unwrap();
        immut_term.draw(|f| render_app(f, &app)).unwrap();
        let _ = render_to_string(&mut app, w, h);

        // Help modal (with active filter)
        app.show_help_modal = true;
        app.help_filter_active = true;
        app.help_filter_query = "zoom".to_string();
        let _ = render_to_string(&mut app, w, h);
        app.show_help_modal = false;

        // ThemePicker scrolling across all 16 themes
        for idx in [0usize, 8, 15] {
            app.theme_picker = Some(ThemePickerModal {
                selected_idx: idx,
                original_preset: ThemePreset::TokyoNight,
                original_overrides: BTreeMap::new(),
            });
            let _ = render_to_string(&mut app, w, h);
        }
        app.theme_picker = None;

        // ConfigPicker
        app.config_picker = Some(ConfigPickerModal {
            thread_id: "t1".to_string(),
            config_id: "model".to_string(),
            title: "Model".to_string(),
            current_value: "a".to_string(),
            options: vec![
                AgentConfigChoice {
                    value: "a".to_string(),
                    name: "Alpha".to_string(),
                    description: Some("First".to_string()),
                },
                AgentConfigChoice {
                    value: "b".to_string(),
                    name: "Beta".to_string(),
                    description: None,
                },
            ],
            selected_index: 1,
            set_default: true,
        });
        let _ = render_to_string(&mut app, w, h);
        app.config_picker = None;

        // Permission
        app.pending_permission = Some(PendingPermission {
            request_id: "p1".to_string(),
            description: "Run verification command?".to_string(),
        });
        let _ = render_to_string(&mut app, w, h);
        app.pending_permission = None;

        // ThreadDelete (empty title fallback & named title)
        app.pending_thread_delete = Some(PendingThreadDelete {
            thread_id: "t-empty-fallback".to_string(),
            title: "   ".to_string(),
        });
        let rendered = render_to_string(&mut app, w, h);
        if w >= 40 && h >= 12 {
            assert!(rendered.contains("t-empty-fallback"));
        }
        app.pending_thread_delete = None;

        // WorktreeDelete
        app.pending_worktree_delete = Some(PendingWorktreeDelete {
            path: "/tmp/wt".to_string(),
            label: "feat".to_string(),
            dirty_count: 3,
            attached_count: 1,
            force: true,
        });
        let _ = render_to_string(&mut app, w, h);
        app.pending_worktree_delete = None;

        // QuitConfirm
        app.pending_quit_confirm = Some(PendingQuitConfirm {
            active_terminals: 2,
            busy_agents: 1,
        });
        let _ = render_to_string(&mut app, w, h);
        app.pending_quit_confirm = None;
    }

    // Verify ThemePicker scrolling buffer at 80x14 selects bottom theme visibly
    let mut app = App::new();
    app.theme_picker = Some(ThemePickerModal {
        selected_idx: ThemePreset::all().len() - 1,
        original_preset: ThemePreset::TokyoNight,
        original_overrides: BTreeMap::new(),
    });
    let screen = render_to_string(&mut app, 80, 14);
    assert!(screen.contains("Rose Pine Dawn"));

    // Terminal pane + Worktrees view & mouse scroll across narrow and normal sizes
    let mut term_app = App::new();
    term_app.active_thread_id = "term-1".to_string();
    term_app.threads = vec![ThreadItem {
        id: "term-1".to_string(),
        title: "Shell".to_string(),
        custom_name: None,
        anchors: Vec::new(),
    }];
    term_app
        .thread_meta
        .insert("term-1".to_string(), make_meta("term-1", None, ThreadKind::Terminal));
    term_app.pty_screens.insert(
        "term-1".to_string(),
        PtyScreenSnapshot {
            rows: 2,
            cols: 4,
            cells: vec![vec![
                PtyCell {
                    contents: "中".to_string(),
                    fg: PtyColor::Indexed(10),
                    bg: PtyColor::Rgb(1, 2, 3),
                    bold: true,
                    italic: true,
                    underline: true,
                    inverse: true,
                    is_wide_continuation: false,
                },
                PtyCell {
                    contents: String::new(),
                    is_wide_continuation: true,
                    ..PtyCell::default()
                },
                PtyCell {
                    contents: "x".to_string(),
                    fg: PtyColor::Default,
                    bg: PtyColor::Default,
                    ..PtyCell::default()
                },
            ]],
            cursor_row: 0,
            cursor_col: 2,
            ..PtyScreenSnapshot::default()
        },
    );
    term_app.set_focus(FocusedPane::Terminal);
    for (w, h) in sizes {
        let _ = render_to_string(&mut term_app, w, h);
    }

    term_app.worktrees.push(WorktreeEntry {
        repo_root: "/repo".to_string(),
        repo_name: "grein".to_string(),
        path: "/repo/.grein/worktrees/feat-1".to_string(),
        branch: Some("feat-1".to_string()),
        head_short: Some("abc1234".to_string()),
        is_primary: false,
        is_detached: false,
        is_grein_managed: true,
        dirty_count: 2,
        attached_thread_ids: vec!["term-1".to_string()],
    });
    term_app.sidebar_view = SidebarView::Worktrees;
    term_app.worktree_selected_idx = 1;
    term_app.worktree_diff_preview = "diff --git a/x b/x\n@@ -1 +1 @@\n-old\n+new\n".to_string();
    term_app.set_focus(FocusedPane::Tree);
    for (w, h) in sizes {
        let _ = render_to_string(&mut term_app, w, h);
    }
    assert!(term_app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 2,
        row: 2,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(term_app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 60,
        row: 5,
        modifiers: KeyModifiers::NONE,
    }));
}

#[test]
fn test_syntax_scanners_markdown_transcript_and_search_edge_cases() {
    let rust = highlight_code_line(
        "rust,no_run",
        r##"#[doc = "a]b"] #![allow(all)] fn f<'a>() { let s = r#"raw"#; let b = br#"by"#; let c = b"x"; let d = b'a'; let u = '\u{1F600}'; let n = (0xFF, 0b10, 0o7, 1.5e-3); let raw = r#"unclosed"##,
        &mut false,
    );
    assert!(line_text(&rust).contains(r##"r#"raw"#"##));

    let c_dir = highlight_code_line("c", "#include <stdio.h>", &mut false);
    assert!(line_text(&c_dir).contains("<stdio.h>"));

    let py = highlight_code_lines(
        "python",
        r#"@dec.wrap
def f(): return f"hi" + rf'lo' + b"""tr""" + u'''sq'''"#,
    );
    assert_eq!(py.len(), 2);
    assert!(line_text(&py[1]).contains("rf'lo'"));

    let sh = highlight_code_line("sh", "echo $VAR ${VAR} ${UNCLOSED $? $1 $ `uname`", &mut false);
    assert!(line_text(&sh).contains("${VAR}"));

    let nix = highlight_code_line("nix", "let x = ''multi '''quote''; in x", &mut false);
    assert!(line_text(&nix).contains("'''quote"));

    let sql = highlight_code_line("sql", "SELECT 'it''s ok' -- c", &mut false);
    assert!(line_text(&sql).contains("'it''s ok'"));

    let toml = highlight_code_lines("toml", "[[arr]]\ntbl = { k = 1, \"q\" = 2 }\n[sec.\"a]b\"]\n[bad # comment");
    assert_eq!(toml.len(), 4);

    let yaml = highlight_code_lines("yaml", "- key: val\n'q': 1\na:b");
    assert_eq!(yaml.len(), 3);

    let js_go = highlight_code_lines("js", "`tmpl`")
        .into_iter()
        .chain(highlight_code_lines("go", "`raw\\`"))
        .count();
    assert_eq!(js_go, 2);
    assert_eq!(safe_fence_for("````"), "`````");

    let ansi = render_ansi_text(
        "\x1b[4:3mU\x1b[4:0m \x1b[38:5:196mI\x1b[48:2:10:20:30mR\x1b[38:2::1:2:3mS\x1b(B\x1b)0\x1b*A\x1b+B\x1b]0;title\x07\x1b]2;t2\x1b\\plain\x1b[0m\n[stderr]\nerr",
    );
    assert_eq!(line_text(&ansi[0]), "U IRSplain");
    assert!(ansi[0].spans[0].style.add_modifier.contains(Modifier::UNDERLINED));
    assert_eq!(ansi[0].spans[2].style.fg, Some(Color::Indexed(196)));
    assert_eq!(ansi[0].spans[3].style.bg, Some(Color::Rgb(10, 20, 30)));
    assert_eq!(ansi[0].spans[4].style.fg, Some(Color::Rgb(1, 2, 3)));

    let bi = parse_inline_spans(r#"***bold italic*** and \*escaped\_ \` \\ \|"#);
    assert!(bi[0].style.add_modifier.contains(Modifier::BOLD | Modifier::ITALIC));
    assert_eq!(bi[1].content.as_ref(), " and *escaped_ ` \\ |");

    let md_zero = render_markdown_lines_with_width("| A | B |\n| :--- | ---: |\n| 1\\|2 | **hi** |\n\n---", 0);
    assert!(!md_zero.is_empty());
    let plain_md = render_markdown_lines("plain text");
    assert_eq!(plain_md.len(), 1);
    let wrapped_zero = wrap_styled_line(Line::from(Span::styled("abc", Style::default().fg(Color::Red))), 0);
    assert_eq!(wrapped_zero.len(), 1);
    let wrapped_narrow = wrap_styled_lines(vec![Line::raw("  hello world")], 3);
    assert!(wrapped_narrow.len() >= 3);

    // Transcript width == 0, unified diff detection, fenced code block edge cases
    let mut tr = ThreadTranscript::new();
    tr.push_user("u1\nu2");
    tr.append_assistant_chunk("~~~rust\nfn one() {}\n~~~\n\n    ````\nnot fence\n    ````\n```bash\necho 2\n```");
    assert_eq!(tr.latest_code_block().as_deref(), Some("echo 2"));
    tr.start_tool_call(Some("id1".into()), "edit", Some("file".into()));
    tr.complete_tool_call(
        Some("id1"),
        "edit",
        "diff --git a/x b/x\n@@ -1 +1 @@\n-old\n+new\n",
        false,
    );
    tr.push_shell_pending("echo hi");
    tr.complete_shell("echo hi", "ok".into(), "warn".into(), 0);
    tr.push_join_note("joined");
    tr.push_error("boom");
    assert!(tr.blocks.iter().any(|b| matches!(b, TranscriptBlock::ToolCall { .. })));
    let zero_lines = tr.render_lines(0);
    assert!(!zero_lines.is_empty());

    // Search edge cases: empty span, overlapping matches, invalid span, out-of-bounds, unicode
    let lines = vec![Line::from(vec![Span::raw(""), Span::raw("aaaa \u{0130}x")])];
    let mut search = TranscriptSearchState::default();
    search.open(SearchDirection::Reverse);
    search.query = "aa".to_string();
    search.recompute_matches(&lines, Some(0));
    assert_eq!(search.matches.len(), 2);
    assert_eq!(search.counter_label(), "[2/2]");
    search.query = "i".to_string();
    search.recompute_matches(&lines, Some(0));
    assert!(search.matches.is_empty());
    search.query = "x".to_string();
    search.recompute_matches(&lines, Some(0));
    assert_eq!(search.matches.len(), 1);

    let highlighted = highlight_lines_with_matches(
        lines,
        &[
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_end: 2,
            },
            SearchMatch {
                line_index: 0,
                byte_start: 1,
                byte_end: 3,
            },
            SearchMatch {
                line_index: 0,
                byte_start: 2,
                byte_end: 2,
            },
            SearchMatch {
                line_index: 99,
                byte_start: 0,
                byte_end: 1,
            },
        ],
        Some(0),
    );
    assert_eq!(line_text(&highlighted[0]), "aaaa \u{0130}x");
}

#[test]
fn test_diff_card_pager_focus_and_clipboard_binaries() {
    struct StubPager(bool);
    impl PagerRunner for StubPager {
        fn run_pager(&self, _payload: &str) -> io::Result<()> {
            if self.0 {
                Ok(())
            } else {
                Err(io::Error::other("pager failed"))
            }
        }
    }

    let diff = "--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-old\n+new\n ctx\n";
    let card = DiffCard::builder()
        .diff_content(diff)
        .reply_anchor("Turn 1")
        .max_preview_lines(4)
        .pager_runner(StubPager(true))
        .build();
    let preview = card.preview_lines();
    assert_eq!(preview.len(), 4);
    assert_eq!(preview[0].kind(), DiffLineKind::Deletion);
    assert_eq!(preview[1].kind(), DiffLineKind::Addition);
    assert_eq!(preview[2].kind(), DiffLineKind::Header);
    assert!(preview[3].is_deletion());
    assert_eq!(card.reply_anchor(), Some("Turn 1"));
    assert_eq!(card.max_preview_lines(), 4);

    let mut suspended = false;
    assert_eq!(
        card.handle_key(plain(KeyCode::Char('x')), || suspended = true),
        DiffCardAction::Ignored
    );
    assert!(!suspended);
    assert_eq!(
        card.handle_key(plain(KeyCode::Char('o')), || suspended = true),
        DiffCardAction::OpenedPager
    );
    assert!(suspended);

    let mut small_buf = Buffer::empty(Rect::new(0, 0, 12, 3));
    card.render_badge(&mut small_buf, Rect::new(0, 0, 12, 1));
    (&card).render(Rect::new(0, 0, 0, 0), &mut small_buf);
    (&card).render(Rect::new(0, 0, 12, 3), &mut small_buf);
    DiffCard::builder()
        .diff_content(" ctx\n+a")
        .pager_runner(StubPager(false))
        .build()
        .render(Rect::new(0, 0, 20, 4), &mut Buffer::empty(Rect::new(0, 0, 20, 4)));

    // DefaultPagerRunner error handling (PAGER="", PAGER nonexistent, PAGER="true")
    {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev_pager = std::env::var_os("PAGER");
        let prev_path = std::env::var_os("PATH");

        let runner = DefaultPagerRunner::new();
        std::env::set_var("PAGER", "/nonexistent/grein-pager-bin");
        assert!(runner.run_pager("diff").is_err());

        std::env::set_var("PAGER", "");
        std::env::set_var("PATH", "");
        assert!(runner.run_pager("diff").is_err());

        match prev_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        std::env::set_var("PAGER", "true");
        assert!(runner.run_pager("diff").is_ok());

        match prev_pager {
            Some(v) => std::env::set_var("PAGER", v),
            None => std::env::remove_var("PAGER"),
        }
    }

    // focus.rs and pane::focus
    let mut fm = FocusManager::new(CorePaneFocus::Composer);
    fm.handle_action(FocusNavigationAction::Tab);
    assert_eq!(fm.current(), CorePaneFocus::Sidebar);
    fm.handle_action(FocusNavigationAction::BackTab);
    assert_eq!(fm.current(), CorePaneFocus::Composer);
    assert_eq!(CorePaneFocus::from(FocusedPane::Terminal), CorePaneFocus::Transcript);
    assert_eq!(CorePaneFocus::Sessions.next(), CorePaneFocus::Chat);
    assert_eq!(CorePaneFocus::Chat.prev(), CorePaneFocus::Sessions);

    assert_eq!(LegacyPaneFocus::Workspace.name(), "Workspace");
    assert_eq!(LegacyPaneFocus::Composer.name(), "Composer");
    assert_eq!(LegacyPaneFocus::Logs.name(), "Logs");
    assert_eq!(LegacyPaneFocus::Workspace.next(), LegacyPaneFocus::Composer);
    assert_eq!(LegacyPaneFocus::Workspace.previous(), LegacyPaneFocus::Logs);
    assert_eq!(FocusedPane::from(LegacyPaneFocus::Logs), FocusedPane::Transcript);
    assert_eq!(LegacyPaneFocus::from(FocusedPane::Terminal), LegacyPaneFocus::Logs);

    // Binary 0x00..=0xFF clipboard encoding + OSC52 tmux wrapping
    let bin: Vec<u8> = (0u8..=255).collect();
    let b64 = encode_base64(&bin);
    assert_eq!(b64.len(), 344);
    assert!(encode_osc52("hi", true).starts_with("\x1bPtmux;"));
    assert!(encode_osc52("hi", false).starts_with("\x1b]52;c;"));
}
