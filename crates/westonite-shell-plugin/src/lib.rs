//! The hybrid-phase `desktop-shell.so` (plan §2, D2 — R1 only, deleted
//! at R3).  The C frontend dlopens this and calls `wet_shell_init`;
//! everything else happens behind `weston::shell_init`.

use std::ffi::{c_char, c_int, c_void};

use westonite_shell::{Shell, ShellConfig};

/// The module entry point the westonite frontend loads
/// (frontend/weston.h contract).  Opaque pointers only — this crate
/// never names a weston-sys type.
///
/// # Safety
/// Called by the C frontend exactly once per compositor with a live
/// `weston_compositor *`; argc/argv are unused (the R1 shell takes no
/// module arguments, like the trimmed C shell).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wet_shell_init(
    ec: *mut c_void,
    _argc: *mut c_int,
    _argv: *mut *mut c_char,
) -> c_int {
    let ok = weston::shell_init::shell_init(ec, |background_color| {
        Box::new(Shell::new(ShellConfig { background_color }))
    });
    if ok { 0 } else { -1 }
}
