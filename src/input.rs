pub use crossterm::event::*;

pub fn should_quit(input: &Event) -> bool {
    matches!(input, Event::Key(KeyEvent {
                            code: KeyCode::Char('c' | 'd'),
                            modifiers,
                        }) if modifiers.contains(KeyModifiers::CONTROL))
}
