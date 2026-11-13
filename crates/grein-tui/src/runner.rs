use crate::render;
use crate::state::AppState;
use crossterm::terminal;
use ratatui::backend::Backend;
use ratatui::Terminal;
use std::io::{self, IsTerminal};

pub struct TerminalGuard {
    raw_mode: bool,
    alt_screen: bool,
}

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        let mut raw_mode = false;
        let mut alt_screen = false;

        if io::stdout().is_terminal() {
            terminal::enable_raw_mode()?;
            raw_mode = true;
            if let Err(e) = crossterm::execute!(io::stdout(), terminal::EnterAlternateScreen) {
                let _ = terminal::disable_raw_mode();
                return Err(e);
            }
            alt_screen = true;
        }

        Ok(Self {
            raw_mode,
            alt_screen,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.alt_screen {
            let _ = crossterm::execute!(io::stdout(), terminal::LeaveAlternateScreen);
        }
        if self.raw_mode {
            let _ = terminal::disable_raw_mode();
        }
    }
}

pub fn handle_event(_app_state: &mut AppState, event: crossterm::event::Event) -> io::Result<bool> {
    match event {
        crossterm::event::Event::Key(key) => {
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
                && key.code == crossterm::event::KeyCode::Char('c')
            {
                return Ok(true);
            }
            if key.code == crossterm::event::KeyCode::Char('q') {
                return Ok(true);
            }
            Ok(false)
        }
        crossterm::event::Event::Resize(_, _) => Ok(false),
        _ => Ok(false),
    }
}

pub fn draw_frame<B: Backend>(terminal: &mut Terminal<B>, app_state: &AppState) -> io::Result<()> {
    terminal
        .draw(|f| {
            render::render_app(f, app_state);
        })
        .map(|_| ())
        .map_err(io::Error::other)
}

pub async fn run() -> io::Result<()> {
    let _guard = TerminalGuard::new()?;
    let stdout = io::stdout();
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app_state = AppState::new();

    loop {
        draw_frame(&mut terminal, &app_state)?;

        if crossterm::event::poll(std::time::Duration::from_millis(50))? {
            let event = crossterm::event::read()?;
            if handle_event(&mut app_state, event)? {
                break;
            }
        }
    }

    Ok(())
}
