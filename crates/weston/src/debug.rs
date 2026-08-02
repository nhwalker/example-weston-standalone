//! `--debug` and the protocol dump (C main.c:246-360, 4396, 4618-4631).
//!
//! Two separate things that C wires up next to each other:
//!
//! * The **"proto" log scope** and its `wl_protocol_logger`, installed
//!   *unconditionally*.  Every request and event is formatted into that
//!   scope — which costs nothing until somebody subscribes, because the
//!   handler returns immediately when the scope has no subscriber.
//!   `--logger-scopes=proto` is how you turn it on (see [`crate::log`]).
//! * **`--debug` itself**, which enables libweston's weston-debug
//!   protocol and installs a screenshot authority that says yes to
//!   everyone.  That second half is a real privilege change, which is
//!   why C logs nothing about it and neither do we: it is opt-in by
//!   flag and the flag is the record.
//!
//! Formatting is Rust-side and passed to C as a length-delimited byte
//! string, so a client-controlled `wl_resource` class or string
//! argument can never reach a format string (§3i).  C builds the same
//! line with `open_memstream` + `fprintf`.

use std::ffi::{CStr, c_void};

use crate::ctx::Ctx;
use crate::listener::Listener;
use crate::panic_barrier;

/// C `get_next_argument` (main.c:246): walk a `wl_message` signature to
/// the next real argument type, skipping the version and nullable
/// markers (`2`, `?`) that can precede it.
fn next_argument(signature: &[u8], from: usize) -> (Option<u8>, usize) {
    let mut i = from;
    while i < signature.len() {
        let c = signature[i];
        if matches!(c, b'i' | b'u' | b'f' | b's' | b'o' | b'n' | b'a' | b'h') {
            return (Some(c), i + 1);
        }
        i += 1;
    }
    (None, i)
}

/// `wl_fixed_to_double`, which is a static inline and so unbindable:
/// wl_fixed_t is 24.8 fixed point.
fn fixed_to_double(f: weston_sys::wl_fixed_t) -> f64 {
    f64::from(f) / 256.0
}

/// Borrow a C string as a `String`, or a placeholder when null.
///
/// # Safety
/// `p` must be null or a valid NUL-terminated string for this call.
unsafe fn cstr(p: *const std::os::raw::c_char, when_null: &str) -> String {
    if p.is_null() {
        return when_null.to_string();
    }
    // SAFETY: caller guarantees a live NUL-terminated string.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// C `protocol_log_fn` (main.c:267).  One line per message, into the
/// "proto" scope.
pub(crate) unsafe extern "C" fn protocol_log_fn(
    user_data: *mut c_void,
    direction: weston_sys::wl_protocol_logger_type::Type,
    message: *const weston_sys::wl_protocol_logger_message,
) {
    panic_barrier::guard("protocol_log_fn", || {
        let scope = user_data.cast::<weston_sys::weston_log_scope>();
        if scope.is_null() || message.is_null() {
            return;
        }
        // C's first act: no subscriber, no work.  This is what makes an
        // always-installed protocol logger free.
        // SAFETY: scope is the "proto" scope, owned by the Compositor
        // and destroyed only after this logger is.
        if !unsafe { weston_sys::weston_log_scope_is_enabled(scope) } {
            return;
        }
        // SAFETY: `message` is libwayland's per-call record, valid for
        // the duration of this callback.
        let m = unsafe { &*message };
        // SAFETY: `m` is the live record checked above.
        let Some(line) = (unsafe { format_message(direction, m) }) else {
            return;
        };

        // C prefixes with weston_log_scope_timestamp into a 128-byte
        // buffer; same size, same call.
        let mut timestr = [0u8; 128];
        // SAFETY: scope live; the buffer is ours and correctly sized.
        let ts = unsafe {
            weston_sys::weston_log_scope_timestamp(
                scope,
                timestr.as_mut_ptr().cast(),
                timestr.len(),
            )
        };
        // SAFETY: weston_log_scope_timestamp returns the buffer we
        // passed, NUL-terminated, valid until `timestr` goes out of
        // scope below.
        let stamp = unsafe { cstr(ts, "") };

        let out = format!("{stamp} {line}\n");
        // SAFETY: scope live; write takes a pointer + length and copies,
        // so `out` need only outlive the call.  Length-delimited, so the
        // client-controlled text inside is never a format string.
        unsafe {
            weston_sys::weston_log_scope_write(scope, out.as_ptr().cast(), out.len());
        }
    });
}

/// The body of one protocol line, without the timestamp.
///
/// # Safety
/// `m` must be the live message record libwayland passed in.
unsafe fn format_message(
    direction: weston_sys::wl_protocol_logger_type::Type,
    m: &weston_sys::wl_protocol_logger_message,
) -> Option<String> {
    if m.resource.is_null() || m.message.is_null() {
        return None;
    }
    // SAFETY: resource and message live for this callback.
    let (client, class, id, name, signature, types) = unsafe {
        let client = weston_sys::wl_resource_get_client(m.resource);
        let msg = &*m.message;
        (
            client,
            cstr(weston_sys::wl_resource_get_class(m.resource), "[unknown]"),
            weston_sys::wl_resource_get_id(m.resource),
            cstr(msg.name, "[unknown]"),
            if msg.signature.is_null() {
                Vec::new()
            } else {
                CStr::from_ptr(msg.signature).to_bytes().to_vec()
            },
            msg.types,
        )
    };

    let mut pid: libc::pid_t = 0;
    if !client.is_null() {
        // SAFETY: client live; the two out-params we do not want are
        // null, which wl_client_get_credentials accepts.
        unsafe {
            weston_sys::wl_client_get_credentials(
                client,
                &mut pid,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
    }

    let dir = if direction == weston_sys::wl_protocol_logger_type::WL_PROTOCOL_LOGGER_REQUEST {
        "rq"
    } else {
        "ev"
    };
    // `{client:p}` reproduces C's `%p`: the pointer identifies the
    // client across lines, which is the whole point of printing it.
    let mut out = format!("client {client:p} (PID {pid}) {dir} {class}@{id}.{name}(");

    let count = usize::try_from(m.arguments_count).unwrap_or(0);
    let mut sig_at = 0usize;
    for i in 0..count {
        let (ty, next) = next_argument(&signature, sig_at);
        sig_at = next;
        if i > 0 {
            out.push_str(", ");
        }
        if m.arguments.is_null() {
            break;
        }
        // SAFETY: libwayland guarantees `arguments_count` entries, and
        // the signature says which union member of each is live -- the
        // pairing format_argument relies on, and exactly how C reads it.
        out.push_str(&unsafe { format_argument(ty, m.arguments.add(i), types, i) });
    }
    out.push(')');
    Some(out)
}

/// One argument, formatted as C's per-type `fprintf` arm does.
///
/// # Safety
/// `arg` must point at the live argument whose type is `ty`, and
/// `types` must be the message's `wl_interface *` table (or null).
unsafe fn format_argument(
    ty: Option<u8>,
    arg: *const weston_sys::wl_argument,
    types: *mut *const weston_sys::wl_interface,
    index: usize,
) -> String {
    // SAFETY: the caller pairs `arg` with the signature type, so each
    // union read below is the member libwayland wrote.  Object and
    // string arguments are borrowed for this callback only, and are
    // copied out before returning.
    unsafe {
        let a = *arg;
        match ty {
            Some(b'u') => a.u.to_string(),
            Some(b'i') => a.i.to_string(),
            // C's "%f": six decimals.
            Some(b'f') => format!("{:.6}", fixed_to_double(a.f)),
            Some(b's') => format!("\"{}\"", cstr(a.s, "")),
            Some(b'o') => {
                if a.o.is_null() {
                    "nil".to_string()
                } else {
                    let res = a.o.cast::<weston_sys::wl_resource>();
                    format!(
                        "{}@{}",
                        cstr(weston_sys::wl_resource_get_class(res), "[unknown]"),
                        weston_sys::wl_resource_get_id(res)
                    )
                }
            }
            Some(b'n') => {
                // C indexes types[] by ARGUMENT index rather than by
                // position in the signature; preserved rather than
                // "fixed", since diverging would make our dumps
                // disagree with weston's for no gain.
                let iface = if types.is_null() {
                    "[unknown]".to_string()
                } else {
                    let t = *types.add(index);
                    if t.is_null() {
                        "[unknown]".to_string()
                    } else {
                        cstr((*t).name, "[unknown]")
                    }
                };
                if a.n != 0 {
                    format!("new id {iface}@{}", a.n)
                } else {
                    format!("new id {iface}@nil")
                }
            }
            // C prints the word, not the contents.
            Some(b'a') => "array".to_string(),
            // Same as C: after libwayland's demarshalling this is the
            // RECEIVING (compositor-side) fd number, not the number
            // the client passed (PR36-C2) — the dump line just reads
            // like it is the client's.
            Some(b'h') => format!("fd {}", a.h),
            _ => String::new(),
        }
    }
}

/// C main.c:4626 — the effect of `--debug`: enable libweston's
/// weston-debug protocol, and install a screenshot authority that says
/// yes to everyone.
///
/// The returned [`Listener`] must be kept by the caller and detached
/// before the compositor dies, like every frontend listener.
///
/// A `Listener` rather than a bare `extern "C"` fn because that is how
/// the tree attaches to `output_capture.ask_auth` already
/// (screenshooter.rs): the C helper
/// `weston_compositor_add_screenshot_authority` just writes
/// `listener->notify` and `wl_signal_add`s, which would overwrite the
/// primitive's trampoline.
pub(crate) fn enable(ctx: &Ctx) -> Listener {
    let compositor = ctx.inner.compositor.get();
    // SAFETY: compositor live; this only flips libweston's own gate on
    // the weston_debug_v1 global.
    unsafe { weston_sys::weston_compositor_enable_debug_protocol(compositor) };

    let auth = Listener::new(
        "debug.allow_all_screenshots",
        false,
        Box::new(|_ctx, data| {
            let att = data.cast::<weston_sys::weston_output_capture_attempt>();
            if att.is_null() {
                return;
            }
            // C screenshot_allow_all: indiscriminately allow everyone to
            // capture any output.  Deliberately unconditional -- it is
            // what the flag means.
            // SAFETY: `att` is the capture-attempt record libweston is
            // deciding on, live for this emission; `authorized` is the
            // field an authority exists to write.
            unsafe { (*att).authorized = true };
        }),
    );
    // SAFETY: compositor live; ask_auth is the signal the C helper adds
    // to (output-capture.c:801).
    unsafe {
        weston_sys::wsys_wl_signal_add(
            &raw mut (*compositor).output_capture.ask_auth,
            auth.raw_ptr(),
        );
    }
    // raw_ptr() contract (listener.rs): a hand-attached node MUST be
    // marked, or detach-on-drop skips it — the Compositor's teardown
    // would then free this pinned box while its node is still linked
    // on ask_auth, inside the still-live compositor struct.
    auth.mark_attached();
    auth
}
