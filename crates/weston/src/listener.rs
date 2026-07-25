//! `Listener`: the `wl_listener`/`container_of` primitive (plan §3c).
//!
//! One trampoline owns the pervasive C idiom: the `wl_listener` is
//! embedded at offset 0 of a pinned, wrapper-private `ListenerInner`
//! (kind-3 allocation, §3a), so the trampoline's cast *is* the one
//! `container_of` in the design.  Attach/detach are explicit;
//! `Drop` detaches; listener identity is never a lookup key (§3c).

use std::cell::{Cell, RefCell, UnsafeCell};
use std::ffi::c_void;
use std::marker::PhantomPinned;
use std::pin::Pin;

use crate::ctx::Ctx;
use crate::panic_barrier;

/// Handler contract: runs inside the panic barrier at +1 dispatch depth.
/// `data` is the raw signal argument, valid only for this call (§3a
/// kind 5) — handlers copy what they need and never store it.
pub(crate) type Handler = Box<dyn FnMut(&Ctx, *mut c_void)>;

#[repr(C)]
pub(crate) struct ListenerInner {
    /// MUST remain the first field: the trampoline recovers
    /// `ListenerInner` by casting the `wl_listener *` C hands back.
    /// `UnsafeCell`: both C (list surgery during signal add/emit) and
    /// the wrapper mutate the node through shared references —
    /// `repr(transparent)` keeps the offset-0 cast valid.
    raw: UnsafeCell<weston_sys::wl_listener>,
    attached: Cell<bool>,
    /// One-shot listeners (destroy signals) self-detach inside the
    /// emission — after it, the signal owner's list head is gone and a
    /// later `wl_list_remove` would touch freed memory.
    oneshot: bool,
    what: &'static str,
    handler: RefCell<Handler>,
    _pin: PhantomPinned,
}

impl ListenerInner {
    fn link_ptr(&self) -> *mut weston_sys::wl_list {
        // SAFETY: projecting a raw field pointer out of the UnsafeCell;
        // no reference is formed.
        unsafe { &raw mut (*self.raw.get()).link }
    }
}

unsafe extern "C" fn trampoline(listener: *mut weston_sys::wl_listener, data: *mut c_void) {
    // SAFETY: C hands back exactly the pointer we attached — offset 0 of
    // the pinned, still-live `ListenerInner` (detach-before-drop
    // invariant), so the cast recovers the allocation.
    let inner: &ListenerInner = unsafe { &*listener.cast::<ListenerInner>() };
    panic_barrier::guard(inner.what, || {
        if inner.oneshot && inner.attached.get() {
            // Removing the current node during wl_signal_emit is safe
            // (emit iterates with the safe variant); afterwards the
            // dying owner's list must never see this node again.
            // SAFETY: the link is a live list node while attached.
            unsafe {
                weston_sys::wsys_wl_list_remove(inner.link_ptr());
                weston_sys::wsys_wl_list_init(inner.link_ptr());
            }
            inner.attached.set(false);
        }
        let Some(ctx) = Ctx::current() else {
            // A callback with no live Ctx is a wrapper teardown-ordering
            // bug (§3j): loud in debug, benign no-op in release.
            debug_assert!(false, "listener fired with no live Ctx");
            return;
        };
        ctx.with_depth(|| {
            // try_borrow: the same listener re-entered synchronously
            // (emit within emit) would double-borrow the FnMut; skip —
            // the C idiom has no answer for that shape either.
            if let Ok(mut h) = inner.handler.try_borrow_mut() {
                h(&ctx, data);
            }
        });
    });
}

pub(crate) struct Listener {
    inner: Pin<Box<ListenerInner>>,
}

impl Listener {
    pub(crate) fn new(what: &'static str, oneshot: bool, handler: Handler) -> Listener {
        // SAFETY: wl_listener is POD; a zeroed value is valid storage.
        // The link is properly initialized right below, before any use.
        let mut raw: weston_sys::wl_listener = unsafe { std::mem::zeroed() };
        raw.notify = Some(trampoline);
        let inner = Box::pin(ListenerInner {
            raw: UnsafeCell::new(raw),
            attached: Cell::new(false),
            oneshot,
            what,
            handler: RefCell::new(handler),
            _pin: PhantomPinned,
        });
        // SAFETY: initializing the pinned node's own link; the address
        // is stable from here on (Pin<Box>).
        unsafe {
            weston_sys::wsys_wl_list_init(inner.link_ptr());
        }
        Listener { inner }
    }

    /// Attach to a signal.  Caller (wrapper code only) guarantees the
    /// signal outlives the attachment or announces its owner's death
    /// through this very listener (oneshot destroy pattern).
    #[allow(dead_code)] // R0: used by tests; wl_signal attach sites arrive at R1
    pub(crate) unsafe fn attach(&self, signal: *mut weston_sys::wl_signal) {
        debug_assert!(!self.inner.attached.get(), "double attach");
        // SAFETY: caller contract (live signal); the node is initialized
        // and currently detached.
        unsafe {
            weston_sys::wsys_wl_signal_add(signal, self.inner.raw.get());
        }
        self.inner.attached.set(true);
    }

    pub(crate) fn detach(&self) {
        if self.inner.attached.get() {
            // SAFETY: attached ⇒ the node is on a live list (the oneshot
            // path already cleared `attached` before the owner died).
            unsafe {
                weston_sys::wsys_wl_list_remove(self.inner.link_ptr());
                weston_sys::wsys_wl_list_init(self.inner.link_ptr());
            }
            self.inner.attached.set(false);
        }
    }

    /// The C-facing node, for registration APIs that take the
    /// `wl_listener *` directly (wrapper code only).  Callers that hand
    /// this to a C add-listener API MUST call [`Listener::mark_attached`]
    /// right after, or detach-on-drop will skip the node.
    pub(crate) fn raw_ptr(&self) -> *mut weston_sys::wl_listener {
        self.inner.raw.get()
    }

    /// Record that a C-side add-listener API put the node on a list
    /// (same effect as [`Listener::attach`], for APIs that do the
    /// wl_signal_add themselves).
    pub(crate) fn mark_attached(&self) {
        debug_assert!(!self.inner.attached.get(), "double attach");
        self.inner.attached.set(true);
    }

    pub(crate) fn is_attached(&self) -> bool {
        self.inner.attached.get()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.detach();
    }
}

#[cfg(test)]
#[allow(clippy::undocumented_unsafe_blocks)] // tests drive the fake-C harness directly
mod tests {
    use super::*;
    use std::rc::Rc;

    // Uses the fake-C-object harness (D18): a real wl_signal from the
    // shim, real wl_list links, no compositor.

    #[test]
    fn emit_reaches_handler_and_multishot_stays_attached() {
        let ctx = crate::ctx::test_ctx();
        let hits = Rc::new(Cell::new(0u32));
        let h = Rc::clone(&hits);
        let l = Listener::new(
            "test",
            false,
            Box::new(move |_ctx, _data| h.set(h.get() + 1)),
        );
        // SAFETY(test): the harness signal outlives the attachment.
        let sig = unsafe { weston_sys::wsys_test_signal_create() };
        unsafe { l.attach(sig.cast()) };
        unsafe { weston_sys::wsys_test_signal_emit(sig, std::ptr::null_mut()) };
        unsafe { weston_sys::wsys_test_signal_emit(sig, std::ptr::null_mut()) };
        assert_eq!(hits.get(), 2);
        assert!(l.is_attached());
        l.detach();
        unsafe { weston_sys::wsys_test_signal_emit(sig, std::ptr::null_mut()) };
        assert_eq!(hits.get(), 2);
        unsafe { weston_sys::wsys_test_signal_destroy(sig) };
        ctx.teardown();
    }

    #[test]
    fn oneshot_self_detaches_inside_emission() {
        let ctx = crate::ctx::test_ctx();
        let hits = Rc::new(Cell::new(0u32));
        let h = Rc::clone(&hits);
        let l = Listener::new(
            "test-oneshot",
            true,
            Box::new(move |_ctx, _| h.set(h.get() + 1)),
        );
        let sig = unsafe { weston_sys::wsys_test_signal_create() };
        unsafe { l.attach(sig.cast()) };
        unsafe { weston_sys::wsys_test_signal_emit(sig, std::ptr::null_mut()) };
        assert_eq!(hits.get(), 1);
        assert!(!l.is_attached());
        // Owner death after emission: destroying the signal (freeing its
        // list head) must be safe with the node already off the list,
        // and dropping the listener afterwards must not touch it again.
        unsafe { weston_sys::wsys_test_signal_destroy(sig) };
        drop(l);
        ctx.teardown();
    }

    #[test]
    fn drop_detaches_from_live_signal() {
        let ctx = crate::ctx::test_ctx();
        let hits = Rc::new(Cell::new(0u32));
        let h = Rc::clone(&hits);
        let l = Listener::new(
            "test-drop",
            false,
            Box::new(move |_ctx, _| h.set(h.get() + 1)),
        );
        let sig = unsafe { weston_sys::wsys_test_signal_create() };
        unsafe { l.attach(sig.cast()) };
        drop(l);
        // The dropped listener's node must be gone from the list.
        unsafe { weston_sys::wsys_test_signal_emit(sig, std::ptr::null_mut()) };
        assert_eq!(hits.get(), 0);
        unsafe { weston_sys::wsys_test_signal_destroy(sig) };
        ctx.teardown();
    }
}
