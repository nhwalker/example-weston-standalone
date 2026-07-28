//! Xwayland frontend glue (R2d): the whole of C `frontend/xwayland.c`.
//!
//! The xwayland.so module owns the X sockets and calls back
//! ([`spawn_xserver`]) when the first X client connects; the frontend's
//! job is to fork the X server with the right fds (through
//! `westonite-spawn`, the audited fork/exec surface), watch the
//! `-displayfd` readiness pipe, and relay process exit to the module so
//! it can respawn lazily.
//!
//! Ownership map (§3 memory model):
//! - `abstract_fd`/`unix_fd` arrive **borrowed** from the module (C
//!   passes their numbers to the child and never closes them); we dup
//!   each into an `OwnedFd` for the child and leave the originals
//!   untouched.
//! - the wayland socketpair's parent end transfers to libwayland at
//!   `wl_client_create` (freed with the client);
//! - `wm_fd`'s parent end transfers to the module at `xserver_loaded`
//!   (the WM closes it);
//! - the displayfd pipe's read end stays ours (closed when the watch
//!   ends), the write end goes to the child.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, OsStr};
use std::os::fd::{AsRawFd, BorrowedFd, IntoRawFd, OwnedFd};
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::compositor::CompositorError;
use crate::ctx::Ctx;
use crate::log;

/// libwayland's WL_EVENT_READABLE (an anonymous enum in the header, so
/// bindgen gives it a positional module name).
const WL_EVENT_READABLE: u32 = weston_sys::_bindgen_ty_2::WL_EVENT_READABLE;
/// `_bindgen_ty_2` is *positional*: a header change that introduces an
/// anonymous enum ahead of libwayland's event mask would silently point
/// the constant above at an unrelated enum.  WL_EVENT_READABLE is 0x01
/// by the wayland-server ABI — pin it (risk R-C, same family as the
/// bindings version tripwire).  Spelled as a const `if`/`panic!` rather
/// than `assert!`, which clippy reads as an always-true assertion.
const _: () = {
    if WL_EVENT_READABLE != 0x01 {
        panic!("_bindgen_ty_2 is no longer libwayland's event mask — re-check the regen output");
    }
};

/// Frontend-side Xwayland state (C `struct wet_xwayland`), reached from
/// the spawn/displayfd/SIGCHLD callbacks via `ctx.inner.xwayland`.
/// Fields are individually interior-mutable; no borrow is held across
/// an FFI call (the ctx aliasing rule).
pub(crate) struct XwaylandState {
    pub(crate) api: NonNull<weston_sys::weston_xwayland_api>,
    pub(crate) xwayland: NonNull<weston_sys::weston_xwayland>,
    /// `[xwayland] path` (C XSERVER_PATH default, applied in resolve).
    xserver_path: PathBuf,
    /// The running X server, if any (C `wxw->process`): pid for the
    /// SIGCHLD match, path logged on exit.
    pid: Cell<Option<i32>>,
    /// Parent end of the WM socketpair, held between spawn and
    /// `xserver_loaded` (C `wxw->wm_fd`).
    wm_fd: RefCell<Option<OwnedFd>>,
    /// The `-displayfd` readiness watch (C `wxw->display_fd_source`)
    /// and the pipe read end it polls.
    display_fd_source: Cell<*mut weston_sys::wl_event_source>,
    display_pipe: RefCell<Option<OwnedFd>>,
}

fn state(ctx: &Ctx) -> Option<Rc<XwaylandState>> {
    ctx.inner.xwayland.borrow().clone()
}

/// Remove the `-displayfd` readiness watch and close the pipe read end
/// — C `handle_display_fd`'s `out:`, the same
/// `wl_event_source_remove` + `close` pair.  Idempotent, and the only
/// place either slot is cleared, which is what makes the removal paths
/// (readiness, respawn, teardown) exclusive of one another.
fn close_display_watch(st: &XwaylandState) {
    let source = st.display_fd_source.replace(std::ptr::null_mut());
    if !source.is_null() {
        // SAFETY: the source was created on this display's event loop
        // in spawn_xserver and is removed exactly once — the Cell
        // hand-off above makes the concurrent paths exclusive.
        unsafe { weston_sys::wl_event_source_remove(source) };
    }
    drop(st.display_pipe.borrow_mut().take());
}

/// C `wet_load_xwayland`: load the module, resolve the plugin API and
/// context object, and start listening.  Every failure is fatal at
/// startup (C: `goto out`), so each returns an error here.
pub(crate) fn load(ctx: &Ctx, xserver_path: PathBuf) -> Result<(), CompositorError> {
    let compositor = ctx.inner.compositor.get();

    // SAFETY: compositor live; dlopens xwayland.so and runs its init
    // (registers the plugin API), like every backend load.
    let r = ctx.with_depth(|| unsafe { weston_sys::weston_compositor_load_xwayland(compositor) });
    if r < 0 {
        return Err(CompositorError::Xwayland);
    }

    // weston_xwayland_get_api is a static inline (§3k pattern): call the
    // plugin lookup with the installed header's name/size.
    // SAFETY: compositor live; name is the header's API-name constant.
    let api = unsafe {
        weston_sys::weston_plugin_api_get(
            compositor,
            weston_sys::WESTON_XWAYLAND_API_NAME
                .as_ptr()
                .cast::<c_char>(),
            std::mem::size_of::<weston_sys::weston_xwayland_api>(),
        )
    }
    .cast::<weston_sys::weston_xwayland_api>()
    .cast_mut();
    let Some(api) = NonNull::new(api) else {
        log::log_line("Failed to get the xwayland module API.");
        return Err(CompositorError::Xwayland);
    };

    // SAFETY: api non-null and sized-checked by the plugin registry;
    // `get` cannot fail while the API object is valid (header contract).
    let xwayland =
        ctx.with_depth(|| unsafe { ((*api.as_ptr()).get.expect("xwayland api.get"))(compositor) });
    let Some(xwayland) = NonNull::new(xwayland) else {
        log::log_line("Failed to get the xwayland object.");
        return Err(CompositorError::Xwayland);
    };

    *ctx.inner.xwayland.borrow_mut() = Some(Rc::new(XwaylandState {
        api,
        xwayland,
        xserver_path,
        pid: Cell::new(None),
        wm_fd: RefCell::new(None),
        display_fd_source: Cell::new(std::ptr::null_mut()),
        display_pipe: RefCell::new(None),
    }));

    // SAFETY: module loaded and object resolved just above; listen
    // creates the X sockets and registers their event sources.  The
    // spawn callback resolves state via Ctx::current, so user_data
    // stays null (D21).
    let r = ctx.with_depth(|| unsafe {
        ((*api.as_ptr()).listen.expect("xwayland api.listen"))(
            xwayland.as_ptr(),
            std::ptr::null_mut(),
            Some(spawn_xserver),
        )
    });
    if r < 0 {
        *ctx.inner.xwayland.borrow_mut() = None;
        return Err(CompositorError::Xwayland);
    }
    Ok(())
}

/// C `spawn_xserver`: invoked by xwayland.so from event-loop dispatch
/// when the first X client connects (and again after every
/// `xserver_exited`).  Sync tier — the module consumes the returned
/// `wl_client` immediately (A3 proof: touches only wrapper xwayland
/// state, fd plumbing and libwayland calls; no app borrow).
extern "C" fn spawn_xserver(
    _user_data: *mut c_void,
    display: *const c_char,
    abstract_fd: c_int,
    unix_fd: c_int,
) -> *mut weston_sys::wl_client {
    crate::panic_barrier::guard("xwayland.spawn_xserver", || {
        let Some(ctx) = Ctx::current() else {
            return std::ptr::null_mut();
        };
        ctx.with_depth(|| spawn_xserver_body(&ctx, display, abstract_fd, unix_fd))
    })
}

fn spawn_xserver_body(
    ctx: &Ctx,
    display: *const c_char,
    abstract_fd: c_int,
    unix_fd: c_int,
) -> *mut weston_sys::wl_client {
    let null = std::ptr::null_mut();
    let Some(st) = state(ctx) else { return null };
    // SAFETY: the module passes its NUL-terminated display name (":0");
    // copied at the fence (§3h).  Copied as bytes, not through
    // to_string_lossy: argv is bytes on the C side, and a replacement
    // character would hand the child a display name that is not the one
    // the module is listening on.
    let display = OsStr::from_bytes(unsafe { CStr::from_ptr(display) }.to_bytes()).to_os_string();

    let Ok((wl_ours, wl_child)) = westonite_spawn::socketpair_stream() else {
        log::log_line("wl connection socketpair failed");
        return null;
    };
    let Ok((wm_ours, wm_child)) = westonite_spawn::socketpair_stream() else {
        log::log_line("X wm connection socketpair failed");
        return null;
    };
    let Ok((pipe_read, pipe_write)) = westonite_spawn::pipe_cloexec() else {
        log::log_line("pipe creation for displayfd failed");
        return null;
    };

    // The listen sockets are the module's; C clears CLOEXEC on the very
    // same numbers in the child.  A dup per fd is the owned equivalent:
    // the child sees the dup's number in argv, the originals stay with
    // the module.
    // SAFETY: both fds are live for the duration of this callback (the
    // module owns them and is mid-call); borrow_raw only wraps them for
    // the dup.
    let dup2owned = |fd: c_int| unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned();
    let (Ok(abstract_dup), Ok(unix_dup)) = (dup2owned(abstract_fd), dup2owned(unix_fd)) else {
        log::log_line("duplicating X listen sockets failed");
        return null;
    };

    let path = st.xserver_path.clone();
    // C wet_client_launch logs before forking.
    log::log_line(&format!("launching '{}'", path.display()));
    let cmd = westonite_spawn::Command::new(path.as_os_str())
        .arg(&display)
        .arg("-rootless")
        .arg(weston_sys::XWAYLAND_LISTEN_ARG)
        .arg_fd(abstract_dup)
        .arg(weston_sys::XWAYLAND_LISTEN_ARG)
        .arg_fd(unix_dup)
        .arg("-displayfd")
        .arg_fd(pipe_write)
        .arg("-wm")
        .arg_fd(wm_child)
        .arg("-terminate")
        .pass_fd("WAYLAND_SOCKET", wl_child);
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            // C splits this in two: a fork failure logs in
            // wet_client_launch, an exec failure writes "Error:
            // Couldn't launch client '<path>'" from the child to
            // stderr.  std reports both through spawn()'s Err (the
            // child relays its pre-exec/exec errno over the CLOEXEC
            // status pipe), so one line covers both — worded not to
            // blame fork for what is usually a bad [xwayland] path.
            log::log_line(&format!(
                "weston_client_launch: couldn't launch '{}': {e}",
                path.display()
            ));
            log::log_line("Couldn't start Xwayland");
            return null;
        }
    };
    // The SIGCHLD handler owns reaping (C sigchld_handler's global
    // waitpid loop); only the pid is kept.
    let pid = child.id() as i32;
    drop(child);

    // SAFETY: display live; on success libwayland owns the fd (forget
    // below), on failure ownership stays with wl_ours (drop closes) —
    // wl_client_create does not close the fd when it fails.
    let client =
        unsafe { weston_sys::wl_client_create(ctx.inner.display.get(), wl_ours.as_raw_fd()) };
    if client.is_null() {
        log::log_line("Couldn't create client for Xwayland");
        // C err_proc unlinks the wet_process: the exited server goes
        // down the sigchld "unknown child" path and xserver_exited is
        // never called for it.  (Nothing kills the child there either;
        // its sockets close as our ends drop and -terminate reaps it.)
        return null;
    }
    std::mem::forget(wl_ours);
    st.pid.set(Some(pid));
    // A previous server's wm_fd could still be parked here if it died
    // before readiness; the replace drops (closes) it.  (C overwrites
    // the number and leaks the old fd.)
    st.wm_fd.replace(Some(wm_ours));

    // That same server can also leave its readiness watch installed:
    // its pipe hits EOF when it dies, but the module can relay
    // xserver_exited and be handed a fresh X connection in the very
    // same wl_event_loop_dispatch batch, before that EOF event is
    // dispatched.  Remove the old watch instead of just overwriting the
    // slots below: an orphaned source outliving the pipe it watches is
    // owned by nobody, and its next run would tear down THIS server's
    // watch — leaving the WM unstarted.  (C overwrites the pointer and
    // keeps the stale source; divergence (b) in PROVENANCE, same family
    // as the wm_fd one above.)
    close_display_watch(&st);

    // "During initialization the X server will round trip and block on
    // the wayland compositor, so avoid making blocking requests until
    // it's done with that" — hence the pipe watch instead of a blocking
    // read (C comment, kept verbatim in spirit).
    // SAFETY: display live; the source is removed by
    // close_display_watch (readiness, respawn or teardown), always
    // before the display dies.
    let ev_loop = unsafe { weston_sys::wl_display_get_event_loop(ctx.inner.display.get()) };
    // SAFETY: loop live (owned by the display fetched just above); the
    // callback resolves state via Ctx::current, so data stays null.
    let source = unsafe {
        weston_sys::wl_event_loop_add_fd(
            ev_loop,
            pipe_read.as_raw_fd(),
            WL_EVENT_READABLE,
            Some(handle_display_fd),
            std::ptr::null_mut(),
        )
    };
    // C stores a NULL source without checking (the handler then simply
    // never fires and the WM never starts); log the anomaly instead of
    // matching that silence — the client is already live either way.
    if source.is_null() {
        log::log_line("westonite: failed to watch the Xwayland displayfd");
    }
    st.display_fd_source.set(source);
    st.display_pipe.replace(Some(pipe_read));

    client
}

/// C `handle_display_fd`: Xwayland writes its display number (ending in
/// `\n`) to the `-displayfd` pipe when ready; that is the cue to hand
/// the WM socket to the module.  Sync tier (A3: wrapper state + module
/// call only).
extern "C" fn handle_display_fd(fd: c_int, mask: u32, _data: *mut c_void) -> c_int {
    crate::panic_barrier::guard("xwayland.handle_display_fd", || {
        let Some(ctx) = Ctx::current() else { return 0 };
        ctx.with_depth(|| display_fd_body(&ctx, fd, mask))
    })
}

fn display_fd_body(ctx: &Ctx, fd: c_int, mask: u32) -> c_int {
    let Some(st) = state(ctx) else { return 0 };

    // "xwayland exited before being ready, don't finish initialization,
    // the process watcher will cleanup" (C comment) — fall through to
    // the watch teardown without xserver_loaded.
    let mut loaded = false;
    if mask & WL_EVENT_READABLE != 0 {
        let mut buf = [0u8; 64];
        // SAFETY: fd is the displayfd pipe read end the loop is
        // watching (our copy parked in st.display_pipe), live for the
        // whole callback; plain read into a stack buffer.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        let err = std::io::Error::last_os_error();
        if n < 0 && err.raw_os_error() != Some(libc::EAGAIN) {
            log::log_line(&format!("read from Xwayland display_fd failed: {err}"));
        } else if n < 0 || (n > 0 && buf[(n - 1) as usize] != b'\n') {
            // "Xwayland writes to the pipe twice, so if we close it too
            // early it's possible the second write will fail" — keep
            // reading until the end-of-line marker (C returns 1:
            // recheck and call again).
            return 1;
        } else if n == 0 {
            // Deliberate divergence: C also returns 1 on EOF, re-arming
            // a level-triggered readable-forever pipe until the process
            // watcher runs.  A closed write end can never complete the
            // line, so tear the watch down now instead of spinning.
        } else {
            loaded = true;
        }
    }

    if loaded {
        let wm = st.wm_fd.borrow_mut().take();
        if let Some(wm) = wm {
            // SAFETY: api/xwayland live while the module is loaded (the
            // compositor is alive inside the run loop); the module
            // takes ownership of wm_fd (the WM closes it).  No borrows
            // held across the call.
            unsafe {
                ((*st.api.as_ptr()).xserver_loaded.expect("xserver_loaded"))(
                    st.xwayland.as_ptr(),
                    wm.into_raw_fd(),
                );
            }
        }
    }

    // C's `out:` — every non-retry path removes the source and closes
    // the pipe.
    close_display_watch(&st);
    0
}

/// SIGCHLD hook (C `sigchld_handler`'s wet_process branch +
/// `xserver_cleanup`): returns true when `pid` was the Xwayland server,
/// after logging its exit and notifying the module — which keeps
/// listening and respawns on the next X connection.
pub(crate) fn handle_child_exit(ctx: &Ctx, pid: i32, status: c_int) -> bool {
    let Some(st) = state(ctx) else { return false };
    if st.pid.get() != Some(pid) {
        return false;
    }
    st.pid.set(None);

    log::log_child_exit(&st.xserver_path.display().to_string(), status);

    // At +1 dispatch depth per the A4 rule (every outbound FFI call):
    // this one tears down the module's WM and destroys the Xwayland
    // wl_client, whose destroy signals re-enter our trampolines.
    // Today this wrap has a backstop rather than a live failure mode —
    // on_sigchld only ever fires inside wl_display_run, which run()
    // itself holds at +1, so nested trampolines cannot return the
    // counter to zero here even without it (verified empirically: no
    // drain fires anywhere inside the run loop; see the plan §6 note
    // on deferred-drain timing).  The wrap makes this call site
    // A4-correct on its own instead of leaning on that run() wrap.
    // SAFETY: module loaded for the compositor's lifetime; no borrows
    // held (state reached through the Rc clone).
    ctx.with_depth(|| unsafe {
        ((*st.api.as_ptr()).xserver_exited.expect("xserver_exited"))(st.xwayland.as_ptr());
    });
    true
}

/// C `wet_xwayland_destroy`, called from Compositor::drop BEFORE
/// weston_compositor_destroy (main.c teardown order — the module must
/// still be live for xserver_exited).
pub(crate) fn teardown(ctx: &Ctx) {
    let Some(st) = ctx.inner.xwayland.borrow_mut().take() else {
        return;
    };
    // A pending readiness watch dies with us (C leaves the source to
    // the display teardown; we own the pipe fd, so do it explicitly).
    close_display_watch(&st);
    drop(st.wm_fd.borrow_mut().take());

    // C: wet_process_destroy(process, 0, true) → xserver_cleanup →
    // xserver_exited, so the module tears down the WM and client state
    // while the compositor is still alive.  The server process itself
    // is never killed (C comment in wet_client_start: "We have no way
    // of killing the process"); -terminate ends it once its sockets
    // drop with the display.
    if st.pid.take().is_some() {
        ctx.with_depth(|| {
            // SAFETY: compositor and module still live (we run before
            // weston_compositor_destroy); no borrows held.
            unsafe {
                ((*st.api.as_ptr()).xserver_exited.expect("xserver_exited"))(st.xwayland.as_ptr());
            }
        });
    }
}
