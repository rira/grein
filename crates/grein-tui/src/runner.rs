use std::io;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::app::{App, Pane};

pub fn run<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        terminal.draw(|f| crate::ui::render(f, app))?;
        if event::poll(std::time::Duration::from_millis(50))? {
            handle_event(app, event::read()?)?;
        }
    }
    Ok(())
}

pub fn handle_event(app: &mut App, event: Event) -> io::Result<()> {
    if let Event::Key(key) = event {
        if key.kind == KeyEventKind::Press {
            match (key.modifiers, key.code) {
                (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                    app.should_quit = true;
                }
                (KeyModifiers::CONTROL, KeyCode::Char('b')) => {
                    app.focus_prev();
                }
                (KeyModifiers::SHIFT, KeyCode::Tab) | (_, KeyCode::BackTab) => {
                    app.focus_prev();
                }
                (KeyModifiers::NONE, KeyCode::Tab) => {
                    app.focus_next();
                }
                (_, KeyCode::Esc) => {
                    if app.focus == Pane::Composer {
                        app.focus_next();
                    }
                }
                (_, KeyCode::Char('q')) if app.focus != Pane::Composer => {
                    app.should_quit = true;
                }
                (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c))
                    if app.focus == Pane::Composer =>
                {
                    app.input_buffer.push(c);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    fn make_key_event(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn test_q_does_not_exit_when_composer_has_focus_and_appends_to_input_buffer() {
        let mut app = App::default();
        app.focus = Pane::Composer;
        app.input_buffer.clear();
        app.should_quit = false;

        handle_event(&mut app, make_key_event(KeyCode::Char('q'), KeyModifiers::NONE))
            .expect("handle_event should succeed");

        assert!(
            !app.should_quit,
            "Regression: pressing 'q' while Composer has focus must NOT exit the application"
        );
        assert_eq!(
            app.input_buffer,
            "q",
            "Pressing 'q' while Composer has focus should route character into input_buffer"
        );
    }

    #[test]
    fn test_q_exits_when_composer_does_not_have_focus() {
        let mut app = App::default();
        if app.focus == Pane::Composer {
            app.focus_next();
        }
        assert_ne!(app.focus, Pane::Composer);
        app.should_quit = false;

        handle_event(&mut app, make_key_event(KeyCode::Char('q'), KeyModifiers::NONE))
            .expect("handle_event should succeed");

        assert!(
            app.should_quit,
            "Pressing 'q' when Composer does not have focus must exit the application"
        );
    }

    #[test]
    fn test_ctrl_c_unconditionally_exits_even_when_composer_focused() {
        let mut app = App::default();
        app.focus = Pane::Composer;
        app.should_quit = false;

        handle_event(&mut app, make_key_event(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .expect("handle_event should succeed");

        assert!(
            app.should_quit,
            "Ctrl+C must unconditionally exit the application regardless of active focus"
        );
    }

    #[test]
    fn test_character_input_routed_to_input_buffer_when_composer_focused() {
        let mut app = App::default();
        app.focus = Pane::Composer;
        app.input_buffer.clear();

        for ch in "hello world".chars() {
            handle_event(&mut app, make_key_event(KeyCode::Char(ch), KeyModifiers::NONE))
                .expect("handle_event should succeed");
        }

        assert_eq!(
            app.input_buffer,
            "hello world",
            "All typed characters should append to input_buffer when Composer has focus"
        );
        assert!(!app.should_quit);
    }

    #[test]
    fn test_tab_routes_focus_forward() {
        let mut app = App::default();
        let initial_focus = app.focus;

        handle_event(&mut app, make_key_event(KeyCode::Tab, KeyModifiers::NONE))
            .expect("handle_event should succeed");

        assert_ne!(
            app.focus,
            initial_focus,
            "Tab key must advance focus to the next pane"
        );
    }

    #[test]
    fn test_backtab_and_shift_tab_routes_focus_backward() {
        let mut app = App::default();
        let initial_focus = app.focus;

        handle_event(&mut app, make_key_event(KeyCode::Tab, KeyModifiers::NONE))
            .expect("advance focus with tab");
        assert_ne!(app.focus, initial_focus);

        handle_event(&mut app, make_key_event(KeyCode::BackTab, KeyModifiers::NONE))
            .expect("handle_event BackTab should succeed");
        assert_eq!(
            app.focus,
            initial_focus,
            "BackTab must cycle focus backward to previous pane"
        );

        handle_event(&mut app, make_key_event(KeyCode::Tab, KeyModifiers::NONE))
            .expect("advance focus with tab");
        assert_ne!(app.focus, initial_focus);

        handle_event(&mut app, make_key_event(KeyCode::Tab, KeyModifiers::SHIFT))
            .expect("handle_event Shift+Tab should succeed");
        assert_eq!(
            app.focus,
            initial_focus,
            "Shift+Tab must cycle focus backward to previous pane"
        );
    }

    #[test]
    fn test_ctrl_b_routes_focus_backward() {
        let mut app = App::default();
        let initial_focus = app.focus;

        handle_event(&mut app, make_key_event(KeyCode::Tab, KeyModifiers::NONE))
            .expect("advance focus with tab");
        assert_ne!(app.focus, initial_focus);

        handle_event(&mut app, make_key_event(KeyCode::Char('b'), KeyModifiers::CONTROL))
            .expect("handle_event Ctrl+B should succeed");
        assert_eq!(
            app.focus,
            initial_focus,
            "Ctrl+B must route to moving focus backward across panes"
        );
    }

    #[test]
    fn test_esc_routes_focus_away_from_composer() {
        let mut app = App::default();
        app.focus = Pane::Composer;

        handle_event(&mut app, make_key_event(KeyCode::Esc, KeyModifiers::NONE))
            .expect("handle_event Esc should succeed");

        assert_ne!(
            app.focus,
            Pane::Composer,
            "Esc must route focus away from Composer pane"
        );
    }
}