//! Screenshooter/recorder frontend glue (R2e): the whole of C
//! `frontend/weston-screenshooter.c`.
//!
//! Two Super key bindings: Super+S forks the `weston-screenshooter`
//! client (which drives the weston-output-capture protocol; the
//! authority listener below authorizes exactly that client and nobody
//! else), Super+R toggles the wcap recorder on the focused output.
//!
//! Ownership note (plan §4): in C the *shell* calls
//! `screenshooter_create` (shell.c:2232); in the Rust frontend the
//! frontend owns it — `create` runs from build() right after the shell
//! attaches.  Same registrations at the same point in startup; recorded
//! as a structural, not behavioral, change.

use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::os::fd::AsRawFd;
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ctx::Ctx;
use crate::listener::Listener;
use crate::log;

// linux/input-event-codes.h, as C includes linux/input.h.
const KEY_R: u32 = 19;
const KEY_S: u32 = 31;

/// C `struct screenshooter`, reached from the binding/authority/SIGCHLD
/// callbacks via `ctx.inner.screenshooter`.  Interior-mutable per
/// field; no borrow held across FFI (the ctx aliasing rule).
pub(crate) struct ScreenshooterState {
    /// The in-flight screenshot client (C `shooter->client`); null
    /// means none, and gates re-entry ("Don't start a screenshot
    /// whilst we already have one in progress").
    client: Cell<*mut weston_sys::wl_client>,
    /// Destroy listener for that client (C's embedded
    /// `client_destroy_listener`): ONE node, reattached per spawn
    /// exactly as C reuses the listener embedded in `struct
    /// screenshooter`.  Oneshot, so the firing takes the node off the
    /// dying client's list and leaves it ready for the next Super+S.
    /// Deliberately not a box-per-spawn retired through `defer_drop`.
    /// That mattered acutely when this was written — the pending-drop
    /// list then only drained after the event loop exited, so a box
    /// per press was a per-press leak — and the drain gap has since
    /// been fixed (§3e/A4).  The reuse stands on its own regardless:
    /// it is what C does, and one node per subsystem beats one per
    /// user action even when the list does drain.
    client_destroy: Listener,
    /// C `shooter->recorder`: the active wcap recorder, if any.
    recorder: Cell<*mut weston_sys::weston_recorder>,
    /// pid + argv[0] of the spawned client for the SIGCHLD exit lines
    /// (C tracks it as a `wet_process`).
    pid: Cell<Option<i32>>,
    exe: RefCell<Option<PathBuf>>,
}

fn state(ctx: &Ctx) -> Option<Rc<ScreenshooterState>> {
    ctx.inner.screenshooter.borrow().clone()
}

/// C `screenshooter_create`: register the two key bindings and the
/// capture authority.  Returns the authority listener for the
/// Compositor's frontend-listener set — its node sits on a signal list
/// inside the weston_compositor struct (`output_capture.ask_auth`), so
/// it must be detached in Drop before `weston_compositor_destroy`
/// (C removes it in `screenshooter_destroy`, from the compositor's own
/// destroy signal).
pub(crate) fn create(ctx: &Ctx) -> Listener {
    let compositor = ctx.inner.compositor.get();

    // C `screenshooter_client_destroy`: free the slot so a new Super+S
    // can spawn.  The oneshot trampoline has already taken the node off
    // the dying client's list by the time this runs, leaving the single
    // embedded listener reattachable (see the field comment).
    let client_destroy = Listener::new(
        "screenshooter.client_destroy",
        true,
        Box::new(move |ctx, _data| {
            if let Some(st) = state(ctx) {
                st.client.set(std::ptr::null_mut());
            }
        }),
    );

    *ctx.inner.screenshooter.borrow_mut() = Some(Rc::new(ScreenshooterState {
        client: Cell::new(std::ptr::null_mut()),
        client_destroy,
        recorder: Cell::new(std::ptr::null_mut()),
        pid: Cell::new(None),
        exe: RefCell::new(None),
    }));

    // SAFETY: compositor live; registrations mirror screenshooter_create
    // (weston-screenshooter.c:142-145).  The bindings live until the
    // compositor destroys them, like every binding in the tree; data
    // stays null (state resolves via Ctx::current, D21).
    unsafe {
        weston_sys::weston_compositor_add_key_binding(
            compositor,
            KEY_S,
            weston_sys::weston_keyboard_modifier::MODIFIER_SUPER,
            Some(screenshooter_binding),
            std::ptr::null_mut(),
        );
        weston_sys::weston_compositor_add_key_binding(
            compositor,
            KEY_R,
            weston_sys::weston_keyboard_modifier::MODIFIER_SUPER,
            Some(recorder_binding),
            std::ptr::null_mut(),
        );
    }

    // The authority listener.  C's helper
    // (weston_compositor_add_screenshot_authority) just writes
    // `listener->notify = auth` and wl_signal_add's onto
    // output_capture.ask_auth — it would overwrite our trampoline, so
    // attach the same way it does, keeping the Listener primitive.
    let auth = Listener::new(
        "screenshooter.authorize",
        false,
        Box::new(move |ctx, data| {
            let Some(st) = state(ctx) else { return };
            let att = data.cast::<weston_sys::weston_output_capture_attempt>();
            if att.is_null() {
                return;
            }
            // C authorize_screenshooter: only the client we spawned.
            // SAFETY: att and att.who are live for the emission (the
            // capture code fills them right before asking).
            unsafe {
                let who = (*att).who;
                if !st.client.get().is_null() && !who.is_null() && (*who).client == st.client.get()
                {
                    (*att).authorized = true;
                }
            }
        }),
    );
    // SAFETY: compositor live; ask_auth is the very signal the C helper
    // adds to (output-capture.c:801).
    unsafe {
        weston_sys::wsys_wl_signal_add(
            &raw mut (*compositor).output_capture.ask_auth,
            auth.raw_ptr(),
        );
    }
    auth.mark_attached();
    auth
}

/// C `screenshooter_binding` + the `wet_client_start` it calls: spawn
/// `weston-screenshooter` with a wayland socketpair.  Sync tier (A3
/// proof: wrapper screenshooter state, fd plumbing, spawn and
/// `wl_client_create` only — no app borrow).
extern "C" fn screenshooter_binding(
    _keyboard: *mut weston_sys::weston_keyboard,
    _time: *const weston_sys::timespec,
    _key: u32,
    _data: *mut c_void,
) {
    crate::panic_barrier::guard("screenshooter.binding", || {
        let Some(ctx) = Ctx::current() else { return };
        ctx.with_depth(|| screenshooter_binding_body(&ctx));
    });
}

fn screenshooter_binding_body(ctx: &Ctx) {
    let Some(st) = state(ctx) else { return };
    // "Don't start a screenshot whilst we already have one in progress"
    if !st.client.get().is_null() {
        return;
    }

    let Some(exe) = bindir_path("weston-screenshooter") else {
        log::log_line("Could not construct screenshooter path.");
        return;
    };

    let Ok((ours, child_end)) = westonite_spawn::socketpair_stream() else {
        // C wet_client_start's wording for the same failure.
        log::log_line(&format!(
            "wet_client_start: socketpair failed while launching '{}'",
            exe.display()
        ));
        return;
    };

    // C wet_client_launch logs before forking.
    log::log_line(&format!("launching '{}'", exe.display()));
    let child = westonite_spawn::Command::new(exe.as_os_str())
        .pass_fd("WAYLAND_SOCKET", child_end)
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            // C splits this across fork (parent log) and exec (child
            // stderr, "Error: Couldn't launch client '<path>'"); std
            // reports both through Err.
            log::log_line(&format!(
                "Error: Couldn't launch client '{}': {e}",
                exe.display()
            ));
            return;
        }
    };
    let pid = child.id() as i32;
    drop(child); // reaped by the SIGCHLD handler, like every child

    // C's wet_client_launch puts the wet_process on the child list
    // before wet_client_start reaches wl_client_create, so a child
    // whose wl_client never came up still gets its exit logged.
    st.pid.set(Some(pid));
    *st.exe.borrow_mut() = Some(exe.clone());

    // SAFETY: display live; on success libwayland owns the fd (forget
    // below), on failure ownership stays with `ours` (drop closes).
    let client = unsafe { weston_sys::wl_client_create(ctx.inner.display.get(), ours.as_raw_fd()) };
    if client.is_null() {
        // C: "We have no way of killing the process, so leave it
        // hanging" — it exits on its own when the socket closes.
        log::log_line(&format!(
            "wet_client_start: wl_client_create failed while launching '{}'.",
            exe.display()
        ));
        return;
    }
    std::mem::forget(ours);

    st.client.set(client);

    // C reattaches its embedded client_destroy_listener to each new
    // client; ours is detached by the previous oneshot firing (or was
    // never attached), so the node is free to go back on a list here.
    // SAFETY: client just created and live; the node comes off again in
    // the oneshot firing or in teardown, both before the client's list
    // dies.
    unsafe { weston_sys::wl_client_add_destroy_listener(client, st.client_destroy.raw_ptr()) };
    st.client_destroy.mark_attached();
}

/// C `recorder_binding`: Super+R toggles the wcap recorder on the
/// keyboard-focus output (first output as fallback).  Sync tier (A3:
/// wrapper state + libweston recorder calls, no app borrow).
extern "C" fn recorder_binding(
    keyboard: *mut weston_sys::weston_keyboard,
    _time: *const weston_sys::timespec,
    _key: u32,
    _data: *mut c_void,
) {
    crate::panic_barrier::guard("screenshooter.recorder_binding", || {
        let Some(ctx) = Ctx::current() else { return };
        ctx.with_depth(|| recorder_binding_body(&ctx, keyboard));
    });
}

fn recorder_binding_body(ctx: &Ctx, keyboard: *mut weston_sys::weston_keyboard) {
    let Some(st) = state(ctx) else { return };

    let recorder = st.recorder.get();
    if !recorder.is_null() {
        st.recorder.set(std::ptr::null_mut());
        // SAFETY: the pointer came from weston_recorder_start and the
        // recorder is stopped exactly once (the Cell swap above).
        unsafe { weston_sys::weston_recorder_stop(recorder) };
        return;
    }

    let compositor = ctx.inner.compositor.get();
    // C: keyboard->focus->output, else the first output on the
    // compositor list.
    // SAFETY: keyboard is live inside the binding frame; focus/output
    // are read under the same guards C uses.
    let mut output: *mut weston_sys::weston_output = unsafe {
        let kb = &*keyboard;
        if !kb.focus.is_null() && !(*kb.focus).output.is_null() {
            (*kb.focus).output
        } else {
            std::ptr::null_mut()
        }
    };
    if output.is_null() {
        // SAFETY: compositor live; plain intrusive-list read.  C does
        // container_of(output_list.next) with NO empty check — on a
        // --no-outputs compositor that is a wild pointer into the list
        // head.  Deliberate divergence: an empty list is a logged no-op
        // here instead of undefined behavior.
        output = unsafe {
            let head = &raw mut (*compositor).output_list;
            let next = (*head).next;
            if std::ptr::eq(next, head) {
                std::ptr::null_mut()
            } else {
                next.byte_sub(std::mem::offset_of!(weston_sys::weston_output, link))
                    .cast::<weston_sys::weston_output>()
            }
        };
    }
    let Some(output) = NonNull::new(output) else {
        log::log_line("westonite: no output to record");
        return;
    };

    // SAFETY: output live (heads-changed keeps enabled outputs alive
    // through this dispatch); the filename is a static C string the
    // recorder opens in the compositor's cwd, exactly as C.
    let rec =
        unsafe { weston_sys::weston_recorder_start(output.as_ptr(), c"capture.wcap".as_ptr()) };
    st.recorder.set(rec);
}

/// C's BINDIR resolution (`wet_get_bindir_path` → `wet_get_binary_path`):
/// the WESTON_MODULE_MAP override first, then BINDIR/name.  BINDIR is
/// meson's prefix bindir — /usr/bin for every supported install (the
/// RPM and the e2e containers alike).
fn bindir_path(name: &str) -> Option<PathBuf> {
    let cname = std::ffi::CString::new(name).ok()?;
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: plain env-map lookup into a stack buffer; the helper
    // NUL-terminates anything it writes and returns 0 when unmapped.
    let len = unsafe {
        weston_sys::weston_module_path_from_env(
            cname.as_ptr(),
            buf.as_mut_ptr().cast::<c_char>(),
            buf.len(),
        )
    };
    // `len == 0` means unmapped.  Copied as bytes, not through
    // to_string_lossy: this path becomes argv[0] for execve, and a
    // replacement character would spawn something other than the binary
    // the map named (same rule as the Xwayland display name, §3h).
    if len > 0
        && let Some(bytes) = buf.get(..len)
    {
        return Some(PathBuf::from(OsStr::from_bytes(bytes)));
    }
    Some(PathBuf::from("/usr/bin").join(name))
}

/// SIGCHLD hook: C tracks the spawned client as a `wet_process`, so its
/// exit is logged by sigchld_handler.  Returns true when `pid` was the
/// screenshooter client.  Nothing else to relay — the client-destroy
/// listener (not process exit) is what resets the client slot.
pub(crate) fn handle_child_exit(ctx: &Ctx, pid: i32, status: c_int) -> bool {
    let Some(st) = state(ctx) else { return false };
    if st.pid.get() != Some(pid) {
        return false;
    }
    st.pid.set(None);
    // Copy the path out before logging: weston_log is an outbound FFI
    // call and this module holds no borrow across one.
    let exe = st.exe.borrow().as_ref().map(|p| p.display().to_string());
    if let Some(exe) = exe {
        log::log_child_exit(&exe, status);
    }
    true
}

/// Frontend-side teardown, from Compositor::drop before
/// weston_compositor_destroy.  C's screenshooter_destroy removes its
/// listeners from inside the compositor's destroy emission; detaching
/// beforehand is the wrapper's detach-before-free rule.  The authority
/// listener is Compositor-owned and detaches with the other frontend
/// listeners; here only the per-client node and the state drop.  An
/// active recorder is deliberately left as C leaves it (no stop at
/// destroy).
pub(crate) fn teardown(ctx: &Ctx) {
    let Some(st) = ctx.inner.screenshooter.borrow_mut().take() else {
        return;
    };
    // SAFETY-relevant ordering: while attached the node sits on the live
    // client's destroy list; the display (and its clients) dies after
    // this.  A no-op when the client already died (oneshot detached).
    st.client_destroy.detach();
}
