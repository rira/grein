use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::app::{App, FocusPane};

pub struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        use std::io::IsTerminal;
        if io::stdout().is_terminal() {
            crossterm::terminal::enable_raw_mode()?;
            if let Err(e) =
                crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)
            {
                let _ = crossterm::terminal::disable_raw_mode();
                return Err(e);
            }
            Ok(Self { active: true })
        } else {
            Ok(Self { active: false })
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen);
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

pub fn draw_frame<B: Backend>(terminal: &mut Terminal<B>, app: &App) -> io::Result<()> {
    terminal.draw(|f| crate::render::render_app(f, app))?;
    Ok(())
}

pub fn handle_key_event(app: &mut App, key: KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') | KeyCode::Char('q') => {
                app.should_quit = true;
                return true;
            }
            KeyCode::Char('b') => {
                app.blackboard_collapsed = !app.blackboard_collapsed;
                return false;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Char('q') => {
            app.should_quit = true;
            true
        }
        KeyCode::Tab => {
            app.focused_pane = match app.focused_pane {
                FocusPane::Workspace | FocusPane::Transcript => FocusPane::Composer,
                FocusPane::Composer => FocusPane::Blackboard,
                FocusPane::Blackboard => FocusPane::Workspace,
            };
            false
        }
        KeyCode::BackTab => {
            app.focused_pane = match app.focused_pane {
                FocusPane::Workspace | FocusPane::Transcript => FocusPane::Blackboard,
                FocusPane::Composer => FocusPane::Workspace,
                FocusPane::Blackboard => FocusPane::Composer,
            };
            false
        }
        _ => false,
    }
}

pub fn handle_event(app: &mut App, event: Event) -> io::Result<bool> {
    match event {
        Event::Key(key) => Ok(handle_key_event(app, key)),
        Event::Resize(_, _) => Ok(false),
        _ => Ok(false),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn terminal_guard_succeeds_in_test_env() {
        let guard = TerminalGuard::new().expect("guard init");
        drop(guard);
    }

    #[test]
    fn handle_key_event_quits_on_q() {
        let mut app = App::new();
        let quit = handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        );
        assert!(quit);
        assert!(app.should_quit);
    }

    #[test]
    fn handle_key_event_cycles_focus_on_tab() {
        let mut app = App::new();
        assert_eq!(app.focused_pane, FocusPane::Workspace);
        handle_key_event(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane, FocusPane::Composer);
        handle_key_event(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane, FocusPane::Blackboard);
        handle_key_event(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focused_pane, FocusPane::Workspace);
    }

    #[test]
    fn draw_frame_renders_onto_test_backend() {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let app = App::new();
        draw_frame(&mut terminal, &app).expect("draw frame");
    }
}
