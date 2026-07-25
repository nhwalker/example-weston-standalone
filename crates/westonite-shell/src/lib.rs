//! `westonite-shell`: the desktop-shell policy state machine — the safe
//! port of `desktop-shell/shell.c` (plan §4, D4/D20).
//!
//! Plain structs, plain maps, `Copy` ids, `&mut self` — no `RefCell`,
//! no `Rc`, no lifetimes (plan goal 2).  Every wrapper result is an
//! `Option` handled with `let … else` skips (D13); `unwrap`/`expect`
//! are compile errors here.  All host interaction goes through
//! [`weston::ShellHost`], so the whole state machine unit-tests against
//! the mock in `tests/`.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use weston::{
    ActivateFlags, ActivateTarget, ActivateVia, DesktopSurfaceId, Event, OutputId, ResizeEdges,
    SeatId, ShellHost, SurfaceId,
};

#[derive(Debug, Clone, Copy)]
pub struct ShellConfig {
    /// `[shell] background-color` (C default 0xff002244).
    pub background_color: u32,
}

impl Default for ShellConfig {
    fn default() -> Self {
        ShellConfig {
            background_color: 0xff002244,
        }
    }
}

#[derive(Debug, Default)]
struct SurfState {
    parent: Option<DesktopSurfaceId>,
    /// Stacking-order children (C children_list, tail = most recent).
    children: Vec<DesktopSurfaceId>,
    focus_count: i32,
    xwayland: Option<(f64, f64)>,
    last_width: i32,
    last_height: i32,
}

#[derive(Debug, Default)]
struct SeatState {
    /// The main desktop surface last activated on this seat
    /// (C shell_seat.focused_surface).
    focused_surface: Option<DesktopSurfaceId>,
}

pub struct Shell {
    config: ShellConfig,
    surfaces: HashMap<DesktopSurfaceId, SurfState>,
    seats: HashMap<SeatId, SeatState>,
    /// Per-seat keyboard-focus tracking (C focus_state of the single
    /// workspace): the actual focused weston_surface, watched by the
    /// wrapper until untracked.
    focus: HashMap<SeatId, SurfaceId>,
    /// Deterministic placement rng (C uses unseeded random(), which is
    /// deterministic too; bit-parity is not required — §10 cosmetic).
    rng: u64,
}

impl Shell {
    pub fn new(config: ShellConfig) -> Shell {
        Shell {
            config,
            surfaces: HashMap::new(),
            seats: HashMap::new(),
            focus: HashMap::new(),
            rng: 0x853c49e6748fea9b,
        }
    }

    fn next_rand(&mut self, modulo: i32) -> i32 {
        // xorshift64*; only used for initial window placement.
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        if modulo <= 0 {
            0
        } else {
            ((x.wrapping_mul(0x2545F4914F6CDD1D) >> 33) % modulo as u64) as i32
        }
    }

    /// The public entry point; `weston`'s dispatch calls this through
    /// [`weston::ShellApp`].
    pub fn on_event(&mut self, host: &dyn ShellHost, ev: Event) {
        match ev {
            Event::SurfaceAdded { surface } => {
                self.surfaces.insert(surface, SurfState::default());
            }
            Event::SurfaceRemoved { surface } => self.surface_removed(host, surface),
            Event::Committed {
                surface,
                buf_dx,
                buf_dy,
                resize_edges,
                width,
                height,
                mapped,
            } => self.committed(
                host,
                surface,
                buf_dx,
                buf_dy,
                resize_edges,
                width,
                height,
                mapped,
            ),
            Event::ParentSet { surface, parent } => self.set_parent(surface, parent),
            Event::XwaylandPosition { surface, x, y } => {
                if let Some(st) = self.surfaces.get_mut(&surface) {
                    st.xwayland = Some((x, y));
                }
            }
            Event::PingTimeout { surfaces } => {
                // C desktop_surface_ping_timeout: busy cursor on every
                // seat whose pointer focus is one of the client's
                // surfaces.
                for seat in host.seats() {
                    let Some(focus) = host.pointer_focus(seat) else {
                        continue;
                    };
                    if surfaces.contains(&focus) {
                        host.set_busy_cursor(focus, seat);
                    }
                }
            }
            Event::Pong { surfaces } => {
                if let Some(first) = surfaces.first() {
                    host.end_busy_grabs_for_client_of(*first);
                }
            }
            Event::Activate { seat, surface, via } => {
                let (flags, target) = match via {
                    ActivateVia::PointerBinding => (
                        ActivateFlags::CLICKED | ActivateFlags::CONFIGURE,
                        ActivateTarget::PointerFocusView(seat),
                    ),
                    ActivateVia::TouchBinding => (
                        ActivateFlags::CONFIGURE,
                        ActivateTarget::TouchFocusView(seat),
                    ),
                    ActivateVia::BusyClick => (
                        ActivateFlags::CONFIGURE,
                        ActivateTarget::SurfaceView(surface),
                    ),
                };
                self.activate(host, surface, seat, flags, target);
            }
            Event::TrackedSurfaceGone { surface, main } => {
                self.focus_surface_gone(host, surface, main)
            }
            Event::OutputCreated { output } => {
                host.recreate_background(output, self.config.background_color);
                // C create_shell_output: when this is the first output,
                // re-fit views that were left off-screen.
                if host.outputs().len() == 1 {
                    self.reposition_all(host);
                }
            }
            Event::OutputGone { .. } => self.reposition_all(host),
            Event::OutputResized { output } => {
                host.recreate_background(output, self.config.background_color);
            }
            Event::OutputMoved { output, dx, dy } => {
                let ids: Vec<DesktopSurfaceId> = self.surfaces.keys().copied().collect();
                for id in ids {
                    if host.view_output(id) == Some(output) {
                        let Some((x, y)) = host.view_pos(id) else {
                            continue;
                        };
                        host.set_view_position(id, x + dx, y + dy);
                    }
                }
            }
            Event::SeatCreated { seat } => {
                self.seats.insert(seat, SeatState::default());
            }
            Event::SeatGone { seat } => {
                if let Some(old) = self.focus.remove(&seat) {
                    host.untrack_surface(old);
                }
                self.seats.remove(&seat);
            }
            Event::SessionActivated => {
                // C desktop_shell_notify_session: re-issue input focus.
                let entries: Vec<(SeatId, DesktopSurfaceId)> = self
                    .seats
                    .iter()
                    .filter_map(|(s, st)| st.focused_surface.map(|f| (*s, f)))
                    .collect();
                for (seat, focused) in entries {
                    host.activate_input(
                        ActivateTarget::SurfaceView(focused),
                        seat,
                        ActivateFlags::empty(),
                    );
                }
            }
            Event::Shutdown => {
                // C shell_destroy → desktop_shell_destroy_layer: destroy
                // every shell surface, then drop all bookkeeping.
                let ids: Vec<DesktopSurfaceId> = self.surfaces.keys().copied().collect();
                for id in ids {
                    host.destroy_surface_state(id);
                }
                self.surfaces.clear();
                self.seats.clear();
                self.focus.clear();
            }
            // Wrapper-internal grab bookkeeping already done; frontend
            // events (heads) are not the shell's business.
            _ => {}
        }
    }

    // -- lifecycle ----------------------------------------------------

    fn surface_removed(&mut self, host: &dyn ShellHost, id: DesktopSurfaceId) {
        // C desktop_surface_removed: invalidate stale focused_surface
        // references before running the teardown.
        for st in self.seats.values_mut() {
            if st.focused_surface == Some(id) {
                st.focused_surface = None;
            }
        }
        if let Some(st) = self.surfaces.remove(&id) {
            // Children lose their parent link (C unlinks each child);
            // they keep living unparented.
            for child in st.children {
                if let Some(c) = self.surfaces.get_mut(&child) {
                    c.parent = None;
                }
            }
            if let Some(parent) = st.parent
                && let Some(p) = self.surfaces.get_mut(&parent)
            {
                p.children.retain(|c| *c != id);
            }
        }
        host.destroy_surface_state(id);
    }

    #[allow(clippy::too_many_arguments)]
    fn committed(
        &mut self,
        host: &dyn ShellHost,
        id: DesktopSurfaceId,
        buf_dx: f64,
        buf_dy: f64,
        resize_edges: ResizeEdges,
        width: i32,
        height: i32,
        mapped: bool,
    ) {
        if !self.surfaces.contains_key(&id) || width == 0 {
            return;
        }

        if !mapped {
            self.map(host, id, width, height);
            return;
        }

        let (last_w, last_h) = self
            .surfaces
            .get(&id)
            .map(|st| (st.last_width, st.last_height))
            .unwrap_or((0, 0));

        if buf_dx == 0.0 && buf_dy == 0.0 && last_w == width && last_h == height {
            return;
        }

        host.update_transforms(id);

        // C desktop_surface_committed's offset block.
        let mut offs = (buf_dx, buf_dy);
        if !resize_edges.is_empty() {
            offs = (0.0, 0.0);
        }
        if resize_edges.contains(ResizeEdges::LEFT) {
            offs.0 = (last_w - width) as f64;
        }
        if resize_edges.contains(ResizeEdges::TOP) {
            offs.1 = (last_h - height) as f64;
        }
        if let Some((x, y)) = host.view_pos(id) {
            host.set_view_position_with_offset(id, x, y, offs.0, offs.1);
        }

        if let Some(st) = self.surfaces.get_mut(&id) {
            st.last_width = width;
            st.last_height = height;
        }
        host.update_transforms(id);
    }

    /// C map(): initial position, map, stacking, activation on every
    /// seat.
    fn map(&mut self, host: &dyn ShellHost, id: DesktopSurfaceId, width: i32, height: i32) {
        let xwayland = self.surfaces.get(&id).and_then(|st| st.xwayland);
        if let Some((x, y)) = xwayland {
            // C set_position_from_xwayland.
            let g = host.surface_geometry(id).unwrap_or_default();
            host.set_view_position_with_offset(id, x, y, -g.x as f64, -g.y as f64);
        } else {
            self.set_initial_position(host, id, width, height);
        }

        host.map_surface(id);
        host.raise_to_workspace_top(id);

        for seat in host.seats() {
            self.activate(
                host,
                id,
                seat,
                ActivateFlags::CONFIGURE,
                ActivateTarget::SurfaceView(id),
            );
        }
    }

    /// C weston_view_set_initial_position.
    fn set_initial_position(
        &mut self,
        host: &dyn ShellHost,
        id: DesktopSurfaceId,
        width: i32,
        height: i32,
    ) {
        let mut pos = (0.0, 0.0);
        for seat in host.seats() {
            if let Some(p) = host.pointer_pos(seat) {
                pos = p;
                break;
            }
        }

        let mut target = None;
        for out in host.outputs() {
            let Some(info) = host.output_info(out) else {
                continue;
            };
            if contains(&info.geometry, pos) {
                target = Some(info.geometry);
                break;
            }
        }

        let Some(area) = target else {
            let x = 10 + self.next_rand(400);
            let y = 10 + self.next_rand(400);
            host.set_view_position(id, x as f64, y as f64);
            return;
        };

        let mut x = area.x;
        let mut y = area.y;
        let range_x = area.width - width;
        let range_y = area.height - height;
        if range_x > 0 {
            x += self.next_rand(range_x);
        }
        if range_y > 0 {
            y += self.next_rand(range_y);
        }
        host.set_view_position(id, x as f64, y as f64);
    }

    fn set_parent(&mut self, id: DesktopSurfaceId, parent: Option<DesktopSurfaceId>) {
        let old_parent = match self.surfaces.get(&id) {
            Some(st) => st.parent,
            None => return,
        };
        if let Some(op) = old_parent
            && let Some(p) = self.surfaces.get_mut(&op)
        {
            p.children.retain(|c| *c != id);
        }
        if let Some(np) = parent
            && self.surfaces.contains_key(&np)
        {
            if let Some(p) = self.surfaces.get_mut(&np) {
                p.children.push(id);
            }
            if let Some(st) = self.surfaces.get_mut(&id) {
                st.parent = Some(np);
            }
            return;
        }
        if let Some(st) = self.surfaces.get_mut(&id) {
            st.parent = None;
        }
    }

    // -- activation / focus -------------------------------------------

    /// C get_last_child: the most recent *mapped* child.
    fn last_mapped_child(
        &self,
        host: &dyn ShellHost,
        id: DesktopSurfaceId,
    ) -> Option<DesktopSurfaceId> {
        let st = self.surfaces.get(&id)?;
        st.children
            .iter()
            .rev()
            .copied()
            .find(|c| host.is_view_mapped(*c))
    }

    /// C activate() — child redirection, input activation, focus
    /// bookkeeping, restacking.
    fn activate(
        &mut self,
        host: &dyn ShellHost,
        surface: DesktopSurfaceId,
        seat: SeatId,
        flags: ActivateFlags,
        target: ActivateTarget,
    ) {
        if !self.surfaces.contains_key(&surface) {
            return;
        }
        if let Some(child) = self.last_mapped_child(host, surface) {
            // Activate the last xdg child instead of the parent.
            return self.activate(host, child, seat, flags, ActivateTarget::SurfaceView(child));
        }

        let focused_es = host.activate_input(target, seat, flags);

        let prev = self.seats.entry(seat).or_default().focused_surface;
        if let Some(p) = prev
            && p != surface
            && self.surfaces.contains_key(&p)
        {
            self.change_focus_count(host, p, -1);
        }
        if prev != Some(surface) {
            self.change_focus_count(host, surface, 1);
            if let Some(st) = self.seats.get_mut(&seat) {
                st.focused_surface = Some(surface);
            }
        }

        // C ensure_focus_state + focus_state_set_focus.
        if let Some(es) = focused_es {
            let old = self.focus.insert(seat, es);
            if let Some(old) = old
                && old != es
            {
                host.untrack_surface(old);
            }
        }

        host.raise_to_workspace_top(surface);
    }

    fn change_focus_count(&mut self, host: &dyn ShellHost, id: DesktopSurfaceId, delta: i32) {
        let crossed = {
            let Some(st) = self.surfaces.get_mut(&id) else {
                return;
            };
            let before = st.focus_count;
            st.focus_count += delta;
            (before == 0) != (st.focus_count == 0)
        };
        if crossed {
            self.sync_activated(host, id);
        }
    }

    /// C sync_surface_activated_state: climb to the root parent, then
    /// set_activated from "any transitive child focused".  (The C
    /// recursion has undefined behavior at depth ≥ 2 — the `bool **`
    /// bug; this implements the evident intent, plan §10.3.)
    fn sync_activated(&mut self, host: &dyn ShellHost, id: DesktopSurfaceId) {
        let mut root = id;
        let mut hops = 0;
        while let Some(p) = self.surfaces.get(&root).and_then(|st| st.parent) {
            root = p;
            hops += 1;
            if hops > 64 {
                break; // cycle guard; parents are client-controlled
            }
        }
        let active = self.has_focused_descendant(root, 0);
        host.set_activated(root, active);
    }

    fn has_focused_descendant(&self, id: DesktopSurfaceId, depth: u32) -> bool {
        if depth > 64 {
            return false;
        }
        let Some(st) = self.surfaces.get(&id) else {
            return false;
        };
        if st.focus_count > 0 {
            return true;
        }
        st.children
            .iter()
            .any(|c| self.has_focused_descendant(*c, depth + 1))
    }

    /// C focus_state_surface_destroy: hunt a replacement focus.
    fn focus_surface_gone(
        &mut self,
        host: &dyn ShellHost,
        gone: SurfaceId,
        main: Option<DesktopSurfaceId>,
    ) {
        let seats: Vec<SeatId> = self
            .focus
            .iter()
            .filter(|(_, es)| **es == gone)
            .map(|(s, _)| *s)
            .collect();
        for seat in seats {
            // "if the focus was a sub-surface, activate its main
            // surface"; otherwise the topmost remaining shell surface.
            let next = main.filter(|m| self.surfaces.contains_key(m)).or_else(|| {
                host.workspace_views_top_down()
                    .into_iter()
                    .find(|v| self.surfaces.contains_key(v))
            });
            self.focus.remove(&seat);
            if let Some(next) = next {
                self.activate(
                    host,
                    next,
                    seat,
                    ActivateFlags::CONFIGURE,
                    ActivateTarget::SurfaceView(next),
                );
            }
        }
    }

    // -- output changes ------------------------------------------------

    /// C shell_output_changed_move_layer →
    /// shell_reposition_view_on_output_change for every shell surface.
    fn reposition_all(&mut self, host: &dyn ShellHost) {
        let outputs: Vec<OutputId> = host.outputs();
        if outputs.is_empty() {
            return;
        }
        let infos: Vec<weston::Rect> = outputs
            .iter()
            .filter_map(|o| host.output_info(*o).map(|i| i.geometry))
            .collect();
        let first = infos.first().copied();

        let ids: Vec<DesktopSurfaceId> = self.surfaces.keys().copied().collect();
        for id in ids {
            let Some(pos) = host.view_pos(id) else {
                continue;
            };
            let visible = infos.iter().any(|g| contains(g, pos));
            if visible {
                host.mark_view_dirty(id);
            } else if let Some(f) = first {
                host.set_view_position(id, (f.x + f.width / 4) as f64, (f.y + f.height / 4) as f64);
            }
        }
    }
}

fn contains(g: &weston::Rect, pos: (f64, f64)) -> bool {
    pos.0 >= g.x as f64
        && pos.0 < (g.x + g.width) as f64
        && pos.1 >= g.y as f64
        && pos.1 < (g.y + g.height) as f64
}

/// Glue: `weston`'s dispatcher hands us concrete `&Ctx`; the logic only
/// sees `&dyn ShellHost`.
impl weston::ShellApp for Shell {
    fn handle(&mut self, host: &weston::Ctx, event: Event) {
        self.on_event(host, event);
    }
}

#[cfg(test)]
mod tests;
