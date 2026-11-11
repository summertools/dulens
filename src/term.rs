//! Shared terminal lifecycle for full-screen views.

use std::io;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::execute;
use crossterm::style::ResetColor;
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};

pub(crate) struct TerminalGuard;

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(
            io::stdout(),
            EnterAlternateScreen,
            Hide,
            Clear(ClearType::All),
            MoveTo(0, 0)
        ) {
            restore_terminal();
            return Err(error);
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

pub(crate) fn restore_terminal() {
    let _ = execute!(
        io::stdout(),
        Show,
        ResetColor,
        LeaveAlternateScreen,
        MoveTo(0, 0)
    );
    let _ = disable_raw_mode();
}

/// Terminal dimensions with a small sanity floor for drawing code.
pub(crate) fn size() -> io::Result<(usize, usize)> {
    let (width, height) = terminal::size()?;
    Ok((width as usize, height as usize))
}
