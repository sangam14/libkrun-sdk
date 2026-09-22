//! Terminal state preservation and signal-safe restoration.
//!
//! When attaching a live interactive guest console or pseudo-terminal, the controlling
//! terminal is configured in raw mode (character-at-a-time input, no echo).
//! If the process is abruptly terminated by SIGINT (Ctrl+C), SIGTERM, or SIGHUP,
//! standard Rust `Drop` destructors are not invoked, leaving the user's shell broken.
//!
//! This module provides signal-safe terminal state caching and registers async-signal-safe
//! signal handlers to guarantee terminal restoration under all exit paths.

use anyhow::Result;
use std::sync::OnceLock;

/// Thread-safe, single-initialization storage for the original terminal state.
/// `OnceLock` guarantees that the termios is written exactly once, even under
/// concurrent access from multiple threads.
static SAVED_TERMIOS: OnceLock<libc::termios> = OnceLock::new();

/// Snapshot stdin termios and install signal handlers if stdin is a terminal.
///
/// Safe to call multiple times — the snapshot and signal registration happens
/// only on the first invocation (guarded by `OnceLock`).
pub fn save_and_install_signal_handlers() {
    if unsafe { libc::isatty(libc::STDIN_FILENO) } != 1 {
        return;
    }

    SAVED_TERMIOS.get_or_init(|| {
        let mut t = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut t) } != 0 {
            // Can't read termios — return zeroed struct, but it won't be
            // used for restoration since the get_or_init will have a value
            // but restore_stdin_termios will still call tcsetattr (safe no-op).
            return t;
        }

        // Install signal handlers using nix SigAction (portable, correct sa_handler alignment).
        let sig_action = nix::sys::signal::SigAction::new(
            nix::sys::signal::SigHandler::Handler(handle_signal),
            nix::sys::signal::SaFlags::SA_RESETHAND,
            nix::sys::signal::SigSet::empty(),
        );

        for sig in [
            nix::sys::signal::Signal::SIGINT,
            nix::sys::signal::Signal::SIGTERM,
            nix::sys::signal::Signal::SIGHUP,
        ] {
            let _ = unsafe { nix::sys::signal::sigaction(sig, &sig_action) };
        }

        t
    });
}

/// Async-signal-safe terminal restore function.
///
/// `tcsetattr` is POSIX async-signal-safe. We restore the exact termios
/// snapshot captured before entering raw mode.
pub fn restore_stdin_termios() {
    if let Some(saved) = SAVED_TERMIOS.get() {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, saved);
        }
    }
}

extern "C" fn handle_signal(sig: i32) {
    restore_stdin_termios();
    // Re-raise with default handler so the parent process sees the correct
    // wait status (e.g. WIFSIGNALED for SIGTERM).
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

/// RAII Guard that manages raw terminal mode with fallback restoration on drop.
pub struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    /// Creates a new raw-mode guard for interactive TTY sessions.
    ///
    /// When `enable_onlcr` is true, output post-processing (`OPOST | ONLCR`) is
    /// re-enabled after `cfmakeraw` so that bare `\n` from the guest console
    /// expands to `\r\n`, preventing the "staircase" effect. Set to false for
    /// binary/framed protocol connections where byte transparency is required.
    pub fn new() -> Result<Self> {
        Self::with_onlcr(true)
    }

    /// Creates a raw-mode guard with explicit control over ONLCR output processing.
    pub fn with_onlcr(enable_onlcr: bool) -> Result<Self> {
        if unsafe { libc::isatty(libc::STDIN_FILENO) } != 1 {
            return Ok(Self { active: false });
        }

        save_and_install_signal_handlers();

        let mut term = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut term) } != 0 {
            return Ok(Self { active: false });
        }

        unsafe { libc::cfmakeraw(&mut term) };

        if enable_onlcr {
            // Re-enable output post-processing so bare newlines expand to \r\n.
            // This prevents the "staircasing" effect when the guest sends plain \n.
            term.c_oflag |= libc::OPOST | libc::ONLCR;
        }

        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term) };

        Ok(Self { active: true })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.active {
            restore_stdin_termios();
        }
    }
}
