//! Catch stray stderr writes + panic messages and route them into the
//! [`App::logs`] ring buffer so they render via the status-bar ticker
//! instead of corrupting the cell buffer.
//!
//! The TUI owns the terminal via crossterm's alt-screen + raw mode, so
//! anything written directly to stderr lands on top of our cells and
//! mangles the display. This module installs two traps:
//!
//! 1. A `std::panic::set_hook` closure that formats the panic info and
//!    forwards it via a static `mpsc::Sender` before skipping the default
//!    hook. This catches panics in the chat background thread (and the
//!    main thread, before Drop has a chance to restore the terminal).
//! 2. On unix, a `dup2(pipe_write, 2)` redirection of fd 2 into a pipe
//!    whose read end is drained by a reader thread. Anything that bypasses
//!    the panic hook — direct `eprintln!`, C-library stderr writes, the
//!    Rust std I/O machinery's backtrace output — gets captured too.
//!
//! Both paths feed the same `Receiver<CapturedLine>` which the main loop
//! drains every frame via [`drain_captured`]. On drop, fd 2 is restored
//! so the shell sees any final output after we leave alt-screen.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::OnceLock;

use crate::app::{App, LogLevel};

/// Severity we tag captured lines with when we push them into the log store.
#[derive(Debug, Clone, Copy)]
pub enum CapturedSource {
    /// A panic hook fired. Logged as `panic` at error level.
    Panic,
    /// A raw stderr write. Logged as `stderr` at warn level — we can't
    /// distinguish an innocuous info message from a real error.
    Stderr,
}

/// A single captured message bound for the log store.
#[derive(Debug)]
pub struct CapturedLine {
    pub source: CapturedSource,
    pub text: String,
}

/// Global sender used by the panic hook (which must be `'static`).
static CAPTURE_TX: OnceLock<Sender<CapturedLine>> = OnceLock::new();

/// Handle to the capture. Holding this keeps the pipe alive; dropping it
/// restores the original stderr fd on unix.
pub struct Capture {
    /// Pull loop drains this each frame.
    pub rx: Receiver<CapturedLine>,
    #[cfg(unix)]
    original_stderr: Option<std::os::fd::OwnedFd>,
}

impl Capture {
    /// Install panic hook + stderr redirection. Must be called before
    /// entering alt-screen so any early stderr is captured too.
    pub fn install() -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = CAPTURE_TX.set(tx.clone());

        // Swap in our panic hook. We skip the default hook — it writes
        // formatted panic info to stderr, which would still land in the
        // captured pipe but with extra terminal control bytes we don't
        // need. The structured line we emit here is enough.
        std::panic::set_hook(Box::new(move |info| {
            let msg = format_panic(info);
            if let Some(tx) = CAPTURE_TX.get() {
                let _ = tx.send(CapturedLine {
                    source: CapturedSource::Panic,
                    text: msg,
                });
            }
        }));

        #[cfg(unix)]
        let original_stderr = redirect_stderr(tx).ok();
        #[cfg(not(unix))]
        let _ = tx;

        Self {
            rx,
            #[cfg(unix)]
            original_stderr,
        }
    }
}

#[cfg(unix)]
impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(orig) = self.original_stderr.take() {
            // SAFETY: restoring the saved fd via dup2 — the original was
            // captured via dup(2) and is still open; orig.as_raw_fd() is
            // valid for the duration of this call, and fd 2 is a well-known
            // descriptor we're allowed to redirect.
            unsafe {
                use std::os::fd::AsRawFd;
                libc::dup2(orig.as_raw_fd(), 2);
            }
        }
    }
}

#[cfg(unix)]
fn redirect_stderr(tx: Sender<CapturedLine>) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::io::{BufRead, BufReader};
    use std::os::fd::{FromRawFd, OwnedFd};

    // SAFETY: `libc::pipe` takes a pointer to a 2-element int array it
    // fills with the read and write fds. We immediately wrap both in
    // `OwnedFd` so they're closed on drop.
    let mut fds = [0 as libc::c_int; 2];
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };

    // Save the current stderr and point fd 2 at the write end.
    let original_raw = unsafe { libc::dup(2) };
    if original_raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let original = unsafe { OwnedFd::from_raw_fd(original_raw) };

    use std::os::fd::AsRawFd;
    let dup2_rc = unsafe { libc::dup2(write_fd.as_raw_fd(), 2) };
    if dup2_rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // `write_fd` can be dropped now — fd 2 is the sole reference that
    // keeps the pipe write end open. When we restore fd 2 on shutdown,
    // the read end will see EOF and the reader thread will exit.
    drop(write_fd);

    // Reader thread. Forwards each line as it arrives.
    std::thread::spawn(move || {
        let file = std::fs::File::from(read_fd);
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let Ok(text) = line else { break };
            if text.is_empty() {
                continue;
            }
            if tx
                .send(CapturedLine {
                    source: CapturedSource::Stderr,
                    text,
                })
                .is_err()
            {
                break;
            }
        }
    });

    Ok(original)
}

fn format_panic(info: &std::panic::PanicHookInfo<'_>) -> String {
    let loc = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_string());
    let msg = info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
        .unwrap_or("<non-string panic>");
    let thread = std::thread::current()
        .name()
        .map(str::to_string)
        .unwrap_or_else(|| "<unnamed>".to_string());
    format!("panic in thread '{thread}' at {loc}: {msg}")
}

/// Drain all ready captured lines into `App::logs`. Safe to call every frame.
pub fn drain_captured(app: &mut App, capture: &Capture) {
    while let Ok(CapturedLine { source, text }) = capture.rx.try_recv() {
        let (level, tag) = match source {
            CapturedSource::Panic => (LogLevel::Error, "panic"),
            CapturedSource::Stderr => (LogLevel::Warn, "stderr"),
        };
        app.log(level, tag, text);
    }
}
