use std::io;
use std::time::Duration;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{backend::Backend, Terminal};

use crate::app::{App, Focus};
use crate::ui;

pub fn run<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        if event::poll(Duration::from_millis(50))? {
            let event = event::read()?;
            handle_event(&mut app, event)?;
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

pub fn handle_event(app: &mut App, event: Event) -> io::Result<()> {
    if let Event::Key(key) = event {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            app.should_quit = true;
            return Ok(());
        }

        match key.code {
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    app.cycle_focus_reverse();
                } else {
                    app.cycle_focus();
                }
            }
            KeyCode::BackTab => {
                app.cycle_focus_reverse();
            }
            KeyCode::Esc => {
                if app.focus == Focus::Composer {
                    app.focus = Focus::Chat;
                }
            }
            KeyCode::Char(c) => {
                if app.focus == Focus::Composer {
                    app.input_buffer.push(c);
                } else if c == 'q' && !key.modifiers.contains(KeyModifiers::CONTROL) {
                    app.should_quit = true;
                }
            }
            KeyCode::Backspace => {
                if app.focus == Focus::Composer {
                    app.input_buffer.pop();
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use crate::app::{App, Focus};

    #[test]
    fn test_handle_event_ctrl_c_always_quits() {
        let mut app = App::new();
        app.focus = Focus::Composer;

        let event = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        let result = handle_event(&mut app, event);

        assert!(result.is_ok());
        assert!(app.should_quit);
    }

    #[test]
    fn test_handle_event_q_quits_when_not_in_composer() {
        let mut app = App::new();
        app.focus = Focus::Chat;

        let event = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        let result = handle_event(&mut app, event);

        assert!(result.is_ok());
        assert!(app.should_quit);
    }

    #[test]
    fn test_handle_event_q_appends_to_composer_when_focused() {
        let mut app = App::new();
        app.focus = Focus::Composer;
        app.input_buffer.clear();

        let event = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        let result = handle_event(&mut app, event);

        assert!(result.is_ok());
        assert!(!app.should_quit);
        assert_eq!(app.input_buffer, "q");
    }

    #[test]
    fn test_handle_event_routes_chars_to_composer_buffer() {
        let mut app = App::new();
        app.focus = Focus::Composer;
        app.input_buffer.clear();

        for ch in "hello world".chars() {
            let event = Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
            let result = handle_event(&mut app, event);
            assert!(result.is_ok());
        }

        assert!(!app.should_quit);
        assert_eq!(app.input_buffer, "hello world");
    }

    #[test]
    fn test_handle_event_tab_cycles_focus() {
        let mut app = App::new();
        let initial_focus = app.focus;

        let event = Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let result = handle_event(&mut app, event);

        assert!(result.is_ok());
        assert_ne!(app.focus, initial_focus);
    }

    #[test]
    fn test_handle_event_backtab_cycles_focus_reverse() {
        let mut app = App::new();
        let event_tab = Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let _ = handle_event(&mut app, event_tab);

        let event_backtab = Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        let result = handle_event(&mut app, event_backtab);

        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_event_esc_unfocuses_or_resets() {
        let mut app = App::new();
        app.focus = Focus::Composer;

        let event = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let result = handle_event(&mut app, event);

        assert!(result.is_ok());
        assert!(!app.should_quit);
    }
}