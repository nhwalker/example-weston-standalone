//! Opaque generational handles (plan §3b, D13).
//!
//! An id is `(slot, generation)` into the per-kind registry table — no
//! pointer, no public constructor, no conversions (§2 "opaque ids" fence
//! rule).  Ids are freely `Copy`/comparable/hashable and may outlive the
//! object; resolution through the registry is the only way to reach the
//! underlying C object, and a stale id resolves to `None`.

/// Internal representation shared by all id kinds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct RawId {
    pub(crate) slot: u32,
    pub(crate) generation: u32,
}

macro_rules! define_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub(crate) RawId);
    };
}

define_id!(
    /// A `weston_output` (kind 1: C owns, announces death).
    OutputId
);
define_id!(
    /// A `weston_head` (kind 1).
    HeadId
);
define_id!(
    /// A `weston_seat` (kind 1).
    SeatId
);
define_id!(
    /// A `weston_surface` (kind 1).
    SurfaceId
);
define_id!(
    /// A `weston_view` (kind 2 when shell-created: we destroy it).
    ViewId
);
define_id!(
    /// A `weston_desktop_surface` (kind 1, but death is announced by the
    /// `surface_removed` desktop-api callback, not a signal — §3a).
    DesktopSurfaceId
);
define_id!(
    /// A `weston_layer` we own, pinned inside the wrapper (kind 4).
    LayerId
);
define_id!(
    /// A `weston_curtain` (kind 2: we destroy it).
    CurtainId
);
