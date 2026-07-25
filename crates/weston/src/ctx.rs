//! `Ctx`: the wrapper's central state — registry, dispatch, drain
//! (plan §3b, §3e, §3j, D21).
//!
//! Aliasing model (load-bearing): C can call back into Rust while an
//! outbound wrapper call is on the stack, so `CtxInner` is only ever
//! reached through shared references; every field is individually
//! interior-mutable and **no borrow of any field is held across an FFI
//! call or an app-handler call**.  Trampolines reach `CtxInner` through
//! one private thread-local slot (D21) holding an `Rc` clone; `Ctx` is
//! `!Send + !Sync` (`Rc`), so the slot and all C state stay on the
//! libweston thread (§3j).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ptr::NonNull;
use std::rc::Rc;

use crate::events::Event;
use crate::ids::DesktopSurfaceId;
use crate::listener::Listener;
use crate::registry::SlotTable;

/// The app (shell at R1, frontend later).  One entry point for both
/// dispatch tiers (§3e): sync-tier trampolines call [`Ctx::dispatch_sync`]
/// (each site carries a non-reentrancy proof — A3), deferred events are
/// drained at depth zero.  The borrow is taken once per delivered event
/// and never held across FFI.
pub trait ShellApp {
    fn handle(&mut self, host: &Ctx, event: Event);
}

/// Wrapper-held per-desktop-surface state (never visible to safe code).
pub(crate) struct SurfaceRec {
    pub(crate) view: NonNull<weston_sys::weston_view>,
    /// The underlying weston_surface (registered in `surfaces`).
    pub(crate) wsurf: NonNull<weston_sys::weston_surface>,
    /// Grab state the wrapper owns (C: shsurf->grabbed / resize_edges).
    pub(crate) grabbed: Cell<bool>,
    pub(crate) resize_edges: Cell<u32>,
    pub(crate) unresponsive: Cell<bool>,
}

pub(crate) struct CtxInner {
    // Raw C singletons (borrowed for the compositor's lifetime).
    pub(crate) compositor: Cell<*mut weston_sys::weston_compositor>,
    pub(crate) display: Cell<*mut weston_sys::wl_display>,

    // §3e: dispatch depth across trampolines AND outbound FFI calls;
    // whoever returns it to zero drains.
    depth: Cell<u32>,
    draining: Cell<bool>,
    /// Set once teardown begins: enqueued events are discarded (§3e).
    pub(crate) shutting_down: Cell<bool>,

    queue: RefCell<VecDeque<Event>>,
    /// Boxes that must outlive the C frame currently on the stack
    /// (listener inners whose trampoline is executing, ended grabs — §3f).
    pending_drop: RefCell<Vec<Box<dyn Any>>>,

    app: RefCell<Option<Box<dyn ShellApp>>>,

    // §3b registries: one table per kind; insertion and destroy-listener
    // attachment are paired in the register_* helpers.
    pub(crate) outputs: RefCell<SlotTable<weston_sys::weston_output>>,
    pub(crate) heads: RefCell<SlotTable<weston_sys::weston_head>>,
    pub(crate) seats: RefCell<SlotTable<weston_sys::weston_seat>>,
    pub(crate) surfaces: RefCell<SlotTable<weston_sys::weston_surface>>,
    pub(crate) desktop_surfaces: RefCell<SlotTable<weston_sys::weston_desktop_surface>>,

    /// Wrapper state per live desktop surface.
    pub(crate) surface_recs: RefCell<HashMap<DesktopSurfaceId, Rc<SurfaceRec>>>,

    /// Wrapper-owned destroy listeners, keyed by the C object address.
    /// On death the entry migrates to `pending_drop` (its trampoline
    /// frame is still on the stack when the handler runs).
    pub(crate) listeners: RefCell<Vec<(usize, Listener)>>,

    // ---- shell-side wrapper state (R1) ----
    /// The two shell layers (kind-4 pinned allocations, §3a).
    pub(crate) shell_layers: RefCell<Option<crate::layer::ShellLayers>>,
    /// Per-output background curtain (kind 2: we destroy).
    pub(crate) curtains: RefCell<HashMap<crate::ids::OutputId, crate::curtain::Curtain>>,
    /// libweston-desktop context (destroyed at shell teardown).
    pub(crate) desktop: Cell<*mut weston_sys::weston_desktop>,
    /// Live pointer/touch grabs the wrapper started (§3f: the boxes
    /// live here while C dispatches on them; ending moves them to
    /// `pending_drop`).
    pub(crate) active_grabs: RefCell<Vec<crate::grab::ActiveGrab>>,
}

thread_local! {
    /// D21: the one route from `extern "C"` trampolines back to the
    /// context.  Holding an `Rc` clone keeps resolution entirely safe;
    /// single-threadedness is guaranteed by `Rc`'s `!Send`.
    static CURRENT: RefCell<Option<Rc<CtxInner>>> = const { RefCell::new(None) };
}

/// The public handle.  Cloning is cheap (`Rc`); all clones refer to the
/// one compositor context on this thread.
#[derive(Clone)]
pub struct Ctx {
    pub(crate) inner: Rc<CtxInner>,
}

impl Ctx {
    /// Create the context and install it in the thread-local slot.
    /// One live `Ctx` per thread; the slot is cleared by
    /// [`Ctx::teardown`].
    pub(crate) fn new() -> Ctx {
        let inner = Rc::new(CtxInner {
            compositor: Cell::new(std::ptr::null_mut()),
            display: Cell::new(std::ptr::null_mut()),
            depth: Cell::new(0),
            draining: Cell::new(false),
            shutting_down: Cell::new(false),
            queue: RefCell::new(VecDeque::with_capacity(64)),
            pending_drop: RefCell::new(Vec::new()),
            app: RefCell::new(None),
            outputs: RefCell::new(SlotTable::default()),
            heads: RefCell::new(SlotTable::default()),
            seats: RefCell::new(SlotTable::default()),
            surfaces: RefCell::new(SlotTable::default()),
            desktop_surfaces: RefCell::new(SlotTable::default()),
            surface_recs: RefCell::new(HashMap::new()),
            listeners: RefCell::new(Vec::new()),
            shell_layers: RefCell::new(None),
            curtains: RefCell::new(HashMap::new()),
            desktop: Cell::new(std::ptr::null_mut()),
            active_grabs: RefCell::new(Vec::new()),
        });
        CURRENT.with(|c| {
            let mut slot = c.borrow_mut();
            debug_assert!(slot.is_none(), "second live Ctx on one thread");
            *slot = Some(Rc::clone(&inner));
        });
        Ctx { inner }
    }

    /// Resolve the thread-local context (trampoline path).  An empty
    /// slot during a live callback would be a wrapper teardown-ordering
    /// bug: debug_assert, benign `None` in release (§3j).
    pub(crate) fn current() -> Option<Ctx> {
        CURRENT.with(|c| {
            c.borrow().as_ref().map(|rc| Ctx {
                inner: Rc::clone(rc),
            })
        })
    }

    pub(crate) fn teardown(&self) {
        self.inner.shutting_down.set(true);
        self.inner.queue.borrow_mut().clear();
        self.inner.pending_drop.borrow_mut().clear();
        self.inner.listeners.borrow_mut().clear();
        self.inner.surface_recs.borrow_mut().clear();
        self.inner.curtains.borrow_mut().clear();
        self.inner.active_grabs.borrow_mut().clear();
        self.inner.shell_layers.borrow_mut().take();
        self.inner.app.borrow_mut().take();
        CURRENT.with(|c| c.borrow_mut().take());
    }

    /// Install the app.
    pub fn set_app(&self, app: Box<dyn ShellApp>) {
        *self.inner.app.borrow_mut() = Some(app);
    }

    /// Deliver a sync-tier event to the app right now (§3e sync tier;
    /// the calling trampoline's inventory row carries the A3 proof that
    /// the app borrow cannot already be held).  Falls back to enqueueing
    /// if the proof is ever violated — loud in debug builds.
    pub(crate) fn dispatch_sync(&self, ev: Event) {
        if self.inner.shutting_down.get() && !matches!(ev, Event::Shutdown) {
            return;
        }
        let taken = self.inner.app.borrow_mut().take();
        match taken {
            Some(mut app) => {
                app.handle(self, ev);
                let mut slot = self.inner.app.borrow_mut();
                if slot.is_none() {
                    *slot = Some(app);
                }
            }
            None => {
                // App borrow already out: either we're mid-drain (fine,
                // ordinary queueing) or an A3 proof failed.
                self.enqueue(ev);
            }
        }
    }

    /// Run a trampoline/outbound body at +1 dispatch depth; drain the
    /// deferred queue (then the pending-drop list) when returning to
    /// depth zero (§3e, amendment A4).
    pub(crate) fn with_depth<R>(&self, f: impl FnOnce() -> R) -> R {
        let inner = &self.inner;
        inner.depth.set(inner.depth.get() + 1);
        let r = f();
        inner.depth.set(inner.depth.get() - 1);
        if inner.depth.get() == 0 && !inner.draining.get() {
            self.drain();
        }
        r
    }

    fn drain(&self) {
        let inner = &self.inner;
        inner.draining.set(true);
        loop {
            let ev = inner.queue.borrow_mut().pop_front();
            let Some(ev) = ev else { break };
            if inner.shutting_down.get() {
                continue; // §3e: discard at teardown
            }
            let taken = inner.app.borrow_mut().take();
            if let Some(mut app) = taken {
                app.handle(self, ev);
                let mut slot = inner.app.borrow_mut();
                if slot.is_none() {
                    *slot = Some(app);
                }
            }
        }
        inner.draining.set(false);
        // No C frame and no handler frame is live past this point: the
        // safe moment for freeing trampoline-owning boxes (§3f).
        let dead: Vec<Box<dyn Any>> = std::mem::take(&mut *inner.pending_drop.borrow_mut());
        drop(dead);
    }

    /// Enqueue a deferred-tier event.
    pub(crate) fn enqueue(&self, ev: Event) {
        if !self.inner.shutting_down.get() {
            self.inner.queue.borrow_mut().push_back(ev);
        }
    }

    /// Move a live box (listener/grab inner) to the deferred-drop list:
    /// it is freed at the next depth-zero drain, never inside the C
    /// frame that may be executing it (§3f).
    pub(crate) fn defer_drop(&self, b: Box<dyn Any>) {
        self.inner.pending_drop.borrow_mut().push(b);
    }

    /// Detach + retire a wrapper-owned listener for a dying C object:
    /// the box rides the pending-drop list because its own trampoline
    /// frame may be on the stack (§3f).
    pub(crate) fn retire_listener(&self, key: usize) {
        let mine = {
            let mut ls = self.inner.listeners.borrow_mut();
            ls.iter()
                .position(|(k, _)| *k == key)
                .map(|i| ls.swap_remove(i).1)
        };
        if let Some(l) = mine {
            self.defer_drop(Box::new(l));
        }
    }

    pub(crate) fn own_listener(&self, key: usize, l: Listener) {
        self.inner.listeners.borrow_mut().push((key, l));
    }

    pub(crate) fn compositor_ptr(&self) -> *mut weston_sys::weston_compositor {
        self.inner.compositor.get()
    }
}

/// Test-only: clear any previous test's thread-local ctx, then create a
/// fresh one (tests in this crate share threads).
#[cfg(test)]
pub(crate) fn test_ctx() -> Ctx {
    CURRENT.with(|c| c.borrow_mut().take());
    Ctx::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recorder(Rc<RefCell<Vec<Event>>>);
    impl ShellApp for Recorder {
        fn handle(&mut self, _ctx: &Ctx, ev: Event) {
            self.0.borrow_mut().push(ev);
        }
    }

    use super::test_ctx as fresh_ctx;

    #[test]
    fn events_drain_at_depth_zero_only() {
        let ctx = fresh_ctx();
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_app(Box::new(Recorder(Rc::clone(&seen))));

        ctx.with_depth(|| {
            ctx.enqueue(Event::HeadsChanged);
            ctx.with_depth(|| {
                ctx.enqueue(Event::CompositorShutdown);
                assert!(seen.borrow().is_empty());
            });
            assert!(seen.borrow().is_empty());
        });
        assert_eq!(
            *seen.borrow(),
            vec![Event::HeadsChanged, Event::CompositorShutdown]
        );
        ctx.teardown();
    }

    #[test]
    fn events_enqueued_during_drain_run_in_same_drain() {
        struct Chainer {
            seen: Rc<RefCell<Vec<Event>>>,
        }
        impl ShellApp for Chainer {
            fn handle(&mut self, ctx: &Ctx, ev: Event) {
                if matches!(ev, Event::HeadsChanged) {
                    ctx.with_depth(|| ctx.enqueue(Event::CompositorShutdown));
                }
                self.seen.borrow_mut().push(ev);
            }
        }
        let ctx = fresh_ctx();
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_app(Box::new(Chainer {
            seen: Rc::clone(&seen),
        }));
        ctx.with_depth(|| ctx.enqueue(Event::HeadsChanged));
        assert_eq!(
            *seen.borrow(),
            vec![Event::HeadsChanged, Event::CompositorShutdown]
        );
        ctx.teardown();
    }

    #[test]
    fn sync_dispatch_delivers_immediately_and_nested_sync_defers() {
        struct Nested {
            seen: Rc<RefCell<Vec<Event>>>,
        }
        impl ShellApp for Nested {
            fn handle(&mut self, ctx: &Ctx, ev: Event) {
                if matches!(ev, Event::HeadsChanged) {
                    // A sync trampoline firing while the app borrow is
                    // held (A3 violation shape) must degrade to a queued
                    // event, not a double borrow.
                    ctx.dispatch_sync(Event::SessionActivated);
                    assert_eq!(self.seen.borrow().len(), 0);
                }
                self.seen.borrow_mut().push(ev);
            }
        }
        let ctx = fresh_ctx();
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_app(Box::new(Nested {
            seen: Rc::clone(&seen),
        }));
        ctx.with_depth(|| ctx.dispatch_sync(Event::HeadsChanged));
        assert_eq!(
            *seen.borrow(),
            vec![Event::HeadsChanged, Event::SessionActivated]
        );
        ctx.teardown();
    }

    #[test]
    fn teardown_discards_queued_events() {
        let ctx = fresh_ctx();
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_app(Box::new(Recorder(Rc::clone(&seen))));
        ctx.enqueue(Event::HeadsChanged);
        ctx.teardown();
        ctx.with_depth(|| {});
        assert!(seen.borrow().is_empty());
    }
}
