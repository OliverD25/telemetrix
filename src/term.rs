use ratatui::DefaultTerminal;

/// Owns the terminal in raw mode and the alternate screen. `ratatui::init`
/// also installs a panic hook that restores the terminal; `Drop` covers every
/// normal and `?` exit path.
pub struct TerminalGuard {
    pub terminal: DefaultTerminal,
}

impl TerminalGuard {
    pub fn new() -> Self {
        Self {
            terminal: ratatui::init(),
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}
