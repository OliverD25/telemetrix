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

/// Call before any other thread starts, so every thread inherits the
/// blocked signal mask (Unix).
pub fn install_signal_handling() {
    signals::install();
}

/// True once SIGTERM, SIGHUP (terminal closed) or SIGINT arrived. The loop
/// then returns normally, so `Drop` restores the terminal.
pub fn stop_requested() -> bool {
    signals::requested()
}

#[cfg(unix)]
mod signals {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    static STOP: AtomicBool = AtomicBool::new(false);
    /// Time the main loop gets to end by itself and restore the terminal.
    const GRACE: Duration = Duration::from_secs(1);

    /// The signals are blocked in every thread and received by one thread in
    /// `sigwait`: no handler code runs in signal context and nothing polls.
    /// When the terminal is gone, reading from it can spin inside crossterm
    /// and the loop never sees the flag, so after the grace time this thread
    /// restores what it can and ends the process itself.
    pub fn install() {
        // SAFETY: `set` is initialised by sigemptyset before use, and
        // pthread_sigmask only reads it.
        let set = unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for sig in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
                libc::sigaddset(&mut set, sig);
            }
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            set
        };
        let spawned = std::thread::Builder::new()
            .name("signals".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let mut sig: libc::c_int = 0;
                // SAFETY: `set` holds the blocked signals; `sig` is writable.
                if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
                    return;
                }
                STOP.store(true, Ordering::Relaxed);
                std::thread::sleep(GRACE);
                ratatui::restore();
                std::process::exit(128 + sig);
            });
        if spawned.is_err() {
            eprintln!("telemetrix: cannot start the signal thread");
        }
    }

    pub fn requested() -> bool {
        STOP.load(Ordering::Relaxed)
    }
}

/// Windows: closing the console ends the process; there is nothing to catch.
#[cfg(not(unix))]
mod signals {
    pub fn install() {}

    pub fn requested() -> bool {
        false
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}
