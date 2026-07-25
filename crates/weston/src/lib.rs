//! The fence crate (docs/rust-migration-plan.md §2, §3 — normative).
//!
//! Everything unsafe about talking to libweston lives behind this crate's
//! public API, and that API must be *sound for arbitrary safe callers*:
//! a `#![forbid(unsafe_code)]` crate holding stale handles, storing them
//! forever, or misusing the API in any order must not be able to cause
//! undefined behavior (plan goal 1).
//!
//! Load-bearing internal rules (risk R-I) — reviewers hold changes to
//! this crate against these:
//!
//! * Stored cross-references are **generational ids** resolved through
//!   the registry; resolution of a dead or recycled slot yields `None`
//!   (§3b).  Registration and destroy-listener attachment happen in one
//!   function, so no object is ever registered without its invalidator.
//! * Kind-3/kind-4 allocations (listener inners, grab inners, vtables,
//!   embedded `weston_layer`s) live behind `Pin<Box<_>>` created here;
//!   safe crates cannot name the types (§3a).
//! * Dispatch is two-tier with a depth-counted drain-at-the-edge (§3e):
//!   trampolines and outbound FFI calls increment the depth; whoever
//!   returns it to zero drains the deferred-event queue and then the
//!   pending-drop list.  The app (shell/frontend) state is borrowed once
//!   per drained event, never across FFI.
//! * Every trampoline body runs inside [`panic_barrier`]: panics are
//!   caught, logged via weston_log, and turned into `abort()` — no
//!   unwinding crosses the C boundary (D16).
//! * Wrapper types are `!Send + !Sync`; ids are inert data, but resolving
//!   one requires `&Ctx`/`&mut Ctx`, which never leaves the libweston
//!   thread (§3j).

pub mod compositor;
pub mod events;
pub mod host;
pub mod ids;
pub mod log;

pub(crate) mod ctx;
pub(crate) mod grab;
pub(crate) mod listener;
pub(crate) mod panic_barrier;
pub(crate) mod registry;

pub use compositor::{BackendKind, Compositor, CompositorBuilder, CompositorError, RendererKind};
pub use ctx::{Ctx, EventSink};
pub use events::Event;
pub use host::ShellHost;
pub use ids::{CurtainId, DesktopSurfaceId, HeadId, LayerId, OutputId, SeatId, SurfaceId, ViewId};
