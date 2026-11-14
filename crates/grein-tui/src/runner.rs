use std::io;
use std::time::Duration;
use crossterm::event::{self, Event, KeyEvent};
use ratatui::{backend::Backend, Terminal};

use crate::app::App;

pub struct TerminalGuard;

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        Ok(Self)
    }
}

pub fn draw_frame<B: Backend>(_terminal: &mut Terminal<B>, _app: &App) -> io::Result<()> {
    Ok(())
}

pub fn handle_key_event(app: &mut App, key: KeyEvent) -> Result<Option<crate::app::TreeAction>, crate::app::TuiError> {
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

        if event::poll(Duration::from_millis(50))? {
            let event = event::read()?;
            let should_quit = handle_event(&mut app, event)?;
            if should_quit {
                break;
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use crate::app::{App, FocusPane};

    #[test]
    fn test_handle_event_ctrl_c_always_quits() {
        let mut app = App::new();
        app.focus = FocusPane::Composer;

        let event = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        let result = handle_event(&mut app, event);

        assert!(result.is_ok());
        assert!(app.should_quit);
    }

    #[test]
    fn test_handle_event_q_quits_when_not_in_composer() {
        let mut app = App::new();
        app.focused_pane = FocusPane::Tree;

        let event = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        let result = handle_event(&mut app, event);

        assert!(result.is_ok());
        assert!(app.should_quit);
    }

    #[test]
    fn test_handle_event_q_appends_to_composer_when_focused() {
        let mut app = App::new();
        app.set_focus(FocusPane::Composer);
        app.input_buffer.clear();

        let event = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        let result = handle_event(&mut app, event);

        assert!(result.is_ok());
        assert!(!app.should_quit);
        assert_eq!(app.composer_content(), "q");
    }

    #[test]
    fn test_handle_event_routes_chars_to_composer_buffer() {
        let mut app = App::new();
        app.set_focus(FocusPane::Composer);
        app.input_buffer.clear();

        for ch in "hello world".chars() {
            let event = Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
            let result = handle_event(&mut app, event);
            assert!(result.is_ok());
        }

        assert!(!app.should_quit);
        assert_eq!(app.composer_content(), "hello world");
    }

    #[test]
    fn test_handle_event_tab_cycles_focus() {
        let mut app = App::new();
        let initial_focus = app.focused_pane;

        let event = Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let result = handle_event(&mut app, event);

        assert!(result.is_ok());
        assert_ne!(app.focused_pane, initial_focus);
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
        app.set_focus(FocusPane::Composer);

        let event = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let result = handle_event(&mut app, event);

        assert!(result.is_ok());
        assert!(!app.should_quit);
        assert_eq!(app.focused_pane, FocusPane::Tree);
    }
}
