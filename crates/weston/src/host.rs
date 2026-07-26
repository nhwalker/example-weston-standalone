//! `ShellHost`: the trait boundary the shell is written against
//! (plan §3e D20, amendment A5).
//!
//! Queries return `Option<plain data>`; commands take ids and are
//! no-ops on stale ids (D13: `Option` everywhere, skipped work — never
//! a panic).  No wrapper type, pointer, or `weston_sys` name crosses
//! this trait: `westonite-shell` stays `#![forbid(unsafe_code)]` and
//! unit-tests against a mock implementation.

use std::ptr::NonNull;

use crate::ctx::Ctx;
use crate::ids::{DesktopSurfaceId, OutputId, SeatId, SurfaceId};

/// Plain geometry (POD re-declared at the fence — §3h).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputInfo {
    /// Owned at the fence (§3h): copied, lossy-UTF-8, never borrowed.
    pub name: String,
    pub geometry: Rect,
}

bitflags::bitflags! {
    /// Mirror of `enum weston_desktop_surface_edge` (§3h: no raw u32
    /// reaches the shell).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct ResizeEdges: u32 {
        const TOP = 1;
        const BOTTOM = 2;
        const LEFT = 4;
        const RIGHT = 8;
    }
}

bitflags::bitflags! {
    /// Mirror of `enum weston_activate_flag`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct ActivateFlags: u32 {
        const CONFIGURE = 1;
        const CLICKED = 2;
    }
}

/// What view input activation lands on.  `PointerFocusView` re-reads the
/// seat's pointer focus inside the (sync) input frame — the C binding
/// path activates the exact clicked view, which may be a subsurface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivateTarget {
    SurfaceView(DesktopSurfaceId),
    PointerFocusView(SeatId),
    TouchFocusView(SeatId),
}

pub trait ShellHost {
    // ---- queries ----
    /// Live outputs in the compositor's own list order (C iterates
    /// `ec->output_list`; "first output" fallbacks depend on it).
    fn outputs(&self) -> Vec<OutputId>;
    fn output_info(&self, id: OutputId) -> Option<OutputInfo>;
    fn seats(&self) -> Vec<SeatId>;
    fn pointer_pos(&self, seat: SeatId) -> Option<(f64, f64)>;
    /// Main desktop surface under the seat's pointer focus, if any.
    fn pointer_focus(&self, seat: SeatId) -> Option<DesktopSurfaceId>;
    /// Workspace-layer views, topmost first, mapped to their desktop
    /// surfaces (§3l snapshot — the focus-replacement hunt).
    fn workspace_views_top_down(&self) -> Vec<DesktopSurfaceId>;
    fn is_view_mapped(&self, id: DesktopSurfaceId) -> bool;
    fn view_pos(&self, id: DesktopSurfaceId) -> Option<(f64, f64)>;
    fn view_output(&self, id: DesktopSurfaceId) -> Option<OutputId>;
    fn surface_geometry(&self, id: DesktopSurfaceId) -> Option<Rect>;

    // ---- commands (stale id ⇒ logged no-op) ----
    /// `weston_view_activate_input` on the target; returns the id of the
    /// `weston_surface` that received focus, registered with a destroy
    /// listener that will emit `TrackedSurfaceGone` (+ its main-surface
    /// payload).  This is the C `focus_state` listener, centralized.
    /// Tracking is ref-counted: every returned id counts as one
    /// acquisition and must be balanced by one [`untrack_surface`] call
    /// (several seats focusing the same surface share one listener).
    fn activate_input(
        &self,
        target: ActivateTarget,
        seat: SeatId,
        flags: ActivateFlags,
    ) -> Option<SurfaceId>;
    /// Release one acquisition of a previously returned focus surface;
    /// watching stops only when every acquisition has been released.
    fn untrack_surface(&self, id: SurfaceId);
    fn set_activated(&self, id: DesktopSurfaceId, active: bool);
    /// Move the surface's view to the top of the workspace layer and
    /// propagate to libweston-desktop-managed child views.
    fn raise_to_workspace_top(&self, id: DesktopSurfaceId);
    fn map_surface(&self, id: DesktopSurfaceId);
    fn set_view_position(&self, id: DesktopSurfaceId, x: f64, y: f64);
    /// Position with a surface-coordinate offset (the xwayland /
    /// resize-commit path).
    fn set_view_position_with_offset(
        &self,
        id: DesktopSurfaceId,
        x: f64,
        y: f64,
        offs_x: f64,
        offs_y: f64,
    );
    /// `weston_view_update_transform` on every view of the surface.
    fn update_transforms(&self, id: DesktopSurfaceId);
    fn mark_view_dirty(&self, id: DesktopSurfaceId);
    /// The C `desktop_shell_destroy_surface` tail: unlink + destroy the
    /// shell view, drop wrapper state, invalidate registries.
    fn destroy_surface_state(&self, id: DesktopSurfaceId);
    /// Busy-cursor grab on the seat's pointer (C `set_busy_cursor`).
    fn set_busy_cursor(&self, id: DesktopSurfaceId, seat: SeatId);
    /// End busy grabs held on any seat whose grabbed surface belongs to
    /// the same desktop client as `id` (C `end_busy_cursor`).
    fn end_busy_grabs_for_client_of(&self, id: DesktopSurfaceId);
    /// (Re)create the per-output solid background curtain.
    fn recreate_background(&self, output: OutputId, argb: u32);
}

// ---------------------------------------------------------------------
// Implementation on the real wrapper.
// ---------------------------------------------------------------------

impl Ctx {
    pub(crate) fn resolve_rec(
        &self,
        id: DesktopSurfaceId,
    ) -> Option<std::rc::Rc<crate::ctx::SurfaceRec>> {
        // Rec presence implies the desktop surface is still live: the
        // removal path drops the rec before the C object dies.
        self.inner.surface_recs.borrow().get(&id).cloned()
    }

    pub(crate) fn resolve_output(
        &self,
        id: OutputId,
    ) -> Option<NonNull<weston_sys::weston_output>> {
        self.inner.outputs.borrow().resolve(id.0)
    }

    pub(crate) fn resolve_seat(&self, id: SeatId) -> Option<NonNull<weston_sys::weston_seat>> {
        self.inner.seats.borrow().resolve(id.0)
    }

    /// pointer for a seat — scoped use only, never stored (§3a).
    pub(crate) fn seat_pointer(&self, id: SeatId) -> Option<NonNull<weston_sys::weston_pointer>> {
        let seat = self.resolve_seat(id)?;
        // SAFETY: live seat; get_pointer returns NULL when no pointer
        // capability exists (§3h nullable-return rule).
        NonNull::new(unsafe { weston_sys::weston_seat_get_pointer(seat.as_ptr()) })
    }

    pub(crate) fn ds_id_of(
        &self,
        ds: NonNull<weston_sys::weston_desktop_surface>,
    ) -> Option<DesktopSurfaceId> {
        self.inner
            .desktop_surfaces
            .borrow()
            .id_of(ds)
            .map(DesktopSurfaceId)
    }

    /// weston_surface → its desktop surface id (the C
    /// `get_shell_surface`, minus the user-data round-trip).
    pub(crate) fn ds_id_of_wsurf(
        &self,
        surf: NonNull<weston_sys::weston_surface>,
    ) -> Option<DesktopSurfaceId> {
        // SAFETY: live surface pointer handed to us by C inside a
        // callback frame; both calls are pure queries.
        unsafe {
            if !weston_sys::weston_surface_is_desktop_surface(surf.as_ptr()) {
                return None;
            }
            let ds = weston_sys::weston_surface_get_desktop_surface(surf.as_ptr());
            NonNull::new(ds).and_then(|ds| self.ds_id_of(ds))
        }
    }

    /// Main surface of a weston_surface → desktop surface id.
    pub(crate) fn main_ds_id_of_wsurf(
        &self,
        surf: NonNull<weston_sys::weston_surface>,
    ) -> Option<DesktopSurfaceId> {
        // SAFETY: live surface; get_main_surface is a pure query.
        let main = unsafe { weston_sys::weston_surface_get_main_surface(surf.as_ptr()) };
        NonNull::new(main).and_then(|m| self.ds_id_of_wsurf(m))
    }
}

impl ShellHost for Ctx {
    fn outputs(&self) -> Vec<OutputId> {
        let ec = self.compositor_ptr();
        if ec.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        // SAFETY: compositor live; iterating the intrusive output_list
        // via bound POD fields, snapshotting ids (§3l).
        unsafe {
            let head = &raw mut (*ec).output_list;
            let mut node = (*head).next;
            while node != head {
                let o = crate::container_of!(node, weston_sys::weston_output, link);
                if let Some(id) = NonNull::new(o).and_then(|p| self.inner.outputs.borrow().id_of(p))
                {
                    out.push(OutputId(id));
                }
                node = (*node).next;
            }
        }
        out
    }

    fn output_info(&self, id: OutputId) -> Option<OutputInfo> {
        let ptr = self.resolve_output(id)?;
        // SAFETY: registry-live output; POD reads, name copied (§3h).
        unsafe {
            let o = ptr.as_ref();
            let name = if o.name.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(o.name)
                    .to_string_lossy()
                    .into_owned()
            };
            Some(OutputInfo {
                name,
                geometry: Rect {
                    x: o.pos.c.x as i32,
                    y: o.pos.c.y as i32,
                    width: o.width,
                    height: o.height,
                },
            })
        }
    }

    fn seats(&self) -> Vec<SeatId> {
        self.inner
            .seats
            .borrow()
            .live_ids()
            .into_iter()
            .map(SeatId)
            .collect()
    }

    fn pointer_pos(&self, seat: SeatId) -> Option<(f64, f64)> {
        let p = self.seat_pointer(seat)?;
        // SAFETY: pointer valid within this call (scoped access, §3a).
        let pos = unsafe { p.as_ref().pos };
        Some((pos.c.x, pos.c.y))
    }

    fn pointer_focus(&self, seat: SeatId) -> Option<DesktopSurfaceId> {
        let p = self.seat_pointer(seat)?;
        // SAFETY: pointer valid within this call; focus may be NULL.
        let view = NonNull::new(unsafe { p.as_ref().focus })?;
        // SAFETY: focus view is live while the pointer holds it.
        let surf = NonNull::new(unsafe { view.as_ref().surface })?;
        self.main_ds_id_of_wsurf(surf)
    }

    fn workspace_views_top_down(&self) -> Vec<DesktopSurfaceId> {
        let Some(layer) = crate::layer::workspace_layer_ptr(self) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // SAFETY: the workspace layer is a live kind-4 allocation owned
        // by us; view_list iteration mirrors wl_list_for_each over
        // layer_link.link, snapshotted before any mutation (§3l).
        unsafe {
            let head = &raw mut (*layer).view_list.link;
            let mut node = (*head).next;
            while node != head {
                let entry = crate::container_of!(node, weston_sys::weston_layer_entry, link);
                let view = crate::container_of!(entry, weston_sys::weston_view, layer_link);
                if let Some(id) = NonNull::new((*view).surface).and_then(|s| self.ds_id_of_wsurf(s))
                {
                    out.push(id);
                }
                node = (*node).next;
            }
        }
        out
    }

    fn is_view_mapped(&self, id: DesktopSurfaceId) -> bool {
        let Some(rec) = self.resolve_rec(id) else {
            return false;
        };
        // SAFETY: rec implies live view; pure query.
        unsafe { weston_sys::weston_view_is_mapped(rec.view.as_ptr()) }
    }

    fn view_pos(&self, id: DesktopSurfaceId) -> Option<(f64, f64)> {
        let rec = self.resolve_rec(id)?;
        // SAFETY: live view; pure query returning POD.
        let pos = unsafe { weston_sys::weston_view_get_pos_offset_global(rec.view.as_ptr()) };
        Some((pos.c.x, pos.c.y))
    }

    fn view_output(&self, id: DesktopSurfaceId) -> Option<OutputId> {
        let rec = self.resolve_rec(id)?;
        // SAFETY: live view; output may be NULL.
        let out = NonNull::new(unsafe { rec.view.as_ref().output })?;
        self.inner.outputs.borrow().id_of(out).map(OutputId)
    }

    fn surface_geometry(&self, id: DesktopSurfaceId) -> Option<Rect> {
        let ds = self.inner.desktop_surfaces.borrow().resolve(id.0)?;
        // SAFETY: live desktop surface; pure query returning POD.
        let g = unsafe { weston_sys::weston_desktop_surface_get_geometry(ds.as_ptr()) };
        Some(Rect {
            x: g.x,
            y: g.y,
            width: g.width,
            height: g.height,
        })
    }

    fn activate_input(
        &self,
        target: ActivateTarget,
        seat: SeatId,
        flags: ActivateFlags,
    ) -> Option<SurfaceId> {
        let seat_ptr = self.resolve_seat(seat)?;
        let view: NonNull<weston_sys::weston_view> = match target {
            ActivateTarget::SurfaceView(id) => self.resolve_rec(id)?.view,
            ActivateTarget::PointerFocusView(s) => {
                let p = self.seat_pointer(s)?;
                // SAFETY: scoped pointer access; focus may be NULL.
                NonNull::new(unsafe { p.as_ref().focus })?
            }
            ActivateTarget::TouchFocusView(s) => {
                let sp = self.resolve_seat(s)?;
                // SAFETY: live seat; touch/focus may be NULL.
                let t = NonNull::new(unsafe { weston_sys::weston_seat_get_touch(sp.as_ptr()) })?;
                // SAFETY: touch valid within this call; focus may be NULL.
                NonNull::new(unsafe { t.as_ref().focus })?
            }
        };
        // SAFETY: view + seat live; activate_input is the C call at
        // shell.c:1655.
        self.with_depth(|| unsafe {
            weston_sys::weston_view_activate_input(view.as_ptr(), seat_ptr.as_ptr(), flags.bits());
        });
        // Track the focused weston_surface (C focus_state_set_focus).
        // SAFETY: the view is live inside this activation frame.
        let es = NonNull::new(unsafe { view.as_ref().surface })?;
        Some(crate::compositor::track_surface(self, es))
    }

    fn untrack_surface(&self, id: SurfaceId) {
        crate::compositor::untrack_surface(self, id);
    }

    fn set_activated(&self, id: DesktopSurfaceId, active: bool) {
        let Some(ds) = self.inner.desktop_surfaces.borrow().resolve(id.0) else {
            return;
        };
        // SAFETY: live desktop surface.
        self.with_depth(|| unsafe {
            weston_sys::weston_desktop_surface_set_activated(ds.as_ptr(), active);
        });
    }

    fn raise_to_workspace_top(&self, id: DesktopSurfaceId) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        let Some(ds) = self.inner.desktop_surfaces.borrow().resolve(id.0) else {
            return;
        };
        let Some(layer) = crate::layer::workspace_layer_ptr(self) else {
            return;
        };
        // SAFETY: live view/layer; mirrors shell_surface_update_layer
        // (move_to_layer to the list head = top, then propagate).
        self.with_depth(|| unsafe {
            weston_sys::weston_view_move_to_layer(rec.view.as_ptr(), &raw mut (*layer).view_list);
            weston_sys::weston_desktop_surface_propagate_layer(ds.as_ptr());
        });
    }

    fn map_surface(&self, id: DesktopSurfaceId) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        self.with_depth(||
            // SAFETY: rec implies live surface; map is idempotent.
            unsafe {
                weston_sys::weston_surface_map(rec.wsurf.as_ptr());
            });
    }

    fn set_view_position(&self, id: DesktopSurfaceId, x: f64, y: f64) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        let pos = weston_sys::weston_coord_global {
            c: weston_sys::weston_coord { x, y },
        };
        // SAFETY: live view; POD argument.
        self.with_depth(|| unsafe {
            weston_sys::weston_view_set_position(rec.view.as_ptr(), pos);
        });
    }

    fn set_view_position_with_offset(
        &self,
        id: DesktopSurfaceId,
        x: f64,
        y: f64,
        offs_x: f64,
        offs_y: f64,
    ) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        let pos = weston_sys::weston_coord_global {
            c: weston_sys::weston_coord { x, y },
        };
        // SAFETY: live view; the offset must name the view's surface
        // (weston_coord_surface invariant) — rec.wsurf is exactly that.
        let offs = weston_sys::weston_coord_surface {
            c: weston_sys::weston_coord {
                x: offs_x,
                y: offs_y,
            },
            coordinate_space_id: rec.wsurf.as_ptr().cast(),
        };
        self.with_depth(||
            // SAFETY: rec implies live view; pos/offs are POD.
            unsafe {
                weston_sys::weston_view_set_position_with_offset(rec.view.as_ptr(), pos, offs);
            });
    }

    fn update_transforms(&self, id: DesktopSurfaceId) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        // SAFETY: live view/surface; iterate surface->views like the C
        // commit path, snapshot first (§3l).
        self.with_depth(|| unsafe {
            weston_sys::weston_view_update_transform(rec.view.as_ptr());
            let surf = rec.wsurf.as_ptr();
            if !(*surf).output.is_null() {
                let head = &raw mut (*surf).views;
                let mut views = Vec::new();
                let mut node = (*head).next;
                while node != head {
                    views.push(crate::container_of!(
                        node,
                        weston_sys::weston_view,
                        surface_link
                    ));
                    node = (*node).next;
                }
                for v in views {
                    weston_sys::weston_view_update_transform(v);
                }
            }
        });
    }

    fn mark_view_dirty(&self, id: DesktopSurfaceId) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        // SAFETY: live view.
        self.with_depth(|| unsafe {
            weston_sys::weston_view_geometry_dirty(rec.view.as_ptr());
        });
    }

    fn destroy_surface_state(&self, id: DesktopSurfaceId) {
        crate::desktop::destroy_surface_state(self, id);
    }

    fn set_busy_cursor(&self, id: DesktopSurfaceId, seat: SeatId) {
        crate::grab::set_busy_cursor(self, id, seat);
    }

    fn end_busy_grabs_for_client_of(&self, id: DesktopSurfaceId) {
        crate::grab::end_busy_grabs_for_client_of(self, id);
    }

    fn recreate_background(&self, output: OutputId, argb: u32) {
        crate::curtain::recreate_background(self, output, argb);
    }
}
