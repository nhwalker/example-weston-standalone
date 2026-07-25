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
use std::collections::VecDeque;
use std::rc::Rc;

use crate::events::Event;
use crate::listener::Listener;
use crate::registry::SlotTable;

/// App-side consumer of deferred-tier events (the shell/frontend at R1+,
/// the smoke binary at R0).  Borrowed once per drained event, never
/// across FFI (§3e).
pub trait EventSink {
    fn on_event(&mut self, ctx: &Ctx, event: Event);
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

    sink: RefCell<Option<Box<dyn EventSink>>>,

    // §3b registries: one table per kind; insertion and destroy-listener
    // attachment are paired in the `compositor` module's register_*
    // functions.
    pub(crate) outputs: RefCell<SlotTable<weston_sys::weston_output>>,
    pub(crate) heads: RefCell<SlotTable<weston_sys::weston_head>>,
    #[allow(dead_code)] // populated at R1 (seat_created registration)
    pub(crate) seats: RefCell<SlotTable<weston_sys::weston_seat>>,

    /// Wrapper-owned destroy listeners, keyed by the C object address.
    /// On death the entry migrates to `pending_drop` (its trampoline
    /// frame is still on the stack when the handler runs).
    pub(crate) listeners: RefCell<Vec<(usize, Listener)>>,
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
    /// [`Ctx::teardown`] (called from `Compositor`'s drop).
    pub(crate) fn new() -> Ctx {
        let inner = Rc::new(CtxInner {
            compositor: Cell::new(std::ptr::null_mut()),
            display: Cell::new(std::ptr::null_mut()),
            depth: Cell::new(0),
            draining: Cell::new(false),
            shutting_down: Cell::new(false),
            queue: RefCell::new(VecDeque::with_capacity(64)),
            pending_drop: RefCell::new(Vec::new()),
            sink: RefCell::new(None),
            outputs: RefCell::new(SlotTable::default()),
            heads: RefCell::new(SlotTable::default()),
            seats: RefCell::new(SlotTable::default()),
            listeners: RefCell::new(Vec::new()),
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
        CURRENT.with(|c| c.borrow_mut().take());
    }

    /// Install the deferred-event consumer.
    pub fn set_sink(&self, sink: Box<dyn EventSink>) {
        *self.inner.sink.borrow_mut() = Some(sink);
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
            // Take the sink out for the call: the app borrow exists only
            // for this one event; no RefCell borrow is held while the
            // handler runs (it may re-enter the wrapper freely).
            let taken = inner.sink.borrow_mut().take();
            if let Some(mut sink) = taken {
                sink.on_event(self, ev);
                let mut slot = inner.sink.borrow_mut();
                if slot.is_none() {
                    *slot = Some(sink);
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
    impl EventSink for Recorder {
        fn on_event(&mut self, _ctx: &Ctx, ev: Event) {
            self.0.borrow_mut().push(ev);
        }
    }

    use super::test_ctx as fresh_ctx;

    #[test]
    fn events_drain_at_depth_zero_only() {
        let ctx = fresh_ctx();
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_sink(Box::new(Recorder(Rc::clone(&seen))));

        ctx.with_depth(|| {
            ctx.enqueue(Event::HeadsChanged);
            // Nested "callback": still above depth zero — nothing drains.
            ctx.with_depth(|| {
                ctx.enqueue(Event::CompositorShutdown);
                assert!(seen.borrow().is_empty());
            });
            assert!(seen.borrow().is_empty());
        });
        // Outermost frame returned to zero: both events, in order.
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
        impl EventSink for Chainer {
            fn on_event(&mut self, ctx: &Ctx, ev: Event) {
                if matches!(ev, Event::HeadsChanged) {
                    // An outbound wrapper call from a handler: enqueues
                    // and returns; must not recurse into drain.
                    ctx.with_depth(|| ctx.enqueue(Event::CompositorShutdown));
                }
                self.seen.borrow_mut().push(ev);
            }
        }
        let ctx = fresh_ctx();
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_sink(Box::new(Chainer {
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
    fn teardown_discards_queued_events() {
        let ctx = fresh_ctx();
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_sink(Box::new(Recorder(Rc::clone(&seen))));
        ctx.enqueue(Event::HeadsChanged);
        ctx.teardown();
        ctx.with_depth(|| {});
        assert!(seen.borrow().is_empty());
    }
}
