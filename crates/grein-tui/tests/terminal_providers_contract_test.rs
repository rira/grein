use std::io::Write;
use std::sync::{Arc, Mutex};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use grein_tui::clipboard::{ClipboardWriterBackend, Osc52Backend};
use grein_tui::runner::key_inspector::{
    format_decoded_input, format_inspector_header, format_raw_bytes, KeyInspectorOptions,
};
use grein_tui::terminal::providers::{
    default_adapter, default_registry, AlacrittyProvider, AppleTerminalProvider,
    GenericXtermProvider, GhosttyProvider, ITerm2Provider, KittyProvider, TmuxProvider,
    WezTermProvider, ZellijProvider,
};
use grein_tui::terminal::{
    ColorCapability, KeyboardProtocolPolicy, TerminalAdapter, TerminalEnv, TerminalKind,
    TerminalSessionHost, TerminalSessionOptions,
};
use grein_tui::terminal_decoder::DecodedInput;
use grein_tui::theme::supports_truecolor_from_env;
use grein_tui::Keymap;

#[test]
fn default_registry_registers_all_nine_providers_in_priority_order() {
    let reg = default_registry();
    assert_eq!(reg.adapters().len(), 9);
    for kind in [
        TerminalKind::Tmux,
        TerminalKind::Zellij,
        TerminalKind::AppleTerminal,
        TerminalKind::ITerm2,
        TerminalKind::Ghostty,
        TerminalKind::Kitty,
        TerminalKind::WezTerm,
        TerminalKind::Alacritty,
        TerminalKind::GenericXterm,
    ] {
        assert!(
            reg.resolve_by_kind(kind).is_some(),
            "missing provider for {kind}"
        );
    }
}

#[test]
fn nested_multiplexer_precedence_beats_outer_terminal_emulators() {
    // tmux inside Ghostty -> Tmux wins (score 90 > 70)
    let tmux_in_ghostty = TerminalEnv::default()
        .with_tmux("/tmp/tmux-1000/default,123,0")
        .with_term_program("ghostty")
        .with_ghostty_resources_dir("/usr/share/ghostty")
        .with_colorterm("truecolor");
    assert_eq!(
        default_adapter(&tmux_in_ghostty).kind(),
        TerminalKind::Tmux
    );

    // zellij inside iTerm2 -> Zellij wins (score 85 > 70)
    let zellij_in_iterm = TerminalEnv::default()
        .with_zellij("0")
        .with_term_program("iTerm.app");
    assert_eq!(
        default_adapter(&zellij_in_iterm).kind(),
        TerminalKind::Zellij
    );

    // zellij detected via ZELLIJ_SESSION_NAME inside Kitty -> Zellij wins (85 > 70)
    let zellij_session_in_kitty = TerminalEnv::default()
        .with_zellij_session_name("dev")
        .with_kitty_window_id("1")
        .with_term("xterm-kitty");
    assert_eq!(
        default_adapter(&zellij_session_in_kitty).kind(),
        TerminalKind::Zellij
    );

    // tmux inside zellij -> Tmux wins (90 > 85)
    let tmux_in_zellij = TerminalEnv::default()
        .with_tmux("/tmp/tmux-1000/default,1,0")
        .with_zellij("0");
    assert_eq!(default_adapter(&tmux_in_zellij).kind(), TerminalKind::Tmux);
}

#[test]
fn all_providers_match_expected_environments_and_grein_terminal_override() {
    let cases = [
        (
            TerminalEnv::default().with_tmux("/tmp/tmux,1,0"),
            TerminalKind::Tmux,
        ),
        (
            TerminalEnv::default().with_zellij("0"),
            TerminalKind::Zellij,
        ),
        (
            TerminalEnv::default().with_term_program("Apple_Terminal"),
            TerminalKind::AppleTerminal,
        ),
        (
            TerminalEnv::default().with_term_program("iTerm.app"),
            TerminalKind::ITerm2,
        ),
        (
            TerminalEnv::default().with_term_program("ghostty"),
            TerminalKind::Ghostty,
        ),
        (
            TerminalEnv::default().with_ghostty_resources_dir("/Applications/Ghostty.app"),
            TerminalKind::Ghostty,
        ),
        (
            TerminalEnv::default().with_kitty_window_id("42"),
            TerminalKind::Kitty,
        ),
        (
            TerminalEnv::default().with_term("xterm-kitty"),
            TerminalKind::Kitty,
        ),
        (
            TerminalEnv::default().with_term_program("kitty"),
            TerminalKind::Kitty,
        ),
        (
            TerminalEnv::default().with_term_program("WezTerm"),
            TerminalKind::WezTerm,
        ),
        (
            TerminalEnv::default().with_wezterm_executable("/usr/bin/wezterm-gui"),
            TerminalKind::WezTerm,
        ),
        (
            TerminalEnv::default().with_term_program("Alacritty"),
            TerminalKind::Alacritty,
        ),
        (
            TerminalEnv::default().with_term("alacritty"),
            TerminalKind::Alacritty,
        ),
        (
            TerminalEnv::default().with_term("xterm-256color"),
            TerminalKind::GenericXterm,
        ),
    ];

    for (env, expected_kind) in cases {
        assert_eq!(
            default_adapter(&env).kind(),
            expected_kind,
            "mismatch for {env:?}"
        );
    }

    let overridden = TerminalEnv::default()
        .with_tmux("/tmp/tmux,1,0")
        .with_grein_terminal_override("alacritty");
    assert_eq!(default_adapter(&overridden).kind(), TerminalKind::Alacritty);
}

#[test]
fn apple_terminal_never_probes_kkp_and_always_emits_setup_hint() {
    let provider = AppleTerminalProvider;
    let legacy_env = TerminalEnv::default()
        .with_term_program("Apple_Terminal")
        .with_term("xterm-256color");
    let tahoe_env = legacy_env.clone().with_colorterm("truecolor");

    assert_eq!(
        provider.capabilities(&legacy_env).color,
        ColorCapability::Xterm256
    );
    assert_eq!(
        provider.capabilities(&tahoe_env).color,
        ColorCapability::TrueColor
    );
    assert_eq!(
        provider.capabilities(&legacy_env).keyboard_protocol,
        KeyboardProtocolPolicy::None
    );

    for opts in [
        TerminalSessionOptions::default(),
        TerminalSessionOptions {
            enhance_keyboard: false,
            ..Default::default()
        },
    ] {
        let plan = provider.protocol_plan(&tahoe_env, &opts);
        assert!(plan.enter_bytes.is_empty());
        assert!(plan.leave_bytes.is_empty());
        assert_eq!(plan.probe_kitty_timeout_ms, None);
        assert!(!plan.expects_kitty_pop);
    }

    for env in [&legacy_env, &tahoe_env] {
        let joined = provider
            .diagnostic_hints(env)
            .into_iter()
            .map(|h| h.message)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("grein terminal setup apple-terminal"), "{joined}");
        assert!(joined.contains("Apple_Terminal"), "{joined}");
        assert!(joined.contains("Shift+Arrows"), "{joined}");
        assert!(joined.contains("Ctrl+G"), "{joined}");
        assert!(joined.contains("grein keys"), "{joined}");
    }
}

#[test]
fn tmux_uses_modify_other_keys_dcs_osc52_and_truecolor_hints() {
    let provider = TmuxProvider;
    let env_256 = TerminalEnv::default()
        .with_tmux("/tmp/tmux,1,0")
        .with_term("tmux-256color");
    let env_tc = env_256.clone().with_colorterm("truecolor");

    assert_eq!(
        provider.capabilities(&env_256).color,
        ColorCapability::Xterm256
    );
    assert_eq!(
        provider.capabilities(&env_tc).color,
        ColorCapability::TrueColor
    );
    assert_eq!(
        provider.capabilities(&env_256).keyboard_protocol,
        KeyboardProtocolPolicy::ModifyOtherKeys
    );

    let plan = provider.protocol_plan(&env_256, &TerminalSessionOptions::default());
    assert_eq!(plan.enter_bytes, b"\x1b[>4;2m\x1b[>1;2m");
    assert_eq!(plan.leave_bytes, b"\x1b[>4m\x1b[>1m");
    assert_eq!(plan.probe_kitty_timeout_ms, None);
    assert!(!plan.expects_kitty_pop);

    let disabled = provider.protocol_plan(
        &env_256,
        &TerminalSessionOptions {
            enhance_keyboard: false,
            ..Default::default()
        },
    );
    assert!(disabled.enter_bytes.is_empty());
    assert!(disabled.leave_bytes.is_empty());

    assert_eq!(
        provider.encode_osc52("YWJj"),
        "\x1bPtmux;\x1b\x1b]52;c;YWJj\x07\x1b\\"
    );

    let hints = provider
        .diagnostic_hints(&env_256)
        .into_iter()
        .map(|h| h.message)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(hints.contains("grein terminal setup tmux"), "{hints}");
    assert!(hints.contains("xterm-keys on"), "{hints}");
    assert!(hints.contains("extended-keys always"), "{hints}");
    assert!(hints.contains("csi-u"), "{hints}");
}

#[test]
fn keyboard_policies_enter_leave_symmetry_and_hints_across_all_providers() {
    let disambiguate_providers: Vec<(Arc<dyn TerminalAdapter>, &str)> = vec![
        (Arc::new(ITerm2Provider), "grein terminal setup iterm2"),
        (Arc::new(GhosttyProvider), "grein terminal setup ghostty"),
        (Arc::new(KittyProvider), "grein terminal setup kitty"),
        (Arc::new(AlacrittyProvider), "grein terminal setup alacritty"),
    ];
    let env = TerminalEnv::default();
    let opts_on = TerminalSessionOptions::default();
    let opts_off = TerminalSessionOptions {
        enhance_keyboard: false,
        ..Default::default()
    };

    for (provider, expected_setup_cmd) in disambiguate_providers {
        let caps = provider.capabilities(&env);
        assert_eq!(caps.color, ColorCapability::TrueColor);
        assert_eq!(
            caps.keyboard_protocol,
            KeyboardProtocolPolicy::KittyDisambiguate
        );

        let plan_on = provider.protocol_plan(&env, &opts_on);
        assert_eq!(plan_on.enter_bytes, b"\x1b[>1u");
        assert_eq!(plan_on.leave_bytes, b"\x1b[<u");
        assert_eq!(plan_on.probe_kitty_timeout_ms, None);
        assert!(plan_on.expects_kitty_pop);

        let plan_off = provider.protocol_plan(&env, &opts_off);
        assert!(plan_off.enter_bytes.is_empty());
        assert!(plan_off.leave_bytes.is_empty());
        assert!(!plan_off.expects_kitty_pop);

        assert_eq!(provider.encode_osc52("YWJj"), "\x1b]52;c;YWJj\x07");

        let hints = provider
            .diagnostic_hints(&env)
            .into_iter()
            .map(|h| h.message)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(hints.contains(expected_setup_cmd), "{hints}");
    }

    let bounded_providers: Vec<(Arc<dyn TerminalAdapter>, &str)> = vec![
        (Arc::new(WezTermProvider), "grein terminal setup wezterm"),
        (Arc::new(ZellijProvider), "grein terminal setup zellij"),
        (Arc::new(GenericXtermProvider), ""),
    ];
    for (provider, expected_setup_cmd) in bounded_providers {
        let caps = provider.capabilities(&env);
        assert_eq!(
            caps.keyboard_protocol,
            KeyboardProtocolPolicy::BoundedProbeKitty { timeout_ms: 120 }
        );
        let plan_on = provider.protocol_plan(&env, &opts_on);
        assert_eq!(plan_on.enter_bytes, b"\x1b[>1u");
        assert_eq!(plan_on.leave_bytes, b"\x1b[<u");
        assert_eq!(plan_on.probe_kitty_timeout_ms, Some(120));
        assert!(plan_on.expects_kitty_pop);

        let plan_off = provider.protocol_plan(&env, &opts_off);
        assert!(plan_off.enter_bytes.is_empty());
        assert!(plan_off.leave_bytes.is_empty());

        if !expected_setup_cmd.is_empty() {
            let hints = provider
                .diagnostic_hints(&env)
                .into_iter()
                .map(|h| h.message)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(hints.contains(expected_setup_cmd), "{hints}");
        }
    }
}

#[test]
fn enter_and_leave_symmetry_through_session_host_writer() {
    struct HostWriterCase {
        adapter: Arc<dyn TerminalAdapter>,
        expected_enter: &'static [u8],
        expected_leave: &'static [u8],
        expected_kbd_enhanced: bool,
    }

    let cases = vec![
        HostWriterCase {
            adapter: Arc::new(TmuxProvider),
            expected_enter: b"\x1b[>4;2m\x1b[>1;2m",
            expected_leave: b"\x1b[>4m\x1b[>1m",
            expected_kbd_enhanced: false,
        },
        HostWriterCase {
            adapter: Arc::new(GhosttyProvider),
            expected_enter: b"\x1b[>1u",
            expected_leave: b"\x1b[<u",
            expected_kbd_enhanced: true,
        },
        HostWriterCase {
            adapter: Arc::new(AppleTerminalProvider),
            expected_enter: b"",
            expected_leave: b"",
            expected_kbd_enhanced: false,
        },
    ];

    for case in cases {
        let mut buf = Vec::new();
        let mut host = TerminalSessionHost::enter_with_writer(
            case.adapter,
            TerminalEnv::default(),
            TerminalSessionOptions::default(),
            &mut buf,
            || Ok(()),
        )
        .expect("enter_with_writer succeeds");
        assert_eq!(
            host.active_modes().keyboard_enhanced,
            case.expected_kbd_enhanced
        );
        if !case.expected_enter.is_empty() {
            assert!(buf.ends_with(case.expected_enter));
        }

        buf.clear();
        host.leave_with_writer(&mut buf, || Ok(()))
            .expect("leave_with_writer succeeds");
        if !case.expected_leave.is_empty() {
            assert!(buf.starts_with(case.expected_leave));
        }
        buf.flush().unwrap();
    }
}

#[test]
fn color_caps_and_osc52_backend_delegate_to_provider_registry() {
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
        None,
        Some("vscode"),
        None,
        false,
        false
    ));
    assert!(supports_truecolor_from_env(
        None,
        None,
        Some("tmux-24bit"),
        false,
        false
    ));

    let tmux_sink = Arc::new(Mutex::new(Vec::new()));
    let tmux_backend = Osc52Backend::with_adapter_and_sink(Arc::new(TmuxProvider), Arc::clone(&tmux_sink));
    tmux_backend.write("abc").unwrap();
    assert_eq!(
        *tmux_sink.lock().unwrap(),
        b"\x1bPtmux;\x1b\x1b]52;c;YWJj\x07\x1b\\"
    );

    let ghostty_sink = Arc::new(Mutex::new(Vec::new()));
    let ghostty_backend =
        Osc52Backend::with_adapter_and_sink(Arc::new(GhosttyProvider), Arc::clone(&ghostty_sink));
    ghostty_backend.write("abc").unwrap();
    assert_eq!(*ghostty_sink.lock().unwrap(), b"\x1b]52;c;YWJj\x07");
}

#[test]
fn key_inspector_formats_header_and_raw_bytes_including_empty_sequences() {
    let env = TerminalEnv::default()
        .with_term("tmux-256color")
        .with_term_program("ghostty")
        .with_tmux("/tmp/tmux,1,0")
        .with_colorterm("truecolor");
    let adapter = default_adapter(&env);
    let caps = adapter.capabilities(&env);
    let header = format_inspector_header(
        env.term.as_deref(),
        env.term_program.as_deref(),
        env.colorterm.as_deref(),
        "policy=ModifyOtherKeys, active=true",
        adapter.display_name(),
        &caps,
    );
    assert!(header.contains("adapter=tmux"), "{header}");
    assert!(header.contains("color=TrueColor"), "{header}");
    assert!(header.contains("keyboard_protocol=ModifyOtherKeys"), "{header}");

    let opts = KeyInspectorOptions {
        enhance_keyboard: true,
        raw_bytes: true,
    };
    assert!(opts.raw_bytes);

    let keymap = Keymap::default();
    let decoded_shift_up = DecodedInput {
        event: Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)),
        raw_bytes: b"\x1b[1;2A".to_vec(),
    };
    let formatted = format_decoded_input(&decoded_shift_up, &keymap, true);
    assert!(formatted.contains("code=Up"), "{formatted}");
    assert!(formatted.contains("hex=1b 5b 31 3b 32 41"), "{formatted}");
    assert!(formatted.contains("escaped=\"\\x1b[1;2A\""), "{formatted}");

    let plain_arrow_with_hint = DecodedInput {
        event: Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
        raw_bytes: b"\x1b[A".to_vec(),
    };
    let formatted_with_hint = format_decoded_input(&plain_arrow_with_hint, &keymap, true);
    assert!(
        formatted_with_hint.contains("raw=[hex=1b 5b 41 escaped=\"\\x1b[A\"]\n  hint:"),
        "{formatted_with_hint}"
    );

    let empty_raw = DecodedInput {
        event: Event::Resize(120, 40),
        raw_bytes: Vec::new(),
    };
    let formatted_empty = format_decoded_input(&empty_raw, &keymap, true);
    assert!(formatted_empty.contains("Resize(120, 40)"), "{formatted_empty}");
    assert!(
        formatted_empty.contains("raw=[hex=<none> escaped=\"\"]"),
        "{formatted_empty}"
    );

    assert_eq!(
        format_raw_bytes(b"\x1b[999z"),
        "hex=1b 5b 39 39 39 7a escaped=\"\\x1b[999z\""
    );
}
