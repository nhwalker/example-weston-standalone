//! The `weston_desktop_api` vtable and desktop-surface lifecycle
//! (plan §3d; tier assignments + A3 proofs in docs/callback-inventory.md).
//!
//! All 10 installed entries are sync tier except ping_timeout/pong
//! (deferred).  Registration pairs the table insert with everything
//! that will invalidate it (§3b): the desktop surface slot is staled in
//! the `surface_removed` trampoline itself — the "no signal, API
//! callback announces death" row of the §3a table.

use std::ffi::{c_int, c_void};
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ctx::{Ctx, SurfaceRec};
use crate::events::Event;
use crate::host::ResizeEdges;
use crate::ids::DesktopSurfaceId;
use crate::panic_barrier;

static SHELL_DESKTOP_API: weston_sys::weston_desktop_api = weston_sys::weston_desktop_api {
    struct_size: std::mem::size_of::<weston_sys::weston_desktop_api>(),
    surface_added: Some(tramp_surface_added),
    surface_removed: Some(tramp_surface_removed),
    committed: Some(tramp_committed),
    move_: Some(tramp_move),
    resize: Some(tramp_resize),
    set_parent: Some(tramp_set_parent),
    ping_timeout: Some(tramp_ping_timeout),
    pong: Some(tramp_pong),
    set_xwayland_position: Some(tramp_set_xwayland_position),
    get_position: Some(tramp_get_position),
    // Not installed by the C shell (T-series trims): advertised
    // capabilities stay identical.
    show_window_menu: None,
    fullscreen_requested: None,
    maximized_requested: None,
    minimized_requested: None,
};

/// `weston_desktop_create` with our vtable (wet_shell_init tail).
pub(crate) fn create_desktop(ctx: &Ctx) -> bool {
    // SAFETY: compositor live; the vtable is 'static; user_data unused
    // (trampolines resolve Ctx through the thread-local slot, D21).
    let d = ctx.with_depth(|| unsafe {
        weston_sys::weston_desktop_create(
            ctx.compositor_ptr(),
            &SHELL_DESKTOP_API,
            std::ptr::null_mut(),
        )
    });
    ctx.inner.desktop.set(d);
    !d.is_null()
}

pub(crate) fn destroy_desktop(ctx: &Ctx) {
    let d = ctx.inner.desktop.replace(std::ptr::null_mut());
    if !d.is_null() {
        // SAFETY: created in create_desktop; single destroy at teardown
        // (C shell_destroy order).
        ctx.with_depth(|| unsafe { weston_sys::weston_desktop_destroy(d) });
    }
}

fn with_ctx(what: &'static str, f: impl FnOnce(&Ctx)) {
    panic_barrier::guard(what, || {
        let Some(ctx) = Ctx::current() else {
            debug_assert!(false, "desktop callback with no live Ctx");
            return;
        };
        ctx.with_depth(|| f(&ctx));
    });
}

/// desktop_surface_added: create the shell view, register everything,
/// then hand the shell the sync event.
unsafe extern "C" fn tramp_surface_added(
    ds: *mut weston_sys::weston_desktop_surface,
    _user_data: *mut c_void,
) {
    with_ctx("desktop.surface_added", |ctx| {
        let Some(ds) = NonNull::new(ds) else { return };
        // SAFETY: ds live inside this callback; create_view may fail.
        let view = unsafe { weston_sys::weston_desktop_surface_create_view(ds.as_ptr()) };
        let Some(view) = NonNull::new(view) else {
            return;
        };
        // SAFETY: ds live; the underlying surface outlives the desktop
        // surface.
        let wsurf = unsafe { weston_sys::weston_desktop_surface_get_surface(ds.as_ptr()) };
        let Some(wsurf) = NonNull::new(wsurf) else {
            return;
        };

        // SAFETY: label func is a libweston helper reading desktop
        // surface state; cleared again on removal.
        unsafe {
            weston_sys::weston_surface_set_label_func(
                wsurf.as_ptr(),
                Some(weston_sys::weston_shell_utils_surface_get_label),
            );
        }

        let id = DesktopSurfaceId(ctx.inner.desktop_surfaces.borrow_mut().insert(ds));
        ctx.inner.surface_recs.borrow_mut().insert(
            id,
            Rc::new(SurfaceRec {
                view,
                wsurf,
                grabbed: std::cell::Cell::new(false),
                resize_edges: std::cell::Cell::new(0),
                unresponsive: std::cell::Cell::new(false),
            }),
        );

        ctx.dispatch_sync(Event::SurfaceAdded { surface: id });
    });
}

/// desktop_surface_removed: sync invalidation + sync policy (the shell
/// must release state and destroy the view inside this call — the
/// "half-dead window", §3a).
unsafe extern "C" fn tramp_surface_removed(
    ds: *mut weston_sys::weston_desktop_surface,
    _user_data: *mut c_void,
) {
    with_ctx("desktop.surface_removed", |ctx| {
        let Some(ds) = NonNull::new(ds) else { return };
        let Some(id) = ctx.ds_id_of(ds) else { return };

        ctx.dispatch_sync(Event::SurfaceRemoved { surface: id });

        // The shell's handler calls destroy_surface_state(id); make the
        // teardown robust even if it didn't.
        destroy_surface_state(ctx, id);

        // Sync half: stale the id — every stored reference to this
        // surface is now permanently dead (§3b).
        ctx.inner.desktop_surfaces.borrow_mut().invalidate_ptr(ds);

        // SAFETY: ds/wsurf still live inside the callback (C clears the
        // label func on this path).
        unsafe {
            let wsurf = weston_sys::weston_desktop_surface_get_surface(ds.as_ptr());
            if !wsurf.is_null() {
                weston_sys::weston_surface_set_label_func(wsurf, None);
            }
        }
    });
}

/// The C `desktop_shell_destroy_surface` tail (minus the shell-state
/// bookkeeping, which lives in the safe crate): unlink + destroy the
/// shell view, drop the wrapper rec.
pub(crate) fn destroy_surface_state(ctx: &Ctx, id: DesktopSurfaceId) {
    let Some(rec) = ctx.inner.surface_recs.borrow_mut().remove(&id) else {
        return;
    };
    // SAFETY: rec implies the view is still live and ours (kind 2: we
    // destroy in the documented unlink → destroy order, shell.c:218-220).
    ctx.with_depth(|| unsafe {
        weston_sys::weston_desktop_surface_unlink_view(rec.view.as_ptr());
        weston_sys::weston_view_destroy(rec.view.as_ptr());
    });
}

/// committed: sync (must run inside commit processing).
unsafe extern "C" fn tramp_committed(
    ds: *mut weston_sys::weston_desktop_surface,
    buf_offset: weston_sys::weston_coord_surface,
    _user_data: *mut c_void,
) {
    with_ctx("desktop.committed", |ctx| {
        let Some(ds) = NonNull::new(ds) else { return };
        let Some(id) = ctx.ds_id_of(ds) else { return };
        let Some(rec) = ctx.resolve_rec(id) else {
            return;
        };
        // SAFETY: surface live inside commit processing; POD reads.
        let (width, height, mapped) = unsafe {
            let s = rec.wsurf.as_ptr();
            (
                (*s).width,
                (*s).height,
                weston_sys::weston_surface_is_mapped(s),
            )
        };
        ctx.dispatch_sync(Event::Committed {
            surface: id,
            buf_dx: buf_offset.c.x,
            buf_dy: buf_offset.c.y,
            resize_edges: ResizeEdges::from_bits_truncate(rec.resize_edges.get()),
            width,
            height,
            mapped,
        });
    });
}

/// move: sync — reads pointer/touch serial state valid only inside the
/// request frame; grab start is wrapper-internal (§3f), no app borrow.
unsafe extern "C" fn tramp_move(
    ds: *mut weston_sys::weston_desktop_surface,
    seat: *mut weston_sys::weston_seat,
    serial: u32,
    _user_data: *mut c_void,
) {
    with_ctx("desktop.move", |ctx| {
        let (Some(ds), Some(seat)) = (NonNull::new(ds), NonNull::new(seat)) else {
            return;
        };
        let Some(id) = ctx.ds_id_of(ds) else { return };
        crate::grab::handle_move_request(ctx, id, seat, serial);
    });
}

/// resize: sync (same frame contract as move).
unsafe extern "C" fn tramp_resize(
    ds: *mut weston_sys::weston_desktop_surface,
    seat: *mut weston_sys::weston_seat,
    serial: u32,
    edges: weston_sys::weston_desktop_surface_edge::Type,
    _user_data: *mut c_void,
) {
    with_ctx("desktop.resize", |ctx| {
        let (Some(ds), Some(seat)) = (NonNull::new(ds), NonNull::new(seat)) else {
            return;
        };
        let Some(id) = ctx.ds_id_of(ds) else { return };
        crate::grab::handle_resize_request(ctx, id, seat, serial, edges);
    });
}

/// set_parent: policy only — deferred would reorder against stacking
/// updates from the same request, so sync per the inventory.
unsafe extern "C" fn tramp_set_parent(
    ds: *mut weston_sys::weston_desktop_surface,
    parent: *mut weston_sys::weston_desktop_surface,
    _user_data: *mut c_void,
) {
    with_ctx("desktop.set_parent", |ctx| {
        let Some(ds) = NonNull::new(ds) else { return };
        let Some(id) = ctx.ds_id_of(ds) else { return };
        let parent = NonNull::new(parent).and_then(|p| ctx.ds_id_of(p));
        ctx.dispatch_sync(Event::ParentSet {
            surface: id,
            parent,
        });
    });
}

/// Collect all surfaces of a desktop client (ping/pong payload).
fn client_surfaces(
    ctx: &Ctx,
    client: *mut weston_sys::weston_desktop_client,
) -> Vec<DesktopSurfaceId> {
    unsafe extern "C" fn collect(
        ds: *mut weston_sys::weston_desktop_surface,
        user_data: *mut c_void,
    ) {
        panic_barrier::guard("desktop.client_surfaces", || {
            // SAFETY: user_data is the Vec below, alive across the
            // synchronous for_each call; ds live during iteration.
            let out = unsafe {
                &mut *user_data.cast::<Vec<NonNull<weston_sys::weston_desktop_surface>>>()
            };
            if let Some(p) = NonNull::new(ds) {
                out.push(p);
            }
        });
    }
    let mut raw: Vec<NonNull<weston_sys::weston_desktop_surface>> = Vec::new();
    // SAFETY: client live inside the ping/pong callback; for_each is
    // synchronous and our collector only appends to the Vec.
    unsafe {
        weston_sys::weston_desktop_client_for_each_surface(
            client,
            Some(collect),
            (&raw mut raw).cast(),
        );
    }
    raw.into_iter().filter_map(|p| ctx.ds_id_of(p)).collect()
}

/// ping_timeout: deferred policy (busy cursors); the unresponsive mark
/// is wrapper state, set synchronously like the C shell does.
unsafe extern "C" fn tramp_ping_timeout(
    client: *mut weston_sys::weston_desktop_client,
    _user_data: *mut c_void,
) {
    with_ctx("desktop.ping_timeout", |ctx| {
        let surfaces = client_surfaces(ctx, client);
        for s in &surfaces {
            if let Some(rec) = ctx.resolve_rec(*s) {
                rec.unresponsive.set(true);
            }
        }
        ctx.enqueue(Event::PingTimeout { surfaces });
    });
}

/// pong: deferred policy (end busy cursors).
unsafe extern "C" fn tramp_pong(
    client: *mut weston_sys::weston_desktop_client,
    _user_data: *mut c_void,
) {
    with_ctx("desktop.pong", |ctx| {
        let surfaces = client_surfaces(ctx, client);
        for s in &surfaces {
            if let Some(rec) = ctx.resolve_rec(*s) {
                rec.unresponsive.set(false);
            }
        }
        ctx.enqueue(Event::Pong { surfaces });
    });
}

/// set_xwayland_position: sync (the following commit consumes it).
unsafe extern "C" fn tramp_set_xwayland_position(
    ds: *mut weston_sys::weston_desktop_surface,
    pos: weston_sys::weston_coord_global,
    _user_data: *mut c_void,
) {
    with_ctx("desktop.set_xwayland_position", |ctx| {
        let Some(ds) = NonNull::new(ds) else { return };
        let Some(id) = ctx.ds_id_of(ds) else { return };
        ctx.dispatch_sync(Event::XwaylandPosition {
            surface: id,
            x: pos.c.x,
            y: pos.c.y,
        });
    });
}

/// get_position: sync with out-params, answers from wrapper-held view
/// state only — NO app borrow (A3; shell.c:1597-1605).
unsafe extern "C" fn tramp_get_position(
    ds: *mut weston_sys::weston_desktop_surface,
    x: *mut c_int,
    y: *mut c_int,
    _user_data: *mut c_void,
) {
    panic_barrier::guard("desktop.get_position", || {
        let Some(ctx) = Ctx::current() else { return };
        let (Some(x), Some(y)) = (NonNull::new(x), NonNull::new(y)) else {
            return;
        };
        let mut px = 0;
        let mut py = 0;
        if let Some(rec) = NonNull::new(ds)
            .and_then(|p| ctx.ds_id_of(p))
            .and_then(|id| ctx.resolve_rec(id))
        {
            {
                // SAFETY: live view; POD read (C reads
                // view->geometry.pos_offset).
                unsafe {
                    let g = &(*rec.view.as_ptr()).geometry;
                    px = g.pos_offset.x as c_int;
                    py = g.pos_offset.y as c_int;
                }
            }
        }
        // SAFETY: out-params supplied by libweston-desktop for this call.
        unsafe {
            *x.as_ptr() = px;
            *y.as_ptr() = py;
        }
    });
}
