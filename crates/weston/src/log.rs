//! weston_log plumbing for the wrapper (plan §3k).
//!
//! Outbound logging always formats in Rust and passes the result as a
//! `%s` argument — client-controlled text must never reach C as a format
//! string (§3i).  Inbound, the shim's C `vlog`/`vlog_continue` handlers
//! forward formatted lines to [`wsys_rust_log_sink`].

use std::cell::RefCell;
use std::ffi::CString;
use std::fs::File;
use std::io::Write;

thread_local! {
    /// Destination for the shim's log handlers: a --log file when the
    /// frontend set one, stderr otherwise.  Thread-local per §3j (all
    /// logging happens on the libweston thread).
    static LOG_FILE: RefCell<Option<File>> = const { RefCell::new(None) };
}

/// Route subsequent log output to `path` (append, like the C
/// frontend's weston_log_file_open).  Called once at startup by the
/// Rust frontend before any other logging.
pub fn set_log_file(path: &std::path::Path) -> std::io::Result<()> {
    let f = File::options().create(true).append(true).open(path)?;
    LOG_FILE.with(|l| *l.borrow_mut() = Some(f));
    Ok(())
}

/// Public logging entry for the safe frontend crates: one line through
/// weston_log (reaches the file/stderr sink installed above).
pub fn message(msg: &str) {
    log_line(msg);
}

/// Emit one line through weston_log (goes to the handlers installed via
/// C sigchld_handler's per-child exit lines (main.c:401-409), shared by
/// every frontend-tracked child (xwayland, screenshooter).
pub(crate) fn log_child_exit(path: &str, status: i32) {
    if libc::WIFEXITED(status) {
        log_line(&format!(
            "{path} exited with status {}",
            libc::WEXITSTATUS(status)
        ));
    } else if libc::WIFSIGNALED(status) {
        log_line(&format!("{path} died on signal {}", libc::WTERMSIG(status)));
    } else {
        log_line(&format!("{path} disappeared"));
    }
}

/// the shim; before a compositor/log context exists it still reaches the
/// handler pair, which is why the panic barrier can use it early).
pub(crate) fn log_line(msg: &str) {
    // Sanitize interior NULs rather than fail: this is the logging path
    // the panic barrier depends on.
    let c = CString::new(msg.replace('\0', "\u{fffd}"))
        .unwrap_or_else(|_| CString::new("westonite: <unloggable>").expect("static"));
    // SAFETY: weston_log is a plain variadic; "%s\n" consumes exactly the
    // one pointer argument we pass, which outlives the call.
    unsafe {
        weston_sys::weston_log(c"%s\n".as_ptr(), c.as_ptr());
    }
}

/// Install the shim's vlog/vlog_continue handlers (idempotent enough for
/// R0: last install wins, matching weston_log_set_handler semantics).
pub fn install_stderr_handlers() {
    // SAFETY: registers two C functions defined in the shim; they stay
    // valid for the process lifetime.
    unsafe { weston_sys::wsys_install_log_handlers() }
}

/// Rust sink for the shim's handlers: write to stderr, return the char
/// count as weston_log expects.  `extern "C"`: called from the shim with
/// a borrowed buffer valid only for this call (§3a kind 5).
#[unsafe(no_mangle)]
extern "C" fn wsys_rust_log_sink(buf: *const libc::c_char, len: usize, _cont: bool) -> libc::c_int {
    crate::panic_barrier::guard("wsys_rust_log_sink", || {
        if buf.is_null() {
            return 0;
        }
        // SAFETY: the shim passes a buffer of exactly `len` initialized
        // bytes, valid for the duration of this call.
        let bytes = unsafe { std::slice::from_raw_parts(buf.cast::<u8>(), len) };
        LOG_FILE.with(|l| {
            let mut slot = l.borrow_mut();
            match slot.as_mut() {
                Some(f) => {
                    let _ = f.write_all(bytes);
                    let _ = f.flush();
                }
                None => {
                    let _ = std::io::stderr().lock().write_all(bytes);
                }
            }
        });
        len as libc::c_int
    })
}
