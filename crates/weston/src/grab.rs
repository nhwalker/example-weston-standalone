//! Grabs: move / resize / busy pointer grabs + the touch move grab
//! (plan §3f).  The C hazards this module owns: the allocation is
//! handed to libweston by address and freed from inside its own
//! callbacks at nine C sites; starting a grab can synchronously cancel
//! and free a different grab; the grabbed surface can die mid-grab.
//!
//! Design: grab-local state lives in the pinned box; every trampoline
//! is sync tier with **no app borrow** (A3) — it touches only the box,
//! the wrapper rec, and libweston.  Ending a grab moves the box to the
//! pending-drop list (freed at depth-zero drain, never inside the frame
//! C is dispatching on) and enqueues a deferred `GrabEnded` policy
//! event.  The back-reference is a `DesktopSurfaceId`: "surface died
//! mid-grab" is a `None` resolution, not a nulled pointer.

use std::cell::{Cell, UnsafeCell};
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::ctx::Ctx;
use crate::events::{ActivateVia, Event};
use crate::ids::{DesktopSurfaceId, SeatId};
use crate::panic_barrier;

const EDGE_TOP: u32 = 1;
const EDGE_BOTTOM: u32 = 2;
const EDGE_LEFT: u32 = 4;
const EDGE_RIGHT: u32 = 8;

// wl_fixed_* are static inlines, mirrored here.  `fixed_from` is
// wl_fixed_from_double's union trick: adding the 3 << 43 magic double
// leaves the rounded-to-nearest 24.8 value in the low mantissa bits (a
// plain `(d * 256.0) as i32` would truncate and diverge from C by one
// fixed-point unit).
fn fixed_from(d: f64) -> i32 {
    ((d + (3i64 << (51 - 8)) as f64).to_bits() as i64) as i32
}
fn fixed_to_int(f: i32) -> i32 {
    f / 256
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerGrabKind {
    Move,
    Resize,
    Busy,
}

/// `raw` MUST remain first (§3f: offset math in exactly one place —
/// the trampolines' cast).  `UnsafeCell`: C writes `grab->pointer`
/// through the handed-out pointer; `repr(transparent)` keeps the
/// offset-0 cast valid.
#[repr(C)]
pub(crate) struct PointerGrabInner {
    raw: UnsafeCell<weston_sys::weston_pointer_grab>,
    kind: PointerGrabKind,
    surface: DesktopSurfaceId,
    // move state
    delta: Cell<(f64, f64)>,
    // resize state
    edges: u32,
    start_w: i32,
    start_h: i32,
    ended: Cell<bool>,
    _pin: PhantomPinned,
}

#[repr(C)]
pub(crate) struct TouchGrabInner {
    raw: UnsafeCell<weston_sys::weston_touch_grab>,
    surface: DesktopSurfaceId,
    delta: Cell<(f64, f64)>,
    active: Cell<bool>,
    ended: Cell<bool>,
    _pin: PhantomPinned,
}

pub(crate) enum ActiveGrab {
    Pointer(Pin<Box<PointerGrabInner>>),
    Touch(Pin<Box<TouchGrabInner>>),
}

impl ActiveGrab {
    fn matches_pointer(&self, raw: *mut weston_sys::weston_pointer_grab) -> bool {
        matches!(self, ActiveGrab::Pointer(b) if b.raw.get() == raw)
    }
    fn matches_touch(&self, raw: *mut weston_sys::weston_touch_grab) -> bool {
        matches!(self, ActiveGrab::Touch(b) if b.raw.get() == raw)
    }
}

// ---------------------------------------------------------------------
// vtables
// ---------------------------------------------------------------------

static MOVE_IFACE: weston_sys::weston_pointer_grab_interface =
    weston_sys::weston_pointer_grab_interface {
        focus: Some(noop_focus),
        motion: Some(move_motion),
        button: Some(move_button),
        axis: Some(noop_axis),
        axis_source: Some(noop_axis_source),
        frame: Some(noop_frame),
        cancel: Some(move_cancel),
    };

static RESIZE_IFACE: weston_sys::weston_pointer_grab_interface =
    weston_sys::weston_pointer_grab_interface {
        focus: Some(noop_focus),
        motion: Some(resize_motion),
        button: Some(resize_button),
        axis: Some(noop_axis),
        axis_source: Some(noop_axis_source),
        frame: Some(noop_frame),
        cancel: Some(resize_cancel),
    };

static BUSY_IFACE: weston_sys::weston_pointer_grab_interface =
    weston_sys::weston_pointer_grab_interface {
        focus: Some(busy_focus),
        motion: Some(busy_motion),
        button: Some(busy_button),
        axis: Some(noop_axis),
        axis_source: Some(noop_axis_source),
        frame: Some(noop_frame),
        cancel: Some(busy_cancel),
    };

static TOUCH_MOVE_IFACE: weston_sys::weston_touch_grab_interface =
    weston_sys::weston_touch_grab_interface {
        down: Some(touch_down),
        up: Some(touch_up),
        motion: Some(touch_motion),
        frame: Some(touch_frame),
        cancel: Some(touch_cancel),
    };

/// Recover the pinned inner from the C grab pointer (the one
/// container_of of this module).
///
/// SAFETY: `grab` is the pointer we handed to weston_*_start_grab —
/// offset 0 of the still-live pinned inner (freed only via the
/// pending-drop list, never while C can still call us).
unsafe fn inner<'a>(grab: *mut weston_sys::weston_pointer_grab) -> &'a PointerGrabInner {
    // SAFETY: see function contract above.
    unsafe { &*grab.cast::<PointerGrabInner>() }
}
unsafe fn tinner<'a>(grab: *mut weston_sys::weston_touch_grab) -> &'a TouchGrabInner {
    // SAFETY: see `inner`.
    unsafe { &*grab.cast::<TouchGrabInner>() }
}

fn guard_ctx(what: &'static str, f: impl FnOnce(&Ctx)) {
    panic_barrier::guard(what, || {
        let Some(ctx) = Ctx::current() else {
            // Expected post-teardown, not a bug (unlike listeners, which
            // are detached before the Ctx goes away): grabs live at
            // compositor destroy are parked in the GRAVEYARD with C
            // still holding pointer->grab / touch->grab, and the
            // backends' input teardown cancels them AFTER the Ctx is
            // gone (weston_seat_release_pointer/touch →
            // weston_*_cancel_grab).  Graceful no-op; the pointer is
            // being destroyed anyway.
            return;
        };
        ctx.with_depth(|| f(&ctx));
    });
}

// ---- shared noops -----------------------------------------------------

unsafe extern "C" fn noop_focus(_g: *mut weston_sys::weston_pointer_grab) {}
unsafe extern "C" fn noop_axis(
    _g: *mut weston_sys::weston_pointer_grab,
    _t: *const weston_sys::timespec,
    _e: *mut weston_sys::weston_pointer_axis_event,
) {
}
unsafe extern "C" fn noop_axis_source(_g: *mut weston_sys::weston_pointer_grab, _s: u32) {}
unsafe extern "C" fn noop_frame(_g: *mut weston_sys::weston_pointer_grab) {}

// ---- move grab --------------------------------------------------------

unsafe extern "C" fn move_motion(
    grab: *mut weston_sys::weston_pointer_grab,
    _time: *const weston_sys::timespec,
    event: *mut weston_sys::weston_pointer_motion_event,
) {
    guard_ctx("grab.move.motion", |ctx| {
        // SAFETY: trampoline contract (see `inner`).
        let g = unsafe { inner(grab) };
        // SAFETY: pointer/event live for this callback.
        let pointer = unsafe { (*grab).pointer };
        // SAFETY: mirrors move_grab_motion: the pointer moves whether or
        // not the surface is still alive.
        unsafe { weston_sys::weston_pointer_move(pointer, event) };
        let Some(rec) = ctx.resolve_rec(g.surface) else {
            return;
        };
        let (dx, dy) = g.delta.get();
        // SAFETY: live pointer/view; POD math mirrors the trimmed
        // shell's move_grab_motion + constrain_position, which is plain
        // pointer->pos + delta (upstream's panel work-area clamp was
        // dropped from the trimmed shell along with the panels).
        unsafe {
            let pos = (*pointer).pos;
            let new = weston_sys::weston_coord_global {
                c: weston_sys::weston_coord {
                    x: pos.c.x + dx,
                    y: pos.c.y + dy,
                },
            };
            weston_sys::weston_view_set_position(rec.view.as_ptr(), new);
        }
    });
}

unsafe extern "C" fn move_button(
    grab: *mut weston_sys::weston_pointer_grab,
    _time: *const weston_sys::timespec,
    _button: u32,
    state: u32,
) {
    guard_ctx("grab.move.button", |ctx| {
        // SAFETY: pointer live inside its own button callback.
        let (button_count, released) = unsafe {
            let p = (*grab).pointer;
            (
                (*p).button_count,
                state == weston_sys::wl_pointer_button_state::WL_POINTER_BUTTON_STATE_RELEASED,
            )
        };
        if button_count == 0 && released {
            end_pointer_grab(ctx, grab);
        }
    });
}

unsafe extern "C" fn move_cancel(grab: *mut weston_sys::weston_pointer_grab) {
    guard_ctx("grab.move.cancel", |ctx| end_pointer_grab(ctx, grab));
}

// ---- resize grab ------------------------------------------------------

unsafe extern "C" fn resize_motion(
    grab: *mut weston_sys::weston_pointer_grab,
    _time: *const weston_sys::timespec,
    event: *mut weston_sys::weston_pointer_motion_event,
) {
    guard_ctx("grab.resize.motion", |ctx| {
        // SAFETY: trampoline contract.
        let g = unsafe { inner(grab) };
        // SAFETY: pointer/event live for this callback.
        let pointer = unsafe { (*grab).pointer };
        // SAFETY: as move_motion.
        unsafe { weston_sys::weston_pointer_move(pointer, event) };
        let Some(rec) = ctx.resolve_rec(g.surface) else {
            return;
        };
        let Some(ds) = ctx.inner.desktop_surfaces.borrow().resolve(g.surface.0) else {
            return;
        };

        // SAFETY: live view/pointer/desktop-surface; mirrors
        // resize_grab_motion's coordinate and clamp math 1:1.
        unsafe {
            weston_sys::weston_view_update_transform(rec.view.as_ptr());

            let grab_pos = (*pointer).grab_pos;
            let pos = (*pointer).pos;
            let from = weston_sys::weston_coord_global_to_surface(rec.view.as_ptr(), grab_pos);
            let to = weston_sys::weston_coord_global_to_surface(rec.view.as_ptr(), pos);
            let (from_x, from_y) = (fixed_from(from.c.x), fixed_from(from.c.y));
            let (to_x, to_y) = (fixed_from(to.c.x), fixed_from(to.c.y));

            let mut width = g.start_w;
            if g.edges & EDGE_LEFT != 0 {
                width += fixed_to_int(from_x - to_x);
            } else if g.edges & EDGE_RIGHT != 0 {
                width += fixed_to_int(to_x - from_x);
            }
            let mut height = g.start_h;
            if g.edges & EDGE_TOP != 0 {
                height += fixed_to_int(from_y - to_y);
            } else if g.edges & EDGE_BOTTOM != 0 {
                height += fixed_to_int(to_y - from_y);
            }

            let max = weston_sys::weston_desktop_surface_get_max_size(ds.as_ptr());
            let min = weston_sys::weston_desktop_surface_get_min_size(ds.as_ptr());
            let min_w = min.width.max(1);
            let min_h = min.height.max(1);
            if width < min_w {
                width = min_w;
            } else if max.width > 0 && width > max.width {
                width = max.width;
            }
            if height < min_h {
                height = min_h;
            } else if max.height > 0 && height > max.height {
                height = max.height;
            }
            weston_sys::weston_desktop_surface_set_size(ds.as_ptr(), width, height);
        }
    });
}

fn resize_finish(ctx: &Ctx, grab: *mut weston_sys::weston_pointer_grab) {
    // SAFETY: trampoline contract.
    let g = unsafe { inner(grab) };
    if let Some(ds) = ctx.inner.desktop_surfaces.borrow().resolve(g.surface.0) {
        // SAFETY: live desktop surface; mirrors resize_grab_button/
        // cancel tail.
        unsafe {
            weston_sys::weston_desktop_surface_set_resizing(ds.as_ptr(), false);
            weston_sys::weston_desktop_surface_set_size(ds.as_ptr(), 0, 0);
        }
    }
    end_pointer_grab(ctx, grab);
}

unsafe extern "C" fn resize_button(
    grab: *mut weston_sys::weston_pointer_grab,
    _time: *const weston_sys::timespec,
    _button: u32,
    state: u32,
) {
    guard_ctx("grab.resize.button", |ctx| {
        // SAFETY: pointer live.
        let (button_count, released) = unsafe {
            let p = (*grab).pointer;
            (
                (*p).button_count,
                state == weston_sys::wl_pointer_button_state::WL_POINTER_BUTTON_STATE_RELEASED,
            )
        };
        if button_count == 0 && released {
            resize_finish(ctx, grab);
        }
    });
}

unsafe extern "C" fn resize_cancel(grab: *mut weston_sys::weston_pointer_grab) {
    guard_ctx("grab.resize.cancel", |ctx| resize_finish(ctx, grab));
}

// ---- busy-cursor grab -------------------------------------------------

unsafe extern "C" fn busy_focus(grab: *mut weston_sys::weston_pointer_grab) {
    guard_ctx("grab.busy.focus", |ctx| {
        // SAFETY: trampoline contract; pointer live.
        let g = unsafe { inner(grab) };
        // SAFETY: grab->pointer live inside its own focus callback.
        let pointer = unsafe { (*grab).pointer };
        // SAFETY: mirrors busy_cursor_grab_focus: pick the view under
        // the pointer and compare desktop surfaces.
        let picked_ds = unsafe {
            let view = weston_sys::weston_compositor_pick_view(
                (*(*pointer).seat).compositor,
                (*pointer).pos,
            );
            if view.is_null() {
                None
            } else {
                NonNull::new((*view).surface).and_then(|s| ctx.ds_id_of_wsurf(s))
            }
        };
        if picked_ds != Some(g.surface) || ctx.resolve_rec(g.surface).is_none() {
            end_pointer_grab(ctx, grab);
        }
    });
}

unsafe extern "C" fn busy_motion(
    grab: *mut weston_sys::weston_pointer_grab,
    _time: *const weston_sys::timespec,
    event: *mut weston_sys::weston_pointer_motion_event,
) {
    guard_ctx("grab.busy.motion", |_ctx| {
        // SAFETY: pointer/event live for this callback.
        unsafe { weston_sys::weston_pointer_move((*grab).pointer, event) };
    });
}

unsafe extern "C" fn busy_button(
    grab: *mut weston_sys::weston_pointer_grab,
    _time: *const weston_sys::timespec,
    button: u32,
    state: u32,
) {
    guard_ctx("grab.busy.button", |ctx| {
        // SAFETY: trampoline contract; pointer/seat live.
        let g = unsafe { inner(grab) };
        const BTN_LEFT: u32 = 0x110;
        if button == BTN_LEFT && state != 0 && ctx.resolve_rec(g.surface).is_some() {
            // SAFETY: pointer/seat live in the input frame.
            let seat_id = unsafe {
                NonNull::new((*(*grab).pointer).seat)
                    .and_then(|s| ctx.inner.seats.borrow().id_of(s))
            };
            if let Some(seat) = seat_id {
                // Policy is the shell's: sync Activate event (input
                // frame), target = the busy surface's own view.
                ctx.dispatch_sync(Event::Activate {
                    seat: SeatId(seat),
                    surface: g.surface,
                    via: ActivateVia::BusyClick,
                });
            }
        }
    });
}

unsafe extern "C" fn busy_cancel(grab: *mut weston_sys::weston_pointer_grab) {
    guard_ctx("grab.busy.cancel", |ctx| end_pointer_grab(ctx, grab));
}

// ---- touch move grab --------------------------------------------------

unsafe extern "C" fn touch_down(
    _grab: *mut weston_sys::weston_touch_grab,
    _time: *const weston_sys::timespec,
    _touch_id: libc::c_int,
    _c: weston_sys::weston_coord_global,
) {
}

unsafe extern "C" fn touch_up(
    grab: *mut weston_sys::weston_touch_grab,
    _time: *const weston_sys::timespec,
    touch_id: libc::c_int,
) {
    guard_ctx("grab.touch.up", |ctx| {
        // SAFETY: trampoline contract; touch live.
        let g = unsafe { tinner(grab) };
        if touch_id == 0 {
            g.active.set(false);
        }
        // SAFETY: touch live for this callback.
        let num_tp = unsafe { (*(*grab).touch).num_tp };
        if num_tp == 0 {
            end_touch_grab(ctx, grab);
        }
    });
}

unsafe extern "C" fn touch_motion(
    grab: *mut weston_sys::weston_touch_grab,
    _time: *const weston_sys::timespec,
    _touch_id: libc::c_int,
    _c: weston_sys::weston_coord_global,
) {
    guard_ctx("grab.touch.motion", |ctx| {
        // SAFETY: trampoline contract; touch live.
        let g = unsafe { tinner(grab) };
        if !g.active.get() {
            return;
        }
        let Some(rec) = ctx.resolve_rec(g.surface) else {
            return;
        };
        let (dx, dy) = g.delta.get();
        // SAFETY: live touch/view; mirrors touch_move_grab_motion
        // (grab_pos + delta, truncated).
        unsafe {
            let gp = (*(*grab).touch).grab_pos;
            let new = weston_sys::weston_coord_global {
                c: weston_sys::weston_coord {
                    x: (gp.c.x + dx).trunc(),
                    y: (gp.c.y + dy).trunc(),
                },
            };
            weston_sys::weston_view_set_position(rec.view.as_ptr(), new);
        }
    });
}

unsafe extern "C" fn touch_frame(_grab: *mut weston_sys::weston_touch_grab) {}

unsafe extern "C" fn touch_cancel(grab: *mut weston_sys::weston_touch_grab) {
    guard_ctx("grab.touch.cancel", |ctx| end_touch_grab(ctx, grab));
}

// ---------------------------------------------------------------------
// start / end
// ---------------------------------------------------------------------

fn clear_surface_grab_flags(ctx: &Ctx, id: DesktopSurfaceId) {
    if let Some(rec) = ctx.resolve_rec(id) {
        rec.grabbed.set(false);
        rec.resize_edges.set(0);
    }
}

/// End a pointer grab: clear surface flags, `weston_pointer_end_grab`,
/// move the box to pending-drop, enqueue the policy event (§3f).
fn end_pointer_grab(ctx: &Ctx, grab: *mut weston_sys::weston_pointer_grab) {
    // SAFETY: trampoline contract.
    let g = unsafe { inner(grab) };
    if g.ended.replace(true) {
        return;
    }
    clear_surface_grab_flags(ctx, g.surface);
    let surface = g.surface;
    // SAFETY: grab->pointer was set by weston_pointer_start_grab and is
    // live here: every caller is either one of this grab's own
    // callbacks (the pointer is dispatching on us) or the
    // end_busy_grabs path, which first proves the grab is still the
    // current grab of a registry-live pointer.  Runtime device removal
    // cancels the current grab (weston_seat_release_pointer) while the
    // pointer is still live, and post-teardown cancels never get here —
    // guard_ctx bails with no Ctx and teardown parks the boxes in the
    // GRAVEYARD instead.
    unsafe { weston_sys::weston_pointer_end_grab((*grab).pointer) };
    let taken = {
        let mut grabs = ctx.inner.active_grabs.borrow_mut();
        grabs
            .iter()
            .position(|a| a.matches_pointer(grab))
            .map(|i| grabs.swap_remove(i))
    };
    if let Some(a) = taken {
        ctx.defer_drop(Box::new(a));
    }
    ctx.enqueue(Event::GrabEnded { surface });
}

fn end_touch_grab(ctx: &Ctx, grab: *mut weston_sys::weston_touch_grab) {
    // SAFETY: trampoline contract.
    let g = unsafe { tinner(grab) };
    if g.ended.replace(true) {
        return;
    }
    clear_surface_grab_flags(ctx, g.surface);
    let surface = g.surface;
    // SAFETY: grab->touch was set by weston_touch_start_grab and is
    // live here: the only callers are this grab's own callbacks, so the
    // touch is dispatching on us.  Runtime device removal cancels the
    // current grab (weston_seat_release_touch) while the touch is still
    // live, and post-teardown cancels never get here — guard_ctx bails
    // with no Ctx and teardown parks the boxes in the GRAVEYARD.
    unsafe { weston_sys::weston_touch_end_grab((*grab).touch) };
    let taken = {
        let mut grabs = ctx.inner.active_grabs.borrow_mut();
        grabs
            .iter()
            .position(|a| a.matches_touch(grab))
            .map(|i| grabs.swap_remove(i))
    };
    if let Some(a) = taken {
        ctx.defer_drop(Box::new(a));
    }
    ctx.enqueue(Event::GrabEnded { surface });
}

/// Retire a pointer grab that C no longer references (superseded: some
/// other grab replaced it via `weston_pointer_start_grab`, which never
/// cancels the old one).  The box leaves `active_grabs` and rides the
/// pending-drop list, but **no libweston call is made** — calling
/// `weston_pointer_end_grab` here would tear down the *superseding*
/// grab — and the surface's grab flags are left alone (they belong to
/// the superseding grab now).  No `GrabEnded` either: C never notices
/// a superseded grab, and policy-wise the surface's grab continues.
fn retire_superseded_pointer_grab(ctx: &Ctx, grab: *mut weston_sys::weston_pointer_grab) {
    // SAFETY: trampoline contract (see `inner`): the box is still owned
    // by active_grabs or the pending-drop list.
    let g = unsafe { inner(grab) };
    if g.ended.replace(true) {
        return;
    }
    let taken = {
        let mut grabs = ctx.inner.active_grabs.borrow_mut();
        grabs
            .iter()
            .position(|a| a.matches_pointer(grab))
            .map(|i| grabs.swap_remove(i))
    };
    if let Some(a) = taken {
        ctx.defer_drop(Box::new(a));
    }
}

/// Common pointer-grab start (C shell_grab_start): break existing
/// desktop grabs (which may synchronously cancel *another* grab — safe
/// here because that grab's box rides the pending-drop list), mark the
/// surface grabbed, hand the pinned box to libweston.
#[allow(clippy::too_many_arguments)]
fn start_pointer_grab(
    ctx: &Ctx,
    kind: PointerGrabKind,
    iface: &'static weston_sys::weston_pointer_grab_interface,
    surface: DesktopSurfaceId,
    pointer: NonNull<weston_sys::weston_pointer>,
    delta: (f64, f64),
    edges: u32,
    start_w: i32,
    start_h: i32,
) {
    // SAFETY: POD zero-init, fields set before C sees the struct.
    let mut raw: weston_sys::weston_pointer_grab = unsafe { std::mem::zeroed() };
    raw.interface = iface;
    let boxed = Box::pin(PointerGrabInner {
        raw: UnsafeCell::new(raw),
        kind,
        surface,
        delta: Cell::new(delta),
        edges,
        start_w,
        start_h,
        ended: Cell::new(false),
        _pin: PhantomPinned,
    });
    // SAFETY: pointer live inside the request frame;
    // break_desktop_grabs may re-enter our cancel trampolines — their
    // boxes are deferred-dropped, never freed under us.
    unsafe {
        weston_sys::weston_seat_break_desktop_grabs((*pointer.as_ptr()).seat);
    }
    if let Some(rec) = ctx.resolve_rec(surface) {
        rec.grabbed.set(true);
    }
    let raw_ptr = boxed.raw.get();
    ctx.inner
        .active_grabs
        .borrow_mut()
        .push(ActiveGrab::Pointer(boxed));
    // SAFETY: the box is pinned and owned by active_grabs; address
    // stable until the deferred drop after end_grab.
    unsafe { weston_sys::weston_pointer_start_grab(pointer.as_ptr(), raw_ptr) };
}

/// desktop-api move request (C desktop_surface_move + surface_move /
/// surface_touch_move).
pub(crate) fn handle_move_request(
    ctx: &Ctx,
    id: DesktopSurfaceId,
    seat: NonNull<weston_sys::weston_seat>,
    serial: u32,
) {
    let Some(rec) = ctx.resolve_rec(id) else {
        return;
    };
    // SAFETY: seat live inside the request frame; pointer/touch are
    // scoped accesses (§3a); all reads are within this frame.
    unsafe {
        let pointer = weston_sys::weston_seat_get_pointer(seat.as_ptr());
        let touch = weston_sys::weston_seat_get_touch(seat.as_ptr());

        if !pointer.is_null()
            && !(*pointer).focus.is_null()
            && (*pointer).button_count > 0
            && (*pointer).grab_serial == serial
        {
            let focus_main =
                weston_sys::weston_surface_get_main_surface((*(*pointer).focus).surface);
            if focus_main != rec.wsurf.as_ptr() {
                return;
            }
            if rec.grabbed.get() {
                return;
            }
            let vp = weston_sys::weston_view_get_pos_offset_global(rec.view.as_ptr());
            let delta = (
                vp.c.x - (*pointer).grab_pos.c.x,
                vp.c.y - (*pointer).grab_pos.c.y,
            );
            start_pointer_grab(
                ctx,
                PointerGrabKind::Move,
                &MOVE_IFACE,
                id,
                NonNull::new_unchecked(pointer),
                delta,
                0,
                0,
                0,
            );
        } else if !touch.is_null() && !(*touch).focus.is_null() && (*touch).grab_serial == serial {
            let focus_main = weston_sys::weston_surface_get_main_surface((*(*touch).focus).surface);
            if focus_main != rec.wsurf.as_ptr() {
                return;
            }
            let vp = weston_sys::weston_view_get_pos_offset_global(rec.view.as_ptr());
            let delta = (
                vp.c.x - (*touch).grab_pos.c.x,
                vp.c.y - (*touch).grab_pos.c.y,
            );
            start_touch_grab(ctx, id, NonNull::new_unchecked(touch), delta);
        }
    }
}

fn start_touch_grab(
    ctx: &Ctx,
    surface: DesktopSurfaceId,
    touch: NonNull<weston_sys::weston_touch>,
    delta: (f64, f64),
) {
    // SAFETY: POD zero-init, fields set before C sees the struct.
    let mut raw: weston_sys::weston_touch_grab = unsafe { std::mem::zeroed() };
    raw.interface = &TOUCH_MOVE_IFACE;
    let boxed = Box::pin(TouchGrabInner {
        raw: UnsafeCell::new(raw),
        surface,
        delta: Cell::new(delta),
        active: Cell::new(true),
        ended: Cell::new(false),
        _pin: PhantomPinned,
    });
    // SAFETY: touch live inside the request frame.
    unsafe {
        weston_sys::weston_seat_break_desktop_grabs((*touch.as_ptr()).seat);
    }
    if let Some(rec) = ctx.resolve_rec(surface) {
        rec.grabbed.set(true);
    }
    let raw_ptr = boxed.raw.get();
    ctx.inner
        .active_grabs
        .borrow_mut()
        .push(ActiveGrab::Touch(boxed));
    // SAFETY: pinned box owned by active_grabs (see start_pointer_grab).
    unsafe { weston_sys::weston_touch_start_grab(touch.as_ptr(), raw_ptr) };
}

/// desktop-api resize request (C desktop_surface_resize + surface_resize).
pub(crate) fn handle_resize_request(
    ctx: &Ctx,
    id: DesktopSurfaceId,
    seat: NonNull<weston_sys::weston_seat>,
    serial: u32,
    edges: u32,
) {
    let Some(rec) = ctx.resolve_rec(id) else {
        return;
    };
    let Some(ds) = ctx.inner.desktop_surfaces.borrow().resolve(id.0) else {
        return;
    };
    // SAFETY: seat live inside the request frame; scoped accesses.
    unsafe {
        let pointer = weston_sys::weston_seat_get_pointer(seat.as_ptr());
        if pointer.is_null()
            || (*pointer).button_count == 0
            || (*pointer).grab_serial != serial
            || (*pointer).focus.is_null()
        {
            return;
        }
        let focus_main = weston_sys::weston_surface_get_main_surface((*(*pointer).focus).surface);
        if focus_main != rec.wsurf.as_ptr() {
            return;
        }
        if rec.grabbed.get() {
            return;
        }
        // Invalid edge combinations (C surface_resize).
        let topbottom = EDGE_TOP | EDGE_BOTTOM;
        let leftright = EDGE_LEFT | EDGE_RIGHT;
        if edges == 0
            || edges > (topbottom | leftright)
            || (edges & topbottom) == topbottom
            || (edges & leftright) == leftright
        {
            return;
        }
        let geometry = weston_sys::weston_desktop_surface_get_geometry(ds.as_ptr());
        rec.resize_edges.set(edges);
        weston_sys::weston_desktop_surface_set_resizing(ds.as_ptr(), true);
        start_pointer_grab(
            ctx,
            PointerGrabKind::Resize,
            &RESIZE_IFACE,
            id,
            NonNull::new_unchecked(pointer),
            (0.0, 0.0),
            edges,
            geometry.width,
            geometry.height,
        );
    }
}

/// C `set_busy_cursor` (from the wrapper's pointer-focus handler or the
/// shell's ping-timeout policy).
pub(crate) fn set_busy_cursor(ctx: &Ctx, id: DesktopSurfaceId, seat: SeatId) {
    let Some(pointer) = ctx.seat_pointer(seat) else {
        return;
    };
    // SAFETY: pointer live (scoped); reading its current grab iface.
    let already_busy = unsafe {
        let g = (*pointer.as_ptr()).grab;
        !g.is_null() && std::ptr::eq((*g).interface, &BUSY_IFACE)
    };
    if already_busy || ctx.resolve_rec(id).is_none() {
        return;
    }
    start_pointer_grab(
        ctx,
        PointerGrabKind::Busy,
        &BUSY_IFACE,
        id,
        pointer,
        (0.0, 0.0),
        0,
        0,
        0,
    );
    // C quirk kept: mark ungrabbed so the move binding still works.
    if let Some(rec) = ctx.resolve_rec(id) {
        rec.grabbed.set(false);
    }
}

/// C `end_busy_cursor`: end busy grabs whose surface belongs to the
/// same desktop client as `id`.
pub(crate) fn end_busy_grabs_for_client_of(ctx: &Ctx, id: DesktopSurfaceId) {
    let Some(target_ds) = ctx.inner.desktop_surfaces.borrow().resolve(id.0) else {
        return;
    };
    // SAFETY: live desktop surface; pure query.
    let target_client =
        unsafe { weston_sys::weston_desktop_surface_get_client(target_ds.as_ptr()) };

    // Snapshot busy grabs (§3l) — ending mutates the list.
    let busy: Vec<(*mut weston_sys::weston_pointer_grab, DesktopSurfaceId)> = ctx
        .inner
        .active_grabs
        .borrow()
        .iter()
        .filter_map(|a| match a {
            ActiveGrab::Pointer(b) if b.kind == PointerGrabKind::Busy && !b.ended.get() => {
                Some((b.raw.get(), b.surface))
            }
            _ => None,
        })
        .collect();

    // Snapshot the registry-live weston_pointers (§3l): a busy grab's
    // back-pointer is dereferenced only after it matches one of these —
    // a superseded grab on a seat that has since died never saw a
    // cancel, so its `pointer` field may dangle.
    let seats: Vec<NonNull<weston_sys::weston_seat>> = {
        let t = ctx.inner.seats.borrow();
        t.live_ids()
            .into_iter()
            .filter_map(|i| t.resolve(i))
            .collect()
    };
    let mut live_pointers: Vec<*mut weston_sys::weston_pointer> = Vec::new();
    for s in seats {
        // SAFETY: registry-live seat; pure query, NULL when the seat
        // has no pointer capability.
        let p = unsafe { weston_sys::weston_seat_get_pointer(s.as_ptr()) };
        if !p.is_null() {
            live_pointers.push(p);
        }
    }

    for (raw, surf) in busy {
        let Some(ds) = ctx.inner.desktop_surfaces.borrow().resolve(surf.0) else {
            continue;
        };
        // SAFETY: live desktop surface; pure query.
        let client = unsafe { weston_sys::weston_desktop_surface_get_client(ds.as_ptr()) };
        if client != target_client {
            continue;
        }
        // C parity (end_busy_cursor checks pointer->grab->interface ==
        // &busy_cursor_grab_interface): only end a busy grab that is
        // still its pointer's CURRENT grab.  The grabbed=0 quirk in
        // set_busy_cursor lets a move/resize grab supersede a busy grab
        // via weston_pointer_start_grab — which never cancels the old
        // grab — and ending a superseded grab would reset the pointer
        // out of the *superseding* grab, stranding it.
        // SAFETY: `raw` is a live box (snapshotted from active_grabs
        // and only retirable through the deferred-drop path); reading
        // its `pointer` field is a POD read of our own allocation, and
        // `(*p).grab` is read only for a `p` proven registry-live
        // above.
        let is_current = unsafe {
            let p = (*raw).pointer;
            live_pointers.contains(&p) && std::ptr::eq((*p).grab, raw)
        };
        if is_current {
            end_pointer_grab(ctx, raw);
        } else {
            retire_superseded_pointer_grab(ctx, raw);
        }
    }
}

/// Wrapper-internal pointer-focus policy (C handle_pointer_focus): busy
/// cursor on unresponsive windows, re-ping otherwise.  No app borrow.
pub(crate) fn on_pointer_focus(ctx: &Ctx, pointer: NonNull<weston_sys::weston_pointer>) {
    // SAFETY: pointer live inside the focus signal; focus may be NULL.
    let Some(view) = NonNull::new(unsafe { pointer.as_ref().focus }) else {
        return;
    };
    // SAFETY: focus view live while the pointer holds it.
    let Some(surf) = NonNull::new(unsafe { view.as_ref().surface }) else {
        return;
    };
    let Some(id) = ctx.ds_id_of_wsurf(surf) else {
        return;
    };
    let Some(rec) = ctx.resolve_rec(id) else {
        return;
    };
    if rec.unresponsive.get() {
        // SAFETY: pointer/seat live in the focus frame.
        let seat_id = unsafe {
            NonNull::new(pointer.as_ref().seat).and_then(|s| ctx.inner.seats.borrow().id_of(s))
        };
        if let Some(seat) = seat_id {
            set_busy_cursor(ctx, id, SeatId(seat));
        }
    } else {
        let Some(ds) = ctx.inner.desktop_surfaces.borrow().resolve(id.0) else {
            return;
        };
        // SAFETY: live desktop surface; ping is fire-and-forget.
        unsafe {
            let client = weston_sys::weston_desktop_surface_get_client(ds.as_ptr());
            weston_sys::weston_desktop_client_ping(client);
        }
    }
}
