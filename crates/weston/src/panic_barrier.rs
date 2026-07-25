//! No unwinding across FFI (plan §3g, D16).
//!
//! `panic = "unwind"` stays on in all profiles; every `extern "C"`
//! trampoline body runs inside [`guard`], which catches any panic, logs
//! it with the callback's context through weston_log, and aborts
//! deliberately.

use std::panic::{self, AssertUnwindSafe};

/// Run a trampoline body.  On panic: log with `what` for context, then
/// abort the process.  Never unwinds into C.
pub(crate) fn guard<R>(what: &str, body: impl FnOnce() -> R) -> R {
    match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(r) => r,
        Err(payload) => {
            let msg: &str = if let Some(s) = payload.downcast_ref::<&str>() {
                s
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s
            } else {
                "<non-string panic payload>"
            };
            crate::log::log_line(&format!(
                "westonite: fatal: Rust panic in callback '{what}': {msg}; aborting"
            ));
            std::process::abort();
        }
    }
}
