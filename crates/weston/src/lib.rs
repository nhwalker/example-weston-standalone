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
//! * Dispatch is two-tier with a depth-counted drain-at-edge (§3e):
//!   trampolines and outbound FFI calls increment the depth; whoever
//!   returns it to zero drains the deferred-event queue and then the
//!   pending-drop list.  The app state is borrowed once per delivered
//!   event, never across FFI.
//! * Every trampoline body runs inside the panic barrier: panics are
//!   caught, logged via weston_log, and turned into `abort()` — no
//!   unwinding crosses the C boundary (D16).
//! * Wrapper types are `!Send + !Sync`; ids are inert data, but resolving
//!   one requires `&Ctx`, which never leaves the libweston thread (§3j).

/// The one `container_of` helper (wrapper-internal; §3c keeps the idiom
/// in exactly one place per shape — this macro serves the read-only
/// intrusive-list walks).
macro_rules! container_of {
    ($ptr:expr, $T:ty, $field:ident) => {
        $ptr.cast::<u8>()
            .sub(core::mem::offset_of!($T, $field))
            .cast::<$T>()
    };
}
pub(crate) use container_of;

pub mod compositor;
pub mod events;
pub mod host;
pub mod ids;
pub mod log;
pub mod output_policy;

pub(crate) mod ctx;
pub(crate) mod curtain;
pub(crate) mod desktop;
pub(crate) mod grab;
pub(crate) mod input_bindings;
pub(crate) mod layer;
pub(crate) mod layoutput;
pub(crate) mod listener;
pub(crate) mod panic_barrier;
pub(crate) mod registry;
pub(crate) mod screenshooter;
pub(crate) mod xwayland;

#[cfg(feature = "hybrid-r1")]
pub mod shell_init;

pub use compositor::{
    BackendKind, Compositor, CompositorBuilder, CompositorError, DrmOptions, HeadlessOptions,
    KeyboardConfig, PipewireOptions, RendererKind, VncOptions, WaylandOptions, X11Options,
};
pub use ctx::{Ctx, ShellApp};
pub use events::{ActivateVia, Event};
pub use host::{ActivateFlags, ActivateTarget, OutputInfo, Rect, ResizeEdges, ShellHost};
pub use ids::{CurtainId, DesktopSurfaceId, HeadId, LayerId, OutputId, SeatId, SurfaceId, ViewId};
pub use layoutput::RequireOutputs;
pub use output_policy::{
    OutputCliOverrides, OutputPolicy, OutputRule, OutputSetup, OutputTransform, VncOutputSetup,
};
