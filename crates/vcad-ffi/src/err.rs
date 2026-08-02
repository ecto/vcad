//! Thread-local last-error channel for the FFI boundary.
//!
//! The original kernel entry points signal failure as a null handle or a `0`
//! return, which is enough when the only possible failure is "that solid
//! didn't tessellate". The simulation surface is different: an env can fail to
//! build for a dozen distinct, *user-actionable* reasons — a document with no
//! floating base, an unknown end-effector id, a malformed config — and a
//! native app that can only say "simulation failed" makes every one of them a
//! debugging session.
//!
//! So simulation entry points still return null/0 (the ABI contract is
//! unchanged and callers that ignore errors keep working), and additionally
//! record a human-readable reason here, which Swift reads with
//! [`vcad_last_error`].
//!
//! The slot is thread-local because that is the only way to make it sound
//! without a lock: two threads failing concurrently must not overwrite each
//! other's diagnosis. Callers must therefore read the error on the same thread
//! that made the failing call — which is the natural usage, since the error is
//! read immediately after the null return.

use std::cell::RefCell;
use std::ptr;

thread_local! {
    /// Most recent error message set on this thread, if any.
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Record `msg` as this thread's last error.
pub(crate) fn set_error(msg: impl Into<String>) {
    let msg = msg.into();
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(msg));
}

/// Clear this thread's error slot. Called at the top of every fallible entry
/// point so a stale message from an earlier call can't be mistaken for the
/// current one's diagnosis.
pub(crate) fn clear_error() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
}

/// Run `f`, recording `context: <err>` and returning `None` if it fails.
pub(crate) fn ctx<T, E: std::fmt::Display>(
    context: &str,
    f: impl FnOnce() -> Result<T, E>,
) -> Option<T> {
    match f() {
        Ok(v) => Some(v),
        Err(e) => {
            set_error(format!("{context}: {e}"));
            None
        }
    }
}

/// Borrow this thread's last error message as UTF-8 bytes, writing its length
/// to `out_len`. Returns null when no error is recorded.
///
/// The pointer is valid until the next FFI call **on the same thread**; copy
/// the bytes before making another call. Reading from a different thread than
/// the one that failed always reports "no error" — see the module docs.
///
/// # Safety
///
/// `out_len` must be null or point to a writable `usize`.
#[no_mangle]
pub extern "C" fn vcad_last_error(out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe { *out_len = 0 };
    }
    LAST_ERROR.with(|slot| {
        let borrowed = slot.borrow();
        match borrowed.as_ref() {
            Some(msg) => {
                if !out_len.is_null() {
                    unsafe { *out_len = msg.len() };
                }
                // Sound because the `String` lives in the thread-local and is
                // only replaced by another call on this same thread, which the
                // documented contract forbids between borrow and copy.
                msg.as_ptr()
            }
            None => ptr::null(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_roundtrips_through_the_c_accessor() {
        clear_error();
        let mut len = 0usize;
        assert!(vcad_last_error(&mut len).is_null());
        assert_eq!(len, 0);

        set_error("no floating base");
        let p = vcad_last_error(&mut len);
        assert!(!p.is_null());
        let seen = unsafe { std::slice::from_raw_parts(p, len) };
        assert_eq!(std::str::from_utf8(seen).unwrap(), "no floating base");

        clear_error();
        assert!(vcad_last_error(&mut len).is_null());
    }

    #[test]
    fn errors_do_not_leak_across_threads() {
        set_error("thread A failed");
        let seen_on_b = std::thread::spawn(|| {
            let mut len = 0usize;
            vcad_last_error(&mut len).is_null()
        })
        .join()
        .unwrap();
        assert!(seen_on_b, "thread B must not observe thread A's error");
        clear_error();
    }

    #[test]
    fn ctx_records_the_failure_and_returns_none() {
        clear_error();
        let r: Option<u32> = ctx("build env", || "nope".parse::<u32>());
        assert!(r.is_none());
        let mut len = 0usize;
        let p = vcad_last_error(&mut len);
        let msg = std::str::from_utf8(unsafe { std::slice::from_raw_parts(p, len) }).unwrap();
        assert!(msg.starts_with("build env: "), "got {msg}");
        clear_error();
    }
}
