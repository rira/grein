use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture,
};
use crossterm::queue;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};

use crate::terminal::env::TerminalEnv;
use crate::terminal::port::{
    ActiveTerminalModes, TerminalAdapter, TerminalCapabilities, TerminalProtocolPlan,
    TerminalSessionOptions,
};

#[derive(Clone)]
struct PanicRestoreState {
    host_id: u64,
    plan: TerminalProtocolPlan,
    active_modes: ActiveTerminalModes,
}

static PANIC_HOOK_ONCE: Once = Once::new();
static NEXT_HOST_ID: AtomicU64 = AtomicU64::new(1);
static PANIC_RESTORE_STATE: Mutex<Option<PanicRestoreState>> = Mutex::new(None);

fn install_panic_restore_hook() {
    PANIC_HOOK_ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let state = PANIC_RESTORE_STATE
                .lock()
                .ok()
                .and_then(|mut guard| guard.take());
            if let Some(mut st) = state {
                let mut stdout = io::stdout().lock();
                let _ = restore_with_writer(
                    &mut stdout,
                    &st.plan,
                    &mut st.active_modes,
                    disable_raw_mode,
                );
            }
            prev(info);
        }));
    });
}

fn write_enter_sequence<W: Write>(
    writer: &mut W,
    opts: &TerminalSessionOptions,
    plan: &TerminalProtocolPlan,
) -> io::Result<()> {
    if opts.alternate_screen {
        queue!(writer, EnterAlternateScreen)?;
    }
    if opts.bracketed_paste {
        queue!(writer, EnableBracketedPaste)?;
    }
    if opts.mouse_capture {
        queue!(writer, EnableMouseCapture)?;
    }
    if opts.focus_change {
        queue!(writer, EnableFocusChange)?;
    }
    if !plan.enter_bytes.is_empty() {
        writer.write_all(&plan.enter_bytes)?;
    }
    writer.flush()
}

pub fn restore_with_writer<W: Write>(
    writer: &mut W,
    plan: &TerminalProtocolPlan,
    modes: &mut ActiveTerminalModes,
    disable_raw: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let mut write_res = Ok(());
    if !plan.leave_bytes.is_empty() {
        write_res = writer.write_all(&plan.leave_bytes);
    }
    if write_res.is_ok() && modes.focus_change {
        write_res = queue!(writer, DisableFocusChange);
    }
    if write_res.is_ok() && modes.mouse_capture {
        write_res = queue!(writer, DisableMouseCapture);
    }
    if write_res.is_ok() && modes.bracketed_paste {
        write_res = queue!(writer, DisableBracketedPaste);
    }
    if write_res.is_ok() && modes.alternate_screen {
        write_res = queue!(writer, LeaveAlternateScreen);
    }
    if write_res.is_ok() {
        write_res = writer.flush();
    }

    let raw_res = if modes.raw_mode {
        disable_raw()
    } else {
        Ok(())
    };
    *modes = ActiveTerminalModes::default();

    write_res.and(raw_res)
}

pub struct TerminalSessionHost {
    adapter: Arc<dyn TerminalAdapter>,
    env: TerminalEnv,
    capabilities: TerminalCapabilities,
    plan: TerminalProtocolPlan,
    active_modes: ActiveTerminalModes,
    pending_input_bytes: Vec<u8>,
    active: bool,
    owns_real_tty: bool,
    host_id: u64,
}

impl TerminalSessionHost {
    pub fn new(
        adapter: Arc<dyn TerminalAdapter>,
        env: TerminalEnv,
        opts: TerminalSessionOptions,
    ) -> io::Result<Self> {
        if !(io::stdout().is_terminal() && io::stdin().is_terminal()) {
            return Ok(Self::inactive(adapter, env, opts));
        }

        let capabilities = adapter.capabilities(&env);
        let plan = adapter.protocol_plan(&env, &opts);
        let host_id = NEXT_HOST_ID.fetch_add(1, Ordering::Relaxed);
        install_panic_restore_hook();

        enable_raw_mode()?;
        let mut active_modes = ActiveTerminalModes {
            raw_mode: true,
            alternate_screen: opts.alternate_screen,
            mouse_capture: opts.mouse_capture,
            bracketed_paste: opts.bracketed_paste,
            focus_change: opts.focus_change,
            keyboard_enhanced: plan.expects_kitty_pop,
        };

        if let Ok(mut guard) = PANIC_RESTORE_STATE.lock() {
            *guard = Some(PanicRestoreState {
                host_id,
                plan: plan.clone(),
                active_modes,
            });
        }

        let mut stdout = io::stdout().lock();
        if let Err(err) = write_enter_sequence(&mut stdout, &opts, &plan) {
            if let Ok(mut guard) = PANIC_RESTORE_STATE.lock() {
                if guard.as_ref().is_some_and(|st| st.host_id == host_id) {
                    *guard = None;
                }
            }
            let _ = restore_with_writer(&mut stdout, &plan, &mut active_modes, disable_raw_mode);
            return Err(err);
        }

        Ok(Self {
            adapter,
            env,
            capabilities,
            plan,
            active_modes,
            pending_input_bytes: Vec::new(),
            active: true,
            owns_real_tty: true,
            host_id,
        })
    }

    pub fn inactive(
        adapter: Arc<dyn TerminalAdapter>,
        env: TerminalEnv,
        opts: TerminalSessionOptions,
    ) -> Self {
        let capabilities = adapter.capabilities(&env);
        let plan = adapter.protocol_plan(&env, &opts);
        Self {
            adapter,
            env,
            capabilities,
            plan,
            active_modes: ActiveTerminalModes::default(),
            pending_input_bytes: Vec::new(),
            active: false,
            owns_real_tty: false,
            host_id: NEXT_HOST_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn enter_with_writer<W: Write>(
        adapter: Arc<dyn TerminalAdapter>,
        env: TerminalEnv,
        opts: TerminalSessionOptions,
        writer: &mut W,
        enable_raw: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Self> {
        let capabilities = adapter.capabilities(&env);
        let plan = adapter.protocol_plan(&env, &opts);
        enable_raw()?;

        let mut active_modes = ActiveTerminalModes {
            raw_mode: true,
            alternate_screen: opts.alternate_screen,
            mouse_capture: opts.mouse_capture,
            bracketed_paste: opts.bracketed_paste,
            focus_change: opts.focus_change,
            keyboard_enhanced: plan.expects_kitty_pop,
        };

        if let Err(err) = write_enter_sequence(writer, &opts, &plan) {
            let _ = restore_with_writer(writer, &plan, &mut active_modes, || Ok(()));
            return Err(err);
        }

        Ok(Self {
            adapter,
            env,
            capabilities,
            plan,
            active_modes,
            pending_input_bytes: Vec::new(),
            active: true,
            owns_real_tty: false,
            host_id: NEXT_HOST_ID.fetch_add(1, Ordering::Relaxed),
        })
    }

    pub fn leave_with_writer<W: Write>(
        &mut self,
        writer: &mut W,
        disable_raw: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        if self.owns_real_tty {
            if let Ok(mut guard) = PANIC_RESTORE_STATE.lock() {
                if guard.as_ref().is_some_and(|st| st.host_id == self.host_id) {
                    *guard = None;
                }
            }
        }
        restore_with_writer(writer, &self.plan, &mut self.active_modes, disable_raw)
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn active_modes(&self) -> ActiveTerminalModes {
        self.active_modes
    }

    pub fn adapter(&self) -> &Arc<dyn TerminalAdapter> {
        &self.adapter
    }

    pub fn env(&self) -> &TerminalEnv {
        &self.env
    }

    pub fn capabilities(&self) -> &TerminalCapabilities {
        &self.capabilities
    }

    pub fn plan(&self) -> &TerminalProtocolPlan {
        &self.plan
    }

    pub fn take_pending_input_bytes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_input_bytes)
    }
}

impl Drop for TerminalSessionHost {
    fn drop(&mut self) {
        if !(self.active && self.owns_real_tty) {
            return;
        }
        self.active = false;
        let still_registered = PANIC_RESTORE_STATE
            .lock()
            .ok()
            .is_some_and(|mut guard| {
                if guard.as_ref().is_some_and(|st| st.host_id == self.host_id) {
                    *guard = None;
                    true
                } else {
                    false
                }
            });
        if still_registered || !std::thread::panicking() {
            let mut stdout = io::stdout().lock();
            let _ = restore_with_writer(
                &mut stdout,
                &self.plan,
                &mut self.active_modes,
                disable_raw_mode,
            );
        }
    }
}
