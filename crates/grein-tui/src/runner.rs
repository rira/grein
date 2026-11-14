use std::io::{self, IsTerminal};
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::app::App;
use crate::render::render_app;

pub struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        if io::stdout().is_terminal() {
            enable_raw_mode()?;
            execute!(io::stdout(), EnterAlternateScreen)?;
            Ok(Self { active: true })
        } else {
            Ok(Self { active: false })
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}

pub fn draw_frame<B: Backend>(terminal: &mut Terminal<B>, app: &App) -> io::Result<()> {
    terminal.draw(|f| render_app(f, app))?;
    Ok(())
}

pub fn handle_key_event(
    app: &mut App,
    key: KeyEvent,
) -> Result<Option<crate::app::TreeAction>, crate::app::TuiError> {
    let action = app.handle_key_tree(key);
    app.handle_key_event(key);
    Ok(action)
}

pub fn handle_event(app: &mut App, event: Event) -> io::Result<bool> {
    if let Event::Key(key) = event {
        app.handle_key_event(key);
        return Ok(app.should_quit);
    }
    Ok(false)
}

pub fn run<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> io::Result<()> {
    loop {
        draw_frame(terminal, &app)?;
        if app.should_quit {
            break;
        }
        if event::poll(Duration::from_millis(50))? {
            let ev = event::read()?;
            if handle_event(&mut app, ev)? {
                break;
            }
        }
    }
    Ok(())
}
