//! Shell/frontend events (plan §3e).
//!
//! One enum for both dispatch tiers.  Sync-tier events (the closed §3e
//! list: desktop-api lifecycle, bindings, session) are delivered to the
//! app synchronously from the trampoline — each such delivery carries a
//! non-reentrancy proof in docs/callback-inventory.md (A3).  Deferred
//! events are queued and drained at depth zero.  Events carry
//! **eagerly-captured payload**: by handling time a deferred subject may
//! be dead, and its id then resolves to `None` by design.

use crate::host::ResizeEdges;
use crate::ids::{DesktopSurfaceId, OutputId, SeatId, SurfaceId};

/// How an [`Event::Activate`] was triggered; decides the input-frame
/// view target and the C activation flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivateVia {
    /// BTN_LEFT/BTN_RIGHT binding: CLICKED|CONFIGURE on the pointer
    /// focus view.
    PointerBinding,
    /// Touch binding: CONFIGURE on the touch focus view.
    TouchBinding,
    /// Left click on a busy (unresponsive) window's grab: CONFIGURE on
    /// the surface's own view.
    BusyClick,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Event {
    // ---- desktop-api tier (sync; proofs in the callback inventory) ----
    /// A new desktop surface exists; the wrapper has already created its
    /// view and registered everything.
    SurfaceAdded {
        surface: DesktopSurfaceId,
    },
    /// The C object is going away (the "half-dead window", §3a): the id
    /// no longer resolves; payload is what the policy needs.
    SurfaceRemoved {
        surface: DesktopSurfaceId,
    },
    /// Commit processed.  `resize_edges` is the wrapper-held grab state
    /// at commit time (C: `shsurf->resize_edges`); width/height/mapped
    /// are the surface's state inside this commit frame.
    Committed {
        surface: DesktopSurfaceId,
        buf_dx: f64,
        buf_dy: f64,
        resize_edges: ResizeEdges,
        width: i32,
        height: i32,
        mapped: bool,
    },
    /// Parent changed (xdg transient stacking).
    ParentSet {
        surface: DesktopSurfaceId,
        parent: Option<DesktopSurfaceId>,
    },
    /// Xwayland told us where the window goes.
    XwaylandPosition {
        surface: DesktopSurfaceId,
        x: f64,
        y: f64,
    },
    /// A client stopped answering pings; payload: all its surfaces.
    PingTimeout {
        surfaces: Vec<DesktopSurfaceId>,
    },
    /// The client answered again.
    Pong {
        surfaces: Vec<DesktopSurfaceId>,
    },

    // ---- input tier ----
    /// Activation request from a binding or the busy-cursor grab (sync:
    /// the pointer/touch focus view is read inside the input frame).
    Activate {
        seat: SeatId,
        /// The main desktop surface being activated.
        surface: DesktopSurfaceId,
        via: ActivateVia,
    },

    // ---- object lifecycle: creation (sync; inventory L27/L28) ----
    /// Delivered via `dispatch_sync` from inside the `output_created`
    /// emission (`register_output_shell`): the shell creates the
    /// background curtain here, and that in-emission creation has been
    /// load-bearing since R2b (PR19-C1 was this curtain missing).
    OutputCreated {
        output: OutputId,
    },
    /// Delivered via `dispatch_sync` from inside the `seat_created`
    /// emission (`register_seat`).
    SeatCreated {
        seat: SeatId,
    },

    // ---- object lifecycle (deferred policy halves) ----
    /// A weston_surface we tracked (keyboard focus) died; the id is
    /// already stale.  The shell runs the C focus-replacement hunt
    /// (focus_state_surface_destroy).  `main` is the eager-captured
    /// desktop surface of the *main* surface when the dead one was a
    /// sub-surface (C's "activate its main surface" branch).
    TrackedSurfaceGone {
        surface: SurfaceId,
        main: Option<DesktopSurfaceId>,
    },
    /// Output died (id already stale).
    OutputGone {
        output: OutputId,
        name: String,
    },
    /// Output geometry changed (frontend resize path).
    OutputResized {
        output: OutputId,
    },
    /// Output moved by (dx, dy); views on it follow.
    OutputMoved {
        output: OutputId,
        dx: f64,
        dy: f64,
    },
    /// Seat died (id already stale).
    SeatGone {
        seat: SeatId,
    },
    /// A move/resize/busy grab the wrapper ran has ended; `surface` may
    /// already be stale.
    GrabEnded {
        surface: DesktopSurfaceId,
    },

    // ---- session / lifecycle ----
    /// VT switch back in (sync; re-issue input activation).
    SessionActivated,
    /// Compositor teardown began (sync, from the destroy listener); the
    /// app must release its per-object state now.
    Shutdown,
    // R0 leftovers used by the frontend smoke:
    HeadsChanged,
    HeadGone {
        head: crate::ids::HeadId,
        name: String,
    },
    CompositorShutdown,
}
