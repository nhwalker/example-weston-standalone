//! Deferred-tier events (plan §3e).
//!
//! Events carry **eagerly-captured payload, not just ids**: by the time a
//! deferred event is handled its subject may legitimately be dead, and
//! the id will then resolve to `None` by design.  This enum grows with
//! the R1 shell port; R0 defines the frontend-side events the smoke
//! binary and the primitives' tests need.

use crate::ids::{HeadId, OutputId, SeatId};

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// The backend's head list changed (hotplug, initial enumeration).
    /// Sync half: registry registration/invalidation already ran in the
    /// trampoline; this is the deferred policy notification.
    HeadsChanged,
    /// An output was destroyed; the id is already stale (§3b list, shape 6).
    OutputGone { output: OutputId, name: String },
    /// A head was destroyed; the id is already stale.
    HeadGone { head: HeadId, name: String },
    /// A seat appeared.
    SeatCreated { seat: SeatId },
    /// A seat died; the id is already stale.
    SeatGone { seat: SeatId },
    /// Compositor teardown began.  Queued events after this are drained
    /// normally; events left after the app observes it are discarded at
    /// teardown (§3e).
    CompositorShutdown,
}
