use std::io;
pub use tui::{backend::CrosstermBackend, Terminal};

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};

pub fn init_crossterm() -> io::Result<(Terminal<CrosstermBackend<io::Stdout>>, OnShutdown)> {
    terminal::enable_raw_mode()?;

    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(io::stdout());
    let term = Terminal::new(backend)?;

    let cleanup = OnShutdown::new(|| {
        // Be a good terminal citizen...
        let mut stdout = std::io::stdout();
        crossterm::execute!(stdout, LeaveAlternateScreen, DisableMouseCapture)?;
        terminal::disable_raw_mode()?;
        Ok(())
    });

    Ok((term, cleanup))
}

pub struct OnShutdown {
    action: fn() -> io::Result<()>,
}

impl OnShutdown {
    fn new(action: fn() -> io::Result<()>) -> Self {
        Self { action }
    }
}

impl Drop for OnShutdown {
    fn drop(&mut self) {
        let _ = (self.action)();
    }
}
