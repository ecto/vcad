//! OSC 11 terminal background probe.
//!
//! At startup (before the TUI enters alt-screen) we ask the terminal to
//! report its current background color by writing the OSC 11 query:
//!
//! ```text
//! ESC ] 11 ; ? ESC \
//! ```
//!
//! Compliant terminals respond with:
//!
//! ```text
//! ESC ] 11 ; rgb:RRRR/GGGG/BBBB ESC \
//! ```
//!
//! (some use BEL `\x07` as the terminator instead of ST `\x1b\\`, and a
//! handful emit 2-digit hex per channel instead of 4). We parse whatever
//! comes back, scale to 8-bit, and hand it to `theme.rs` so the Terminal
//! theme can derive a subtle surface shade a notch off the user's real
//! background — no hardcoded palette.
//!
//! The probe only runs on unix and only when stdin is an actual TTY. If
//! the terminal doesn't respond within ~200 ms, doesn't understand OSC 11,
//! or returns garbage, we fall back to the static defaults in
//! `theme::TERMINAL`.

#![cfg(unix)]

use std::io::{IsTerminal, Read, Write};
use std::os::fd::AsRawFd;

/// Probe the terminal for its current background color. Returns `None`
/// when stdin isn't a TTY, the terminal doesn't respond within the
/// timeout, or the response can't be parsed.
pub fn probe_background() -> Option<(u8, u8, u8)> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return None;
    }

    // Raw mode is required so the terminal's response comes back as raw
    // bytes instead of being buffered waiting for a newline. We bracket
    // the query tightly and restore cooked mode regardless of outcome.
    if crossterm::terminal::enable_raw_mode().is_err() {
        return None;
    }
    let result = query_and_read(&stdin);
    let _ = crossterm::terminal::disable_raw_mode();
    result
}

/// Send the query, poll for a response, parse it. Split out so the
/// raw-mode bracket in `probe_background` is easy to audit.
fn query_and_read(stdin: &std::io::Stdin) -> Option<(u8, u8, u8)> {
    // Send OSC 11 on stdout.
    {
        let mut out = std::io::stdout().lock();
        out.write_all(b"\x1b]11;?\x1b\\").ok()?;
        out.flush().ok()?;
    }

    // Poll stdin with a 200 ms timeout and loop until we see a
    // terminator or exceed the deadline. Some slow / remote terminals
    // split the response across multiple reads.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    let fd = stdin.as_raw_fd();
    let mut buffer = Vec::with_capacity(64);

    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32;

        if !poll_stdin(fd, timeout_ms) {
            break;
        }

        let mut chunk = [0u8; 64];
        let n = match stdin.lock().read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        buffer.extend_from_slice(&chunk[..n]);

        // Done when we've seen an OSC terminator.
        if buffer.contains(&0x07) || contains_st(&buffer) {
            break;
        }
        if buffer.len() > 256 {
            break; // runaway — something's wrong
        }
    }

    parse_osc11(&buffer)
}

/// `poll(fd, POLLIN, timeout_ms)` — returns true when stdin has data
/// ready, false on timeout or error.
fn poll_stdin(fd: std::os::fd::RawFd, timeout_ms: i32) -> bool {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pollfd points to a local value we own for the duration of
    // the call. nfds=1, timeout in ms.
    let ret = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
    ret > 0 && (pollfd.revents & libc::POLLIN) != 0
}

/// Look for ESC '\\' — the String Terminator sequence used by OSC.
fn contains_st(buf: &[u8]) -> bool {
    buf.windows(2).any(|w| w == [0x1b, b'\\'])
}

/// Parse an OSC 11 response body into an `(r, g, b)` triple. Exposed
/// (crate-private) for unit testing without touching stdin.
pub(crate) fn parse_osc11(bytes: &[u8]) -> Option<(u8, u8, u8)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let start = text.find("rgb:")? + 4;
    let remainder = &text[start..];

    // Terminator is either ESC '\\' (ST) or BEL (\x07). Anything past
    // that is a subsequent escape sequence we don't care about.
    let end = remainder
        .find('\x1b')
        .or_else(|| remainder.find('\x07'))
        .unwrap_or(remainder.len());
    let body = &remainder[..end];

    let parts: Vec<&str> = body.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parse_channel(parts[0])?,
        parse_channel(parts[1])?,
        parse_channel(parts[2])?,
    ))
}

/// Convert one channel's hex string to a u8. xterm emits 4 hex digits
/// (16-bit per channel); a handful of terminals use 2 digits.
fn parse_channel(s: &str) -> Option<u8> {
    match s.len() {
        4 => u16::from_str_radix(s, 16).ok().map(|v| (v >> 8) as u8),
        2 => u8::from_str_radix(s, 16).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xterm_4_digit_response() {
        let raw = b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\";
        assert_eq!(parse_osc11(raw), Some((0x1e, 0x1e, 0x1e)));
    }

    #[test]
    fn parses_2_digit_response() {
        let raw = b"\x1b]11;rgb:1e/2a/3f\x07";
        assert_eq!(parse_osc11(raw), Some((0x1e, 0x2a, 0x3f)));
    }

    #[test]
    fn parses_bel_terminated_response() {
        let raw = b"\x1b]11;rgb:ffff/ffff/ffff\x07";
        assert_eq!(parse_osc11(raw), Some((0xff, 0xff, 0xff)));
    }

    #[test]
    fn rejects_missing_rgb_prefix() {
        let raw = b"\x1b]11;something-else\x1b\\";
        assert_eq!(parse_osc11(raw), None);
    }

    #[test]
    fn rejects_wrong_channel_count() {
        let raw = b"\x1b]11;rgb:1e/2a\x1b\\";
        assert_eq!(parse_osc11(raw), None);
    }

    #[test]
    fn rejects_garbage_hex() {
        let raw = b"\x1b]11;rgb:zzzz/2222/3333\x1b\\";
        assert_eq!(parse_osc11(raw), None);
    }
}
