//! weston_log plumbing for the wrapper (plan §3k, R2f).
//!
//! Outbound logging always formats in Rust and passes the result as a
//! `%s` argument — client-controlled text must never reach C as a format
//! string (§3i).
//!
//! Inbound is libweston's own machinery, as C wires it (main.c:4499):
//! the shim's `vlog`/`vlog_continue` timestamp each line and print it
//! into the **"log" scope**, and libweston's *subscribers* decide where
//! it goes — the log file, the in-memory flight recorder, or both.
//! [`LogContext`] owns that whole assembly and must be built by the
//! frontend before anything logs.
//!
//! Outside a `LogContext` — the R0 smoke binary, the unit harness, a
//! panic-barrier line after teardown — the shim falls back to writing
//! straight to [`wsys_rust_log_sink`], which appends to the same file
//! (or stderr).  C has no such fallback and would drop those lines.

use std::cell::RefCell;
use std::ffi::CString;
use std::fs::File;
use std::io::Write;
use std::os::unix::io::IntoRawFd;

/// C main.c DEFAULT_FLIGHT_REC_SIZE (main.c:76).
const DEFAULT_FLIGHT_REC_SIZE: usize = 5 * 1024 * 1024;
/// C main.c DEFAULT_FLIGHT_REC_SCOPES (main.c:77).
pub const DEFAULT_FLIGHT_REC_SCOPES: &str = "log,drm-backend";

thread_local! {
    /// Fallback destination for the shim handlers when no scope is
    /// installed: the --log file when one is open, stderr otherwise.
    /// Thread-local per §3j (all logging happens on the libweston
    /// thread).
    static LOG_FILE: RefCell<Option<File>> = const { RefCell::new(None) };
}

/// How the frontend wants its logging wired: C's `--log`,
/// `--logger-scopes` and `--flight-rec-scopes`, resolved.
#[derive(Debug, Clone, Default)]
pub struct LogSetup {
    /// `--log`: append here instead of stderr.
    pub file: Option<std::path::PathBuf>,
    /// `--logger-scopes` / `-l`.  Empty means C's default: subscribe
    /// the logger to "log" alone.
    pub logger_scopes: Vec<String>,
    /// `--flight-rec-scopes` / `-f`.  `None` means C's default list;
    /// an explicitly empty value disables the recorder entirely.
    pub flight_rec_scopes: Option<Vec<String>>,
}

/// C main.c's log context, log scope, log file and the two subscribers,
/// as one RAII owner.
///
/// Lifetime matters and mirrors main.c: created before the first log
/// line, handed to `weston_compositor_create` (borrowed — libweston
/// keeps the pointer), and destroyed *after* the compositor and the
/// display (main.c:4819).  Holding it in `main` and scoping the
/// compositor inside it gives that order for free.
pub struct LogContext {
    ctx: *mut weston_sys::weston_log_context,
    /// C main.c's `log_scope`.  Kept because libweston requires it be
    /// destroyed explicitly, before the context that owns it -- skip
    /// that and it warns "debug scope 'log' has not been destroyed"
    /// and leaks the scope (caught by the valgrind smoke).
    scope: *mut weston_sys::weston_log_scope,
    logger: *mut weston_sys::weston_log_subscriber,
    flight_rec: *mut weston_sys::weston_log_subscriber,
    /// The `FILE *` the log subscriber writes to.  Null when logging to
    /// stderr, which must never be fclosed.
    file: *mut weston_sys::FILE,
}

/// Why a [`LogContext`] could not be built.
#[derive(Debug)]
pub enum LogError {
    CtxCreate,
    ScopeCreate,
    FileOpen(std::path::PathBuf, std::io::Error),
}

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogError::CtxCreate => f.write_str("failed to initialize weston debug framework"),
            LogError::ScopeCreate => f.write_str("failed to create the \"log\" log scope"),
            LogError::FileOpen(p, e) => write!(f, "cannot open log file '{}': {e}", p.display()),
        }
    }
}

impl LogContext {
    /// C main.c:4499-4522, in order: create the context, add the "log"
    /// scope, open the log file, create the subscribers, subscribe.
    pub fn new(setup: &LogSetup) -> Result<LogContext, LogError> {
        // SAFETY: plain constructor; null-checked immediately.
        let ctx = unsafe { weston_sys::weston_log_ctx_create() };
        if ctx.is_null() {
            return Err(LogError::CtxCreate);
        }
        let mut lc = LogContext {
            ctx,
            scope: std::ptr::null_mut(),
            logger: std::ptr::null_mut(),
            flight_rec: std::ptr::null_mut(),
            file: std::ptr::null_mut(),
        };

        // SAFETY: ctx live; the name/description are static C strings
        // and libweston copies what it needs.  The three null callbacks
        // are what C passes (main.c:4505).
        let scope = unsafe {
            weston_sys::weston_log_ctx_add_log_scope(
                ctx,
                c"log".as_ptr(),
                c"Weston and Wayland log\n".as_ptr(),
                None,
                None,
                std::ptr::null_mut(),
            )
        };
        if scope.is_null() {
            return Err(LogError::ScopeCreate);
        }
        lc.scope = scope;

        // C weston_log_file_open (main.c:183): append, cloexec, line
        // buffered; stderr when there is no --log.  Opened through Rust
        // when there is a path, so a bad --log is a typed error rather
        // than a null FILE*, then handed to C as the FILE* the log
        // subscriber insists on.
        let dump_to = match &setup.file {
            Some(path) => {
                let f = File::options()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|e| LogError::FileOpen(path.clone(), e))?;
                // The fallback sink keeps its own handle on the same
                // file: it is used before this context exists and after
                // it is gone, and two appending descriptors on one file
                // interleave correctly.
                let dup = f
                    .try_clone()
                    .map_err(|e| LogError::FileOpen(path.clone(), e))?;
                LOG_FILE.with(|l| *l.borrow_mut() = Some(dup));
                // fdopen the descriptor we already validated rather
                // than fopen the path a second time: one open, one
                // error to report, and no window for the path to change
                // underneath us.  `into_raw_fd` hands ownership to the
                // stream, which Drop fcloses.
                lc.open_stream(f.into_raw_fd(), path)?
            }
            // A dup of stderr, not stderr itself: Drop fcloses whatever
            // it owns, and closing the real stderr would take every
            // later diagnostic with it.  C dodges this by never
            // fclosing when weston_logfile == stderr (main.c:208).
            None => {
                // SAFETY: fd 2 is open for the process lifetime.
                let fd = unsafe { libc::dup(2) };
                if fd < 0 {
                    return Err(LogError::FileOpen(
                        std::path::PathBuf::from("<stderr>"),
                        std::io::Error::last_os_error(),
                    ));
                }
                lc.open_stream(fd, std::path::Path::new("<stderr>"))?
            }
        };

        // SAFETY: dump_to is a live stream that outlives the
        // subscriber (Drop destroys the subscriber before the file).
        lc.logger = unsafe { weston_sys::weston_log_subscriber_create_log(dump_to) };

        // C: an absent --flight-rec-scopes means the default list; an
        // explicitly empty one disables the recorder (main.c:4515).
        let flight_scopes: Vec<String> = match &setup.flight_rec_scopes {
            Some(v) => v.clone(),
            None => DEFAULT_FLIGHT_REC_SCOPES
                .split(',')
                .map(str::to_string)
                .collect(),
        };
        if !flight_scopes.is_empty() {
            // SAFETY: constructor; owned by us, destroyed in Drop.
            lc.flight_rec = unsafe {
                weston_sys::weston_log_subscriber_create_flight_rec(DEFAULT_FLIGHT_REC_SIZE)
            };
        }

        // C weston_log_subscribe_to_scopes (main.c:4380).
        if setup.logger_scopes.is_empty() {
            lc.subscribe(lc.logger, "log");
        } else {
            for name in &setup.logger_scopes {
                lc.subscribe(lc.logger, name);
            }
        }
        if !lc.flight_rec.is_null() {
            for name in &flight_scopes {
                lc.subscribe(lc.flight_rec, name);
            }
        }

        // Only now do log lines have anywhere to go.
        // SAFETY: the scope outlives the handlers -- it belongs to
        // `ctx`, which Drop destroys last, and teardown re-installs the
        // fallback pair before that.
        unsafe { weston_sys::wsys_install_log_handlers(scope) };
        Ok(lc)
    }

    /// `fdopen` a descriptor we own into the `FILE *` the log
    /// subscriber requires, line-buffered as C's setvbuf does so a
    /// compositor that dies mid-run still leaves whole lines behind.
    /// The stream takes the descriptor over and Drop fcloses it.
    fn open_stream(
        &mut self,
        fd: std::os::unix::io::RawFd,
        path: &std::path::Path,
    ) -> Result<*mut weston_sys::FILE, LogError> {
        // SAFETY: `fd` is open and no longer owned by anything else;
        // fdopen takes it over.
        let fp = unsafe { libc::fdopen(fd, c"a".as_ptr()) };
        if fp.is_null() {
            let e = std::io::Error::last_os_error();
            // SAFETY: fdopen failed, so the descriptor is still ours.
            unsafe { libc::close(fd) };
            return Err(LogError::FileOpen(path.to_path_buf(), e));
        }
        // SAFETY: fp is a live stream we just created.
        unsafe { libc::setvbuf(fp, std::ptr::null_mut(), libc::_IOLBF, 256) };
        self.file = fp.cast();
        Ok(fp.cast())
    }

    fn subscribe(&self, subscriber: *mut weston_sys::weston_log_subscriber, name: &str) {
        if subscriber.is_null() {
            return;
        }
        let Ok(c) = CString::new(name) else { return };
        // SAFETY: ctx and subscriber live; libweston copies the name.
        unsafe { weston_sys::weston_log_subscribe(self.ctx, subscriber, c.as_ptr()) };
    }

    /// The raw context, for `weston_compositor_create`.  Borrowed:
    /// libweston keeps the pointer for the compositor's lifetime, which
    /// is why this owner has to outlive it.
    pub(crate) fn as_ptr(&self) -> *mut weston_sys::weston_log_context {
        self.ctx
    }

    /// Whether the flight recorder is running — C logs this at startup
    /// and gates the Super+D debug binding on it.
    pub fn flight_rec_enabled(&self) -> bool {
        !self.flight_rec.is_null()
    }

    pub(crate) fn flight_rec_ptr(&self) -> *mut weston_sys::weston_log_subscriber {
        self.flight_rec
    }
}

impl Drop for LogContext {
    fn drop(&mut self) {
        // Put the handlers back on the scope-free fallback first: the
        // teardown below destroys the scope this context installed, and
        // a log line in between (a panic-barrier report, say) would
        // otherwise print into freed memory.
        // SAFETY: passing null selects the fallback path in the shim.
        unsafe { weston_sys::wsys_install_log_handlers(std::ptr::null_mut()) };
        // SAFETY: destruction order is main.c:4817-4823 exactly — the
        // scope first, then the subscribers, then the context, then the
        // file.  The file is last because the log subscriber writes to
        // it right up until it is destroyed.
        unsafe {
            if !self.scope.is_null() {
                weston_sys::weston_log_scope_destroy(self.scope);
            }
            if !self.logger.is_null() {
                weston_sys::weston_log_subscriber_destroy(self.logger);
            }
            if !self.flight_rec.is_null() {
                weston_sys::weston_log_subscriber_destroy(self.flight_rec);
            }
            if !self.ctx.is_null() {
                weston_sys::weston_log_ctx_destroy(self.ctx);
            }
            if !self.file.is_null() {
                libc::fclose(self.file.cast());
            }
        }
    }
}

/// Public logging entry for the safe frontend crates: one line through
/// weston_log (reaches the file/stderr sink installed above).
pub fn message(msg: &str) {
    log_line(msg);
}

/// Emit one line through weston_log (goes to the handlers installed via
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

/// C main.c:4589: announce the pid and `raise(SIGSTOP)`, so a debugger
/// can attach to a compositor that has created nothing yet.  Resumes on
/// SIGCONT; if no one ever sends it, the process simply stays stopped,
/// which is the point.
///
/// Lives here rather than in the frontend because `raise` is unsafe and
/// the frontend crates are `forbid(unsafe_code)`.
pub fn wait_for_debugger() {
    log_line(&format!(
        "Weston PID is {} - waiting for debugger, send SIGCONT to continue...",
        std::process::id()
    ));
    // SAFETY: raise(SIGSTOP) on our own process; the default action
    // stops us until SIGCONT arrives.  No handler state is involved.
    unsafe { libc::raise(libc::SIGSTOP) };
}

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

/// Install the shim's vlog/vlog_continue handlers with **no scope**, so
/// weston_log reaches [`wsys_rust_log_sink`] directly.
///
/// This is the pre-[`LogContext`] path: the R0 smoke binary, the unit
/// harness, and the frontend's own earliest failures (a bad `--log`
/// path has to be reportable before the log file exists).  A
/// `LogContext` replaces them with the scope-aware pair and Drop puts
/// these back.
pub fn install_stderr_handlers() {
    // SAFETY: registers two C functions defined in the shim; they stay
    // valid for the process lifetime.  Null selects the fallback path.
    unsafe { weston_sys::wsys_install_log_handlers(std::ptr::null_mut()) }
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

/// C `flight_rec_key_binding_handler` (main.c:4371): dump the recorder's
/// ring buffer.  Registered as a *debug* binding (Super+Shift+Space
/// then D), so it costs nothing until someone asks for it.
///
/// The subscriber arrives as the binding's `data` — the one callback in
/// the tree that carries a payload rather than resolving through
/// `Ctx::current`, because C hands it the same way and the pointer is
/// owned by the frontend's [`LogContext`], which outlives every
/// binding.
pub(crate) unsafe extern "C" fn flight_rec_binding(
    _keyboard: *mut weston_sys::weston_keyboard,
    _time: *const weston_sys::timespec,
    _key: u32,
    data: *mut std::ffi::c_void,
) {
    crate::panic_barrier::guard("binding.flight_rec", || {
        if data.is_null() {
            return;
        }
        let sub = data.cast::<weston_sys::weston_log_subscriber>();
        let dump = || {
            // SAFETY: `data` is the flight-recorder subscriber the
            // frontend registered; it outlives the compositor, so it is
            // live for any binding that can still fire.
            unsafe { weston_sys::weston_log_subscriber_display_flight_rec(sub) };
        };
        // The dump goes out through weston_log, which re-enters our
        // handlers -- so it is an outbound call like any other and takes
        // the A4 wrap when there is a context to wrap it in.
        match crate::ctx::Ctx::current() {
            Some(ctx) => ctx.with_depth(dump),
            None => dump(),
        }
    });
}
