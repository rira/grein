use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;

use crossterm::event::Event;
use grein_tui::runner::{handle_event, TerminalGuard};
use grein_tui::terminal::providers::{default_adapter, default_registry, GenericXtermProvider};
use grein_tui::terminal::{
    ColorCapability, HintLevel, KeyboardProtocolPolicy, TerminalAdapter, TerminalCapabilities,
    TerminalEnv, TerminalHint, TerminalKind, TerminalProtocolPlan, TerminalRegistry,
    TerminalSessionHost, TerminalSessionOptions,
};
use grein_tui::TuiServices;

struct StubTerminalAdapter {
    kind: TerminalKind,
    score: Option<u8>,
    enter_bytes: Vec<u8>,
    leave_bytes: Vec<u8>,
    expects_kitty_pop: bool,
}

impl StubTerminalAdapter {
    fn new(kind: TerminalKind, score: Option<u8>) -> Self {
        Self {
            kind,
            score,
            enter_bytes: b"<ENTER>".to_vec(),
            leave_bytes: b"<LEAVE>".to_vec(),
            expects_kitty_pop: true,
        }
    }
}

impl TerminalAdapter for StubTerminalAdapter {
    fn kind(&self) -> TerminalKind {
        self.kind
    }

    fn display_name(&self) -> &'static str {
        self.kind.display_name()
    }

    fn matches(&self, _env: &TerminalEnv) -> Option<u8> {
        self.score
    }

    fn capabilities(&self, _env: &TerminalEnv) -> TerminalCapabilities {
        TerminalCapabilities {
            color: ColorCapability::TrueColor,
            keyboard_protocol: KeyboardProtocolPolicy::KittyDisambiguate,
            osc52_clipboard: true,
            focus_events: true,
            bracketed_paste: true,
            notes: vec!["stub".to_string()],
        }
    }

    fn protocol_plan(
        &self,
        _env: &TerminalEnv,
        opts: &TerminalSessionOptions,
    ) -> TerminalProtocolPlan {
        if opts.enhance_keyboard {
            TerminalProtocolPlan {
                enter_bytes: self.enter_bytes.clone(),
                probe_kitty_timeout_ms: Some(50),
                expects_kitty_pop: self.expects_kitty_pop,
                leave_bytes: self.leave_bytes.clone(),
            }
        } else {
            TerminalProtocolPlan::default()
        }
    }

    fn encode_osc52(&self, payload_b64: &str) -> String {
        format!("\x1b]52;c;{payload_b64}\x07")
    }

    fn diagnostic_hints(&self, _env: &TerminalEnv) -> Vec<TerminalHint> {
        vec![TerminalHint {
            level: HintLevel::Info,
            message: "stub hint".to_string(),
        }]
    }
}

struct OrderedLogWriter {
    events: Rc<RefCell<Vec<String>>>,
    bytes: Vec<u8>,
}

impl OrderedLogWriter {
    fn new(events: Rc<RefCell<Vec<String>>>) -> Self {
        Self {
            events,
            bytes: Vec::new(),
        }
    }
}

impl Write for OrderedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        self.events
            .borrow_mut()
            .push(format!("write:{}", String::from_utf8_lossy(buf)));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.events.borrow_mut().push("flush".to_string());
        Ok(())
    }
}

#[test]
fn registry_selects_highest_priority_and_ignores_none() {
    let mut registry = TerminalRegistry::new();
    let env = TerminalEnv::default();
    assert!(registry.resolve(&env).is_none());

    registry.register(Arc::new(StubTerminalAdapter::new(
        TerminalKind::GenericXterm,
        Some(10),
    )));
    registry.register(Arc::new(StubTerminalAdapter::new(
        TerminalKind::Ghostty,
        None,
    )));
    registry.register(Arc::new(StubTerminalAdapter::new(
        TerminalKind::Kitty,
        Some(90),
    )));
    registry.register(Arc::new(StubTerminalAdapter::new(
        TerminalKind::Alacritty,
        Some(70),
    )));

    let selected = registry.resolve(&env).expect("adapter should resolve");
    assert_eq!(selected.kind(), TerminalKind::Kitty);
}

#[test]
fn registry_honors_grein_terminal_override_over_higher_scores() {
    let mut registry = TerminalRegistry::new();
    registry.register(Arc::new(StubTerminalAdapter::new(
        TerminalKind::Kitty,
        Some(95),
    )));
    registry.register(Arc::new(StubTerminalAdapter::new(
        TerminalKind::AppleTerminal,
        None,
    )));

    let env = TerminalEnv::default().with_grein_terminal_override("apple-terminal");
    let selected = registry
        .resolve(&env)
        .expect("override should resolve registered kind");
    assert_eq!(selected.kind(), TerminalKind::AppleTerminal);

    let unknown_override = TerminalEnv::default().with_grein_terminal_override("nonexistent-term");
    let fallback = registry
        .resolve(&unknown_override)
        .expect("unknown override falls back to score matching");
    assert_eq!(fallback.kind(), TerminalKind::Kitty);
}

#[test]
fn terminal_kind_from_str_and_display_round_trip() {
    for kind in [
        TerminalKind::AppleTerminal,
        TerminalKind::ITerm2,
        TerminalKind::Ghostty,
        TerminalKind::Kitty,
        TerminalKind::WezTerm,
        TerminalKind::Alacritty,
        TerminalKind::Tmux,
        TerminalKind::Zellij,
        TerminalKind::GenericXterm,
    ] {
        assert_eq!(TerminalKind::from_str(kind.as_str()).unwrap(), kind);
        assert_eq!(TerminalKind::from_str(&kind.to_string()).unwrap(), kind);
    }
    assert!(TerminalKind::from_str("unknown-terminal").is_err());
}

#[test]
fn protocol_plan_enter_and_leave_bytes_flush_before_disable_raw_mode() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut writer = OrderedLogWriter::new(Rc::clone(&events));
    let adapter: Arc<dyn TerminalAdapter> =
        Arc::new(StubTerminalAdapter::new(TerminalKind::Ghostty, Some(90)));
    let env = TerminalEnv::default().with_colorterm("truecolor");
    let opts = TerminalSessionOptions::default();

    let events_for_enable = Rc::clone(&events);
    let mut host = TerminalSessionHost::enter_with_writer(adapter, env, opts, &mut writer, || {
        events_for_enable
            .borrow_mut()
            .push("enable_raw_mode".to_string());
        Ok(())
    })
    .expect("enter_with_writer should succeed");

    assert!(host.is_active());
    assert!(host.active_modes().raw_mode);
    assert!(host.active_modes().alternate_screen);
    assert!(host.active_modes().mouse_capture);
    assert!(host.active_modes().bracketed_paste);
    assert!(host.active_modes().focus_change);
    assert!(host.active_modes().keyboard_enhanced);
    assert_eq!(host.take_pending_input_bytes(), Vec::<u8>::new());

    let enter_written = String::from_utf8_lossy(&writer.bytes).to_string();
    assert!(enter_written.ends_with("<ENTER>"));
    assert!(enter_written.contains("\x1b[?1004h"));

    writer.bytes.clear();
    events.borrow_mut().clear();

    let events_for_disable = Rc::clone(&events);
    host.leave_with_writer(&mut writer, || {
        events_for_disable
            .borrow_mut()
            .push("disable_raw_mode".to_string());
        Ok(())
    })
    .expect("leave_with_writer should succeed");

    assert!(!host.is_active());
    assert!(!host.active_modes().raw_mode);

    let leave_written = String::from_utf8_lossy(&writer.bytes).to_string();
    assert!(leave_written.starts_with("<LEAVE>"));
    assert!(leave_written.contains("\x1b[?1004l"));

    let recorded = events.borrow().clone();
    let disable_idx = recorded
        .iter()
        .position(|e| e == "disable_raw_mode")
        .expect("disable_raw_mode must be called");
    let flush_idx = recorded
        .iter()
        .position(|e| e == "flush")
        .expect("flush must be called");
    assert!(
        flush_idx < disable_idx,
        "leave bytes and mode disables must flush BEFORE disable_raw_mode: {recorded:?}"
    );
}

#[test]
fn non_tty_host_and_terminal_guard_are_safe() {
    let adapter: Arc<dyn TerminalAdapter> = Arc::new(GenericXtermProvider);
    let host = TerminalSessionHost::new(
        adapter,
        TerminalEnv::default(),
        TerminalSessionOptions::default(),
    )
    .expect("non-TTY TerminalSessionHost::new must succeed");
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        assert!(!host.is_active());
        assert!(!host.active_modes().raw_mode);
    }
    drop(host);

    let guard = TerminalGuard::new().expect("TerminalGuard::new must succeed");
    drop(guard);
}

#[test]
fn tui_services_with_terminal_preserves_equality_and_debug() {
    let stub: Arc<dyn TerminalAdapter> =
        Arc::new(StubTerminalAdapter::new(TerminalKind::Kitty, Some(90)));
    let services = TuiServices::default().with_terminal(Arc::clone(&stub));
    assert_eq!(services.terminal.kind(), TerminalKind::Kitty);
    assert_eq!(services, TuiServices::default());
    let dbg = format!("{services:?}");
    assert!(dbg.contains("<dyn TerminalAdapter>"), "{dbg}");

    let reg = default_registry();
    assert_eq!(reg.adapters().len(), 9);
    assert_eq!(
        default_adapter(&TerminalEnv::default()).kind(),
        TerminalKind::GenericXterm
    );
}

#[test]
fn runner_handles_focus_gained_and_lost_explicitly() {
    let mut app = grein_tui::App::new();
    assert!(app.terminal_focused);
    handle_event(&mut app, Event::FocusLost).expect("FocusLost handled");
    assert!(!app.terminal_focused);
    handle_event(&mut app, Event::FocusGained).expect("FocusGained handled");
    assert!(app.terminal_focused);
}
