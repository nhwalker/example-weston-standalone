//! Hybrid-R1 shell bootstrap (plan §2, §7 R1): everything
//! `wet_shell_init` did in C, done through the wrapper, with the shell
//! policy behind `ShellApp`/`ShellHost`.
//!
//! R1-specific hazards handled here (plan §7 R1):
//! * the compositor user-data slot belongs to the C frontend — never
//!   touched; the wrapper roots all state in its own `Ctx`.
//! * the borrowed `weston_config *` is read once into the background
//!   color and never retained (R3 deletes this whole path).
//! * `wet_get_config` / `screenshooter_create` live in
//!   `libexec_westonite.so`, already loaded in the process — resolved
//!   via `dlsym(RTLD_DEFAULT)` so the cdylib carries no link-time
//!   dependency (deleted at R3 with the rest of the hybrid surface).

use std::ffi::{c_char, c_int, c_void};
use std::ptr::NonNull;

use crate::ctx::{Ctx, ShellApp};
use crate::events::Event;
use crate::ids::SeatId;
use crate::listener::Listener;

// Installed libweston exports (config-parser.h) — hand-declared here so
// the generated bindings carry no hybrid-only surface (plan §2: the
// config-parser binding exists only through R1).
#[allow(non_camel_case_types)]
type weston_config = c_void;
#[allow(non_camel_case_types)]
type weston_config_section = c_void;
unsafe extern "C" {
    fn weston_config_get_section(
        config: *mut weston_config,
        section: *const c_char,
        key: *const c_char,
        value: *const c_char,
    ) -> *mut weston_config_section;
    fn weston_config_section_get_color(
        section: *mut weston_config_section,
        key: *const c_char,
        color: *mut u32,
        default_color: u32,
    ) -> c_int;
}

/// Resolve a frontend symbol from the already-loaded
/// `libexec_westonite.so`.
fn frontend_sym(name: &std::ffi::CStr) -> *mut c_void {
    // SAFETY: dlsym with RTLD_DEFAULT searches the global namespace;
    // pure lookup.
    unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) }
}

/// Read `[shell] background-color` exactly as the C shell did
/// (shell_configuration, default 0xff002244).
fn read_background_color(ec: *mut weston_sys::weston_compositor) -> u32 {
    let mut color: u32 = 0xff002244;
    let get_config = frontend_sym(c"wet_get_config");
    if get_config.is_null() {
        crate::log::log_line("westonite-shell: wet_get_config not found; using default background");
        return color;
    }
    // SAFETY: wet_get_config has the declared signature (frontend
    // weston.h contract); the returned config is borrowed for this
    // call chain only and not retained (R1 hazard note).
    unsafe {
        let f: unsafe extern "C" fn(*mut weston_sys::weston_compositor) -> *mut weston_config =
            std::mem::transmute(get_config);
        let config = f(ec);
        let section = weston_config_get_section(
            config,
            c"shell".as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
        );
        weston_config_section_get_color(section, c"background-color".as_ptr(), &mut color, color);
    }
    color
}

fn create_screenshooter(ec: *mut weston_sys::weston_compositor) {
    let sym = frontend_sym(c"screenshooter_create");
    if sym.is_null() {
        crate::log::log_line("westonite-shell: screenshooter_create not found; skipping");
        return;
    }
    // SAFETY: declared `void screenshooter_create(struct weston_compositor *)`
    // in frontend/weston.h; called exactly as the C shell does at init.
    unsafe {
        let f: unsafe extern "C" fn(*mut weston_sys::weston_compositor) = std::mem::transmute(sym);
        f(ec);
    }
}

/// Per-seat wrapper listeners (C shell_seat).  The keyboard-focus
/// listener of the C struct is vestigial after the T-series trims
/// (initialized, never attached) and is not ported.
struct SeatRec {
    destroy: Listener,
    caps: Listener,
    pointer_focus: std::rc::Rc<Listener>,
}

thread_local! {
    /// Seat recs are keyed by seat address; wrapper-internal only.
    /// (Thread-local rather than a CtxInner field to keep the hybrid-R1
    /// surface out of the core context; dies with the shell.)
    static SEAT_RECS: std::cell::RefCell<std::collections::HashMap<usize, SeatRec>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn register_seat(ctx: &Ctx, seat: NonNull<weston_sys::weston_seat>, announce: bool) {
    let key = seat.as_ptr() as usize;
    let already = SEAT_RECS.with(|m| m.borrow().contains_key(&key));
    if already {
        return;
    }
    let id = SeatId(ctx.inner.seats.borrow_mut().insert(seat));

    // Pointer-focus handler: wrapper-internal busy/ping policy (§3f).
    let pointer_focus = std::rc::Rc::new(Listener::new(
        "shell.pointer_focus",
        false,
        Box::new(move |ctx, data| {
            if let Some(p) = NonNull::new(data.cast::<weston_sys::weston_pointer>()) {
                crate::grab::on_pointer_focus(ctx, p);
            }
        }),
    ));

    // caps_changed: attach/detach the pointer-focus listener as the
    // pointer capability comes and goes (C shell_seat_caps_changed).
    let pf = std::rc::Rc::clone(&pointer_focus);
    let seat_ptr = seat;
    let caps = Listener::new(
        "shell.seat_caps",
        false,
        Box::new(move |_ctx, _data| {
            // SAFETY: seat live while its caps signal fires; pointer is
            // a scoped access (§3a).
            let pointer = unsafe { weston_sys::weston_seat_get_pointer(seat_ptr.as_ptr()) };
            if let Some(pointer) = NonNull::new(pointer) {
                if !pf.is_attached() {
                    // SAFETY: pointer->focus_signal outlives the
                    // attachment: the caps handler detaches when the
                    // pointer capability drops, and seat destroy
                    // detaches the rec.
                    unsafe {
                        weston_sys::wsys_wl_signal_add(
                            &raw mut (*pointer.as_ptr()).focus_signal,
                            pf.raw_ptr(),
                        );
                    }
                    pf.mark_attached();
                }
            } else if pf.is_attached() {
                pf.detach();
            }
        }),
    );
    // SAFETY: seat live; updated_caps_signal is a live signal on it.
    unsafe {
        weston_sys::wsys_wl_signal_add(
            &raw mut (*seat.as_ptr()).updated_caps_signal,
            caps.raw_ptr(),
        );
    }
    caps.mark_attached();

    // Seat destroy: sync invalidation + explicit detach of the other
    // listeners (their signals die with the seat — deferring the
    // detach would touch freed memory), then deferred policy event.
    let destroy = Listener::new(
        "shell.seat_destroy",
        true,
        Box::new(move |ctx, data| {
            let Some(p) = NonNull::new(data.cast::<weston_sys::weston_seat>()) else {
                return;
            };
            ctx.inner.seats.borrow_mut().invalidate_ptr(p);
            let key = p.as_ptr() as usize;
            if let Some(rec) = SEAT_RECS.with(|m| m.borrow_mut().remove(&key)) {
                rec.caps.detach();
                rec.pointer_focus.detach();
                ctx.defer_drop(Box::new(rec));
            }
            ctx.enqueue(Event::SeatGone { seat: id });
        }),
    );
    // SAFETY: seat live; destroy_signal announces its death through
    // this listener (§3b pairing rule).
    unsafe {
        weston_sys::wsys_wl_signal_add(&raw mut (*seat.as_ptr()).destroy_signal, destroy.raw_ptr());
    }
    destroy.mark_attached();

    SEAT_RECS.with(|m| {
        m.borrow_mut().insert(
            key,
            SeatRec {
                destroy,
                caps,
                pointer_focus,
            },
        )
    });

    // C create_shell_seat runs caps_changed once by hand.
    let rec_caps_fired = SEAT_RECS.with(|m| m.borrow().get(&key).map(|_| ()));
    if rec_caps_fired.is_some() {
        // Re-run the caps logic once (attach if a pointer exists now).
        // SAFETY: same contract as the caps handler body.
        let pointer = unsafe { weston_sys::weston_seat_get_pointer(seat.as_ptr()) };
        if let Some(pointer) = NonNull::new(pointer) {
            SEAT_RECS.with(|m| {
                if let Some(rec) = m.borrow().get(&key)
                    && !rec.pointer_focus.is_attached()
                {
                    // SAFETY: as in the caps handler.
                    unsafe {
                        weston_sys::wsys_wl_signal_add(
                            &raw mut (*pointer.as_ptr()).focus_signal,
                            rec.pointer_focus.raw_ptr(),
                        );
                    }
                    rec.pointer_focus.mark_attached();
                }
            });
        }
    }

    if announce {
        ctx.dispatch_sync(Event::SeatCreated { seat: id });
    }
}

fn register_output_shell(ctx: &Ctx, output: NonNull<weston_sys::weston_output>, announce: bool) {
    if ctx.inner.outputs.borrow().id_of(output).is_some() {
        return;
    }
    let id = crate::ids::OutputId(ctx.inner.outputs.borrow_mut().insert(output));
    let name = {
        // SAFETY: live output; name copied (§3h).
        unsafe {
            let o = output.as_ref();
            if o.name.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(o.name)
                    .to_string_lossy()
                    .into_owned()
            }
        }
    };
    let key = output.as_ptr() as usize;
    let l = Listener::new(
        "shell.output_destroy",
        true,
        Box::new(move |ctx, data| {
            let Some(p) = NonNull::new(data.cast::<weston_sys::weston_output>()) else {
                return;
            };
            // Sync half: curtain dies with its output (C
            // shell_output_destroy), registry staled.
            crate::curtain::destroy_background(ctx, id);
            ctx.inner.outputs.borrow_mut().invalidate_ptr(p);
            ctx.retire_listener(p.as_ptr() as usize);
            ctx.enqueue(Event::OutputGone {
                output: id,
                name: name.clone(),
            });
        }),
    );
    // SAFETY: output live; user-destroy listener API is the verified
    // death announcement (header fact F4).
    unsafe {
        weston_sys::weston_output_add_destroy_listener(output.as_ptr(), l.raw_ptr());
    }
    l.mark_attached();
    ctx.own_listener(key, l);

    if announce {
        ctx.dispatch_sync(Event::OutputCreated { output: id });
    }
}

fn each_compositor_item<T>(
    head: *mut weston_sys::wl_list,
    offset_of_link: usize,
    mut f: impl FnMut(NonNull<T>),
) {
    // SAFETY: caller passes a live intrusive list head; snapshot-free
    // iteration is confined to registration-time reads (§3l tolerated:
    // registration does not mutate the list).
    unsafe {
        let mut node = (*head).next;
        while node != head {
            let item = node.cast::<u8>().sub(offset_of_link).cast::<T>();
            if let Some(p) = NonNull::new(item) {
                f(p);
            }
            node = (*node).next;
        }
    }
}

/// The whole C `wet_shell_init`, hybrid edition.  `make_app` receives
/// the resolved `[shell] background-color`.
pub fn shell_init(ec_opaque: *mut c_void, make_app: impl FnOnce(u32) -> Box<dyn ShellApp>) -> bool {
    let ec = ec_opaque.cast::<weston_sys::weston_compositor>();
    if ec.is_null() {
        return false;
    }
    // NOTE: no install_stderr_handlers() here — in the hybrid phase the
    // C frontend owns weston_log's handlers (--log/flight recorder);
    // replacing them would hijack the log file (found live in smoke 3).
    let ctx = Ctx::new();
    ctx.inner.compositor.set(ec);
    // SAFETY: compositor live; wl_display field is its display.
    ctx.inner.display.set(unsafe { (*ec).wl_display });

    let bg = read_background_color(ec);

    // Compositor-destroy → full shell teardown (sync tier; the C
    // shell_destroy).  add_destroy_listener_once mirrors the C guard
    // against double init.
    let destroy = Listener::new(
        "shell.compositor_destroy",
        true,
        Box::new(move |ctx, _data| {
            ctx.dispatch_sync(Event::Shutdown);
            crate::curtain::destroy_all_backgrounds(ctx);
            crate::desktop::destroy_desktop(ctx);
            crate::layer::destroy_shell_layers(ctx);
            SEAT_RECS.with(|m| {
                for (_, rec) in m.borrow_mut().drain() {
                    // Detach while the seat signals are still valid
                    // (compositor destroy runs before seats are freed);
                    // no seat trampoline frame can be on the stack here.
                    rec.destroy.detach();
                    rec.caps.detach();
                    rec.pointer_focus.detach();
                    drop(rec);
                }
            });
            ctx.teardown();
        }),
    );
    // SAFETY: compositor live; the once-API refuses a second shell
    // (C wet_shell_init behavior).
    let added = unsafe {
        weston_sys::weston_compositor_add_destroy_listener_once(
            ec,
            destroy.raw_ptr(),
            // The notify fn inside the listener node was set by
            // Listener::new; the API needs it passed explicitly too.
            (*destroy.raw_ptr()).notify,
        )
    };
    if !added {
        ctx.teardown();
        return true; // C returns 0 (success, already-initialized)
    }
    destroy.mark_attached();
    ctx.own_listener(ec as usize, destroy);

    // Transform listener: wrapper-internal xwayland position sender.
    let transform = Listener::new(
        "shell.transform",
        false,
        Box::new(move |ctx, data| {
            if let Some(s) = NonNull::new(data.cast::<weston_sys::weston_surface>()) {
                on_transform(ctx, s);
            }
        }),
    );
    // SAFETY: compositor live; transform_signal on it.
    unsafe {
        weston_sys::wsys_wl_signal_add(&raw mut (*ec).transform_signal, transform.raw_ptr());
    }
    transform.mark_attached();
    ctx.own_listener(ec as usize ^ 1, transform);

    crate::layer::create_shell_layers(&ctx);

    if !crate::desktop::create_desktop(&ctx) {
        return false;
    }

    // App in place before any events can fire.
    ctx.set_app(make_app(bg));

    // Existing outputs (C setup_output_destroy_handler), then the
    // created/moved/resized listeners.
    // SAFETY: compositor live; offsets from the bound structs.
    unsafe {
        each_compositor_item::<weston_sys::weston_output>(
            &raw mut (*ec).output_list,
            std::mem::offset_of!(weston_sys::weston_output, link),
            |o| register_output_shell(&ctx, o, true),
        );
    }

    let out_created = Listener::new(
        "shell.output_created",
        false,
        Box::new(move |ctx, data| {
            if let Some(o) = NonNull::new(data.cast::<weston_sys::weston_output>()) {
                register_output_shell(ctx, o, true);
            }
        }),
    );
    let out_moved = Listener::new(
        "shell.output_moved",
        false,
        Box::new(move |ctx, data| {
            let Some(o) = NonNull::new(data.cast::<weston_sys::weston_output>()) else {
                return;
            };
            let Some(id) = ctx
                .inner
                .outputs
                .borrow()
                .id_of(o)
                .map(crate::ids::OutputId)
            else {
                return;
            };
            // SAFETY: output live in its moved signal; `move` is the
            // delta vector the C handler reads.
            let (dx, dy) = unsafe {
                let m = o.as_ref().move_;
                (m.c.x, m.c.y)
            };
            crate::curtain::move_backgrounds_with_output(ctx, id, dx, dy);
            ctx.enqueue(Event::OutputMoved { output: id, dx, dy });
        }),
    );
    let out_resized = Listener::new(
        "shell.output_resized",
        false,
        Box::new(move |ctx, data| {
            let Some(o) = NonNull::new(data.cast::<weston_sys::weston_output>()) else {
                return;
            };
            if let Some(id) = ctx
                .inner
                .outputs
                .borrow()
                .id_of(o)
                .map(crate::ids::OutputId)
            {
                ctx.enqueue(Event::OutputResized { output: id });
            }
        }),
    );
    let session = Listener::new(
        "shell.session",
        false,
        Box::new(move |ctx, data| {
            let ec = data.cast::<weston_sys::weston_compositor>();
            // SAFETY: compositor live in its session signal.
            if !ec.is_null() && unsafe { (*ec).session_active } {
                ctx.enqueue(Event::SessionActivated);
            }
        }),
    );
    // SAFETY: compositor live; all four signals are fields on it.
    unsafe {
        weston_sys::wsys_wl_signal_add(&raw mut (*ec).output_created_signal, out_created.raw_ptr());
        weston_sys::wsys_wl_signal_add(&raw mut (*ec).output_moved_signal, out_moved.raw_ptr());
        weston_sys::wsys_wl_signal_add(&raw mut (*ec).output_resized_signal, out_resized.raw_ptr());
        weston_sys::wsys_wl_signal_add(&raw mut (*ec).session_signal, session.raw_ptr());
    }
    for (i, l) in [out_created, out_moved, out_resized, session]
        .into_iter()
        .enumerate()
    {
        l.mark_attached();
        ctx.own_listener((ec as usize) ^ (2 + i), l);
    }

    // Existing seats + the created listener (C wet_shell_init tail).
    // SAFETY: compositor live.
    unsafe {
        each_compositor_item::<weston_sys::weston_seat>(
            &raw mut (*ec).seat_list,
            std::mem::offset_of!(weston_sys::weston_seat, link),
            |s| register_seat(&ctx, s, true),
        );
    }
    let seat_created = Listener::new(
        "shell.seat_created",
        false,
        Box::new(move |ctx, data| {
            if let Some(s) = NonNull::new(data.cast::<weston_sys::weston_seat>()) {
                register_seat(ctx, s, true);
            }
        }),
    );
    // SAFETY: compositor live.
    unsafe {
        weston_sys::wsys_wl_signal_add(&raw mut (*ec).seat_created_signal, seat_created.raw_ptr());
    }
    seat_created.mark_attached();
    ctx.own_listener((ec as usize) ^ 6, seat_created);

    create_screenshooter(ec);
    crate::input_bindings::add_shell_bindings(&ctx);

    true
}

/// C transform_handler: xwayland windows get their position re-sent
/// when a transform lands.  Wrapper-internal (no app borrow).
fn on_transform(ctx: &Ctx, surf: NonNull<weston_sys::weston_surface>) {
    let Some(id) = ctx.ds_id_of_wsurf(surf) else {
        return;
    };
    let Some(rec) = ctx.resolve_rec(id) else {
        return;
    };
    // SAFETY: compositor live; the plugin-api lookup mirrors the
    // static-inline weston_xwayland_surface_get_api (header fact F13).
    unsafe {
        let api = weston_sys::weston_plugin_api_get(
            ctx.compositor_ptr(),
            c"weston_xwayland_surface_v2".as_ptr(),
            std::mem::size_of::<weston_sys::weston_xwayland_surface_api>(),
        )
        .cast::<weston_sys::weston_xwayland_surface_api>();
        if api.is_null() {
            return;
        }
        let is_xw = (*api).is_xwayland_surface;
        let send = (*api).send_position;
        let (Some(is_xw), Some(send)) = (is_xw, send) else {
            return;
        };
        if !is_xw(surf.as_ptr()) {
            return;
        }
        if !weston_sys::weston_view_is_mapped(rec.view.as_ptr()) {
            return;
        }
        let g = &(*rec.view.as_ptr()).geometry;
        send(surf.as_ptr(), g.pos_offset.x as i32, g.pos_offset.y as i32);
    }
}
