//! `Grab`: the sixth primitive (plan §3f) — R0 lands the structure the
//! R1 shell port fills with policy.
//!
//! The C hazards this shape neutralizes: the allocation is handed to
//! libweston by address and freed *from inside its own callbacks* at
//! nine C sites; starting a grab can synchronously cancel and free a
//! different grab.  Here: the vtable is filled by static trampolines,
//! grab-local state lives in the pinned box (sync tier with **no app
//! borrow**, amendment A3), ending a grab defers the box to the
//! pending-drop list drained at depth zero — never freed inside the
//! frame C is dispatching on.

use std::cell::{Cell, RefCell, UnsafeCell};
use std::marker::PhantomPinned;
use std::pin::Pin;

use crate::ctx::Ctx;
use crate::panic_barrier;

/// Grab-policy callbacks.  Sync tier without app borrow: implementations
/// may touch only their own state and enqueue deferred events through
/// `ctx` (§3f); they never resolve app state.
pub(crate) trait PointerGrabHandler {
    fn focus(&mut self, _ctx: &Ctx) {}
    fn motion(&mut self, _ctx: &Ctx, _event: *mut weston_sys::weston_pointer_motion_event) {}
    fn button(&mut self, _ctx: &Ctx, _button: u32, _state: u32) {}
    fn axis(&mut self, _ctx: &Ctx, _event: *mut weston_sys::weston_pointer_axis_event) {}
    fn axis_source(&mut self, _ctx: &Ctx, _source: u32) {}
    fn frame(&mut self, _ctx: &Ctx) {}
    /// Cancel ends the grab; the wrapper defers the free (§3f).
    fn cancel(&mut self, _ctx: &Ctx) {}
}

#[repr(C)]
pub(crate) struct PointerGrabInner {
    /// MUST remain first: trampolines recover the box from the
    /// `weston_pointer_grab *` (offset math in exactly one place — here).
    /// `UnsafeCell`: C writes `grab->pointer` through the pointer we
    /// hand to weston_pointer_start_grab; `repr(transparent)` keeps the
    /// offset-0 cast valid.
    raw: UnsafeCell<weston_sys::weston_pointer_grab>,
    ended: Cell<bool>,
    what: &'static str,
    handler: RefCell<Box<dyn PointerGrabHandler>>,
    _pin: PhantomPinned,
}

/// One static vtable for every pointer grab the wrapper starts.
static POINTER_GRAB_IFACE: weston_sys::weston_pointer_grab_interface =
    weston_sys::weston_pointer_grab_interface {
        focus: Some(tramp_focus),
        motion: Some(tramp_motion),
        button: Some(tramp_button),
        axis: Some(tramp_axis),
        axis_source: Some(tramp_axis_source),
        frame: Some(tramp_frame),
        cancel: Some(tramp_cancel),
    };

/// Shared trampoline prologue: recover the inner, run `f` on the handler
/// inside the panic barrier at +1 depth.
fn dispatch(
    grab: *mut weston_sys::weston_pointer_grab,
    what_suffix: &'static str,
    f: impl FnOnce(&mut dyn PointerGrabHandler, &Ctx),
) {
    // SAFETY: C hands back the `&mut inner.raw` we passed to
    // weston_pointer_start_grab; `raw` is at offset 0 of the pinned
    // PointerGrabInner, which stays alive until the deferred drop runs
    // (never inside a dispatching frame).
    let inner: &PointerGrabInner = unsafe { &*grab.cast::<PointerGrabInner>() };
    let _ = what_suffix;
    panic_barrier::guard(inner.what, || {
        let Some(ctx) = Ctx::current() else {
            debug_assert!(false, "grab callback with no live Ctx");
            return;
        };
        ctx.with_depth(|| {
            if let Ok(mut h) = inner.handler.try_borrow_mut() {
                f(&mut **h, &ctx);
            }
        });
    });
}

unsafe extern "C" fn tramp_focus(grab: *mut weston_sys::weston_pointer_grab) {
    dispatch(grab, "focus", |h, ctx| h.focus(ctx));
}
unsafe extern "C" fn tramp_motion(
    grab: *mut weston_sys::weston_pointer_grab,
    _time: *const weston_sys::timespec,
    event: *mut weston_sys::weston_pointer_motion_event,
) {
    dispatch(grab, "motion", |h, ctx| h.motion(ctx, event));
}
unsafe extern "C" fn tramp_button(
    grab: *mut weston_sys::weston_pointer_grab,
    _time: *const weston_sys::timespec,
    button: u32,
    state: u32,
) {
    dispatch(grab, "button", |h, ctx| h.button(ctx, button, state));
}
unsafe extern "C" fn tramp_axis(
    grab: *mut weston_sys::weston_pointer_grab,
    _time: *const weston_sys::timespec,
    event: *mut weston_sys::weston_pointer_axis_event,
) {
    dispatch(grab, "axis", |h, ctx| h.axis(ctx, event));
}
unsafe extern "C" fn tramp_axis_source(grab: *mut weston_sys::weston_pointer_grab, source: u32) {
    dispatch(grab, "axis_source", |h, ctx| h.axis_source(ctx, source));
}
unsafe extern "C" fn tramp_frame(grab: *mut weston_sys::weston_pointer_grab) {
    dispatch(grab, "frame", |h, ctx| h.frame(ctx));
}
unsafe extern "C" fn tramp_cancel(grab: *mut weston_sys::weston_pointer_grab) {
    dispatch(grab, "cancel", |h, ctx| h.cancel(ctx));
    // SAFETY: same recovery as `dispatch`; only Cell state is touched.
    let inner: &PointerGrabInner = unsafe { &*grab.cast::<PointerGrabInner>() };
    inner.ended.set(true);
}

/// A started pointer grab.  R1 adds `start()` against a live
/// `weston_pointer`; at R0 the structure and end-of-life discipline are
/// pinned by unit tests driving the trampolines directly.
#[allow(dead_code)] // R0 skeleton: exercised by tests, wired up at R1
pub(crate) struct PointerGrab {
    inner: Option<Pin<Box<PointerGrabInner>>>,
}

#[allow(dead_code)] // R0 skeleton: exercised by tests, wired up at R1
impl PointerGrab {
    pub(crate) fn new(what: &'static str, handler: Box<dyn PointerGrabHandler>) -> PointerGrab {
        // SAFETY: weston_pointer_grab is POD; zeroed is valid storage,
        // fields are set before C ever sees the struct.
        let mut raw: weston_sys::weston_pointer_grab = unsafe { std::mem::zeroed() };
        raw.interface = &POINTER_GRAB_IFACE;
        PointerGrab {
            inner: Some(Box::pin(PointerGrabInner {
                raw: UnsafeCell::new(raw),
                ended: Cell::new(false),
                what,
                handler: RefCell::new(handler),
                _pin: PhantomPinned,
            })),
        }
    }

    /// The pointer C dispatches on (handed to weston_pointer_start_grab).
    #[cfg(test)]
    pub(crate) fn raw_ptr(&self) -> *mut weston_sys::weston_pointer_grab {
        self.inner
            .as_ref()
            .map(|b| b.raw.get())
            .unwrap_or(std::ptr::null_mut())
    }

    /// End the grab: the box moves to the pending-drop list and is freed
    /// at the next depth-zero drain — never inside a dispatching C frame
    /// (§3f "deferred drop").
    pub(crate) fn end(&mut self, ctx: &Ctx) {
        if let Some(b) = self.inner.take() {
            b.ended.set(true);
            ctx.defer_drop(Box::new(b));
        }
    }
}

#[cfg(test)]
#[allow(clippy::undocumented_unsafe_blocks)] // tests drive the fake-C harness directly
mod tests {
    use super::*;
    use std::rc::Rc;

    struct Counting {
        motions: Rc<Cell<u32>>,
        cancels: Rc<Cell<u32>>,
    }
    impl PointerGrabHandler for Counting {
        fn motion(&mut self, _ctx: &Ctx, _ev: *mut weston_sys::weston_pointer_motion_event) {
            self.motions.set(self.motions.get() + 1);
        }
        fn cancel(&mut self, _ctx: &Ctx) {
            self.cancels.set(self.cancels.get() + 1);
        }
    }

    #[test]
    fn trampolines_reach_handler_and_end_defers_the_free() {
        let ctx = crate::ctx::test_ctx();
        let motions = Rc::new(Cell::new(0));
        let cancels = Rc::new(Cell::new(0));
        let mut grab = PointerGrab::new(
            "test-grab",
            Box::new(Counting {
                motions: Rc::clone(&motions),
                cancels: Rc::clone(&cancels),
            }),
        );
        let raw = grab.raw_ptr();

        // Drive the vtable exactly as libweston would.
        ctx.with_depth(|| {
            // SAFETY(test): raw points at the live pinned inner.
            unsafe {
                (POINTER_GRAB_IFACE.motion.unwrap())(raw, std::ptr::null(), std::ptr::null_mut());
                (POINTER_GRAB_IFACE.cancel.unwrap())(raw);
            }
            // The C idiom frees the grab inside its own cancel callback;
            // our equivalent: end() while the "C frame" is still live —
            // the box must survive until depth returns to zero.
            grab.end(&ctx);
        });
        assert_eq!(motions.get(), 1);
        assert_eq!(cancels.get(), 1);
        ctx.teardown();
    }
}
