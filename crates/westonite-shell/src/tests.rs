//! Mock-`ShellHost` unit tests (D20): the focus-churn and
//! teardown-ordering cases §3b flags as the risky ones, plus child
//! activation — all without a compositor.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use weston::ids::forge;
use weston::{
    ActivateFlags, ActivateTarget, ActivateVia, DesktopSurfaceId, Event, OutputId, OutputInfo,
    Rect, ResizeEdges, SeatId, ShellHost, SurfaceId,
};

use crate::{Shell, ShellConfig};

#[derive(Default)]
struct MockHost {
    outputs: Vec<(OutputId, Rect)>,
    seats: Vec<SeatId>,
    mapped: RefCell<HashSet<DesktopSurfaceId>>,
    view_pos: RefCell<HashMap<DesktopSurfaceId, (f64, f64)>>,
    // command journal the tests assert on
    activated: RefCell<Vec<(DesktopSurfaceId, bool)>>,
    input_activations: RefCell<Vec<(DesktopSurfaceId, SeatId)>>,
    raised: RefCell<Vec<DesktopSurfaceId>>,
    destroyed: RefCell<Vec<DesktopSurfaceId>>,
    untracked: RefCell<Vec<SurfaceId>>,
    tracked_for: RefCell<HashMap<DesktopSurfaceId, SurfaceId>>,
    /// Emulates the wrapper's ref-count contract: activate_input
    /// tracks (+1), untrack_surface drops (-1); the destroy listener
    /// lives while the count is positive.
    track_counts: RefCell<HashMap<SurfaceId, i32>>,
    stacking: RefCell<Vec<DesktopSurfaceId>>, // top first
}

impl MockHost {
    fn track_count(&self, s: SurfaceId) -> i32 {
        self.track_counts.borrow().get(&s).copied().unwrap_or(0)
    }
}

impl ShellHost for MockHost {
    fn outputs(&self) -> Vec<OutputId> {
        self.outputs.iter().map(|(o, _)| *o).collect()
    }
    fn output_info(&self, id: OutputId) -> Option<OutputInfo> {
        self.outputs
            .iter()
            .find(|(o, _)| *o == id)
            .map(|(_, g)| OutputInfo {
                name: "mock".into(),
                geometry: *g,
            })
    }
    fn seats(&self) -> Vec<SeatId> {
        self.seats.clone()
    }
    fn pointer_pos(&self, _seat: SeatId) -> Option<(f64, f64)> {
        None
    }
    fn pointer_focus(&self, _seat: SeatId) -> Option<DesktopSurfaceId> {
        None
    }
    fn workspace_views_top_down(&self) -> Vec<DesktopSurfaceId> {
        self.stacking.borrow().clone()
    }
    fn is_view_mapped(&self, id: DesktopSurfaceId) -> bool {
        self.mapped.borrow().contains(&id)
    }
    fn view_pos(&self, id: DesktopSurfaceId) -> Option<(f64, f64)> {
        self.view_pos.borrow().get(&id).copied()
    }
    fn view_output(&self, _id: DesktopSurfaceId) -> Option<OutputId> {
        None
    }
    fn surface_geometry(&self, _id: DesktopSurfaceId) -> Option<Rect> {
        Some(Rect::default())
    }
    fn activate_input(
        &self,
        target: ActivateTarget,
        seat: SeatId,
        _flags: ActivateFlags,
    ) -> Option<SurfaceId> {
        let ActivateTarget::SurfaceView(id) = target else {
            return None;
        };
        self.input_activations.borrow_mut().push((id, seat));
        let es = self.tracked_for.borrow().get(&id).copied();
        if let Some(es) = es {
            *self.track_counts.borrow_mut().entry(es).or_insert(0) += 1;
        }
        es
    }
    fn untrack_surface(&self, id: SurfaceId) {
        self.untracked.borrow_mut().push(id);
        *self.track_counts.borrow_mut().entry(id).or_insert(0) -= 1;
    }
    fn set_activated(&self, id: DesktopSurfaceId, active: bool) {
        self.activated.borrow_mut().push((id, active));
    }
    fn raise_to_workspace_top(&self, id: DesktopSurfaceId) {
        self.raised.borrow_mut().push(id);
        let mut s = self.stacking.borrow_mut();
        s.retain(|v| *v != id);
        s.insert(0, id);
    }
    fn map_surface(&self, id: DesktopSurfaceId) {
        self.mapped.borrow_mut().insert(id);
    }
    fn set_view_position(&self, id: DesktopSurfaceId, x: f64, y: f64) {
        self.view_pos.borrow_mut().insert(id, (x, y));
    }
    fn set_view_position_with_offset(
        &self,
        id: DesktopSurfaceId,
        x: f64,
        y: f64,
        ox: f64,
        oy: f64,
    ) {
        self.view_pos.borrow_mut().insert(id, (x + ox, y + oy));
    }
    fn update_transforms(&self, _id: DesktopSurfaceId) {}
    fn mark_view_dirty(&self, _id: DesktopSurfaceId) {}
    fn destroy_surface_state(&self, id: DesktopSurfaceId) {
        self.destroyed.borrow_mut().push(id);
        self.mapped.borrow_mut().remove(&id);
        self.stacking.borrow_mut().retain(|v| *v != id);
    }
    fn set_busy_cursor(&self, _id: DesktopSurfaceId, _seat: SeatId) {}
    fn end_busy_grabs_for_client_of(&self, _id: DesktopSurfaceId) {}
    fn recreate_background(&self, _output: OutputId, _argb: u32) {}
}

fn commit(shell: &mut Shell, host: &MockHost, s: DesktopSurfaceId, mapped: bool) {
    shell.on_event(
        host,
        Event::Committed {
            surface: s,
            buf_dx: 0.0,
            buf_dy: 0.0,
            resize_edges: ResizeEdges::empty(),
            width: 100,
            height: 100,
            mapped,
        },
    );
}

#[test]
fn map_activates_on_every_seat_and_raises() {
    let host = MockHost {
        outputs: vec![(
            forge::output(0),
            Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
        )],
        seats: vec![forge::seat(0), forge::seat(1)],
        ..Default::default()
    };
    host.tracked_for
        .borrow_mut()
        .insert(forge::ds(0), forge::surf(0));
    let mut shell = Shell::new(ShellConfig::default());
    shell.on_event(
        &host,
        Event::SeatCreated {
            seat: forge::seat(0),
        },
    );
    shell.on_event(
        &host,
        Event::SeatCreated {
            seat: forge::seat(1),
        },
    );
    shell.on_event(
        &host,
        Event::SurfaceAdded {
            surface: forge::ds(0),
        },
    );
    commit(&mut shell, &host, forge::ds(0), false);

    assert!(host.mapped.borrow().contains(&forge::ds(0)));
    assert_eq!(host.input_activations.borrow().len(), 2);
    // focus_count crossed 0→1 exactly once → one set_activated(true)
    assert_eq!(*host.activated.borrow(), vec![(forge::ds(0), true)]);
    assert!(host.raised.borrow().contains(&forge::ds(0)));
}

#[test]
fn activating_second_surface_deactivates_first() {
    let host = MockHost {
        seats: vec![forge::seat(0)],
        ..Default::default()
    };
    host.tracked_for
        .borrow_mut()
        .insert(forge::ds(0), forge::surf(0));
    host.tracked_for
        .borrow_mut()
        .insert(forge::ds(1), forge::surf(1));
    let mut shell = Shell::new(ShellConfig::default());
    shell.on_event(
        &host,
        Event::SeatCreated {
            seat: forge::seat(0),
        },
    );
    for i in 0..2 {
        shell.on_event(
            &host,
            Event::SurfaceAdded {
                surface: forge::ds(i),
            },
        );
        commit(&mut shell, &host, forge::ds(i), false);
    }
    // Mapping surface 1 activated it and deactivated surface 0.
    let log = host.activated.borrow().clone();
    assert_eq!(
        log,
        vec![
            (forge::ds(0), true),
            (forge::ds(0), false),
            (forge::ds(1), true)
        ]
    );
    // The old tracked focus surface was untracked on refocus.
    assert_eq!(*host.untracked.borrow(), vec![forge::surf(0)]);
}

#[test]
fn activation_redirects_to_last_mapped_child() {
    let host = MockHost {
        seats: vec![forge::seat(0)],
        ..Default::default()
    };
    host.tracked_for
        .borrow_mut()
        .insert(forge::ds(0), forge::surf(0));
    host.tracked_for
        .borrow_mut()
        .insert(forge::ds(1), forge::surf(1));
    let mut shell = Shell::new(ShellConfig::default());
    shell.on_event(
        &host,
        Event::SeatCreated {
            seat: forge::seat(0),
        },
    );
    shell.on_event(
        &host,
        Event::SurfaceAdded {
            surface: forge::ds(0),
        },
    );
    commit(&mut shell, &host, forge::ds(0), false);
    shell.on_event(
        &host,
        Event::SurfaceAdded {
            surface: forge::ds(1),
        },
    );
    shell.on_event(
        &host,
        Event::ParentSet {
            surface: forge::ds(1),
            parent: Some(forge::ds(0)),
        },
    );
    commit(&mut shell, &host, forge::ds(1), false);
    host.input_activations.borrow_mut().clear();

    // Clicking the parent must land on the mapped child.
    shell.on_event(
        &host,
        Event::Activate {
            seat: forge::seat(0),
            surface: forge::ds(0),
            via: ActivateVia::BusyClick,
        },
    );
    assert_eq!(
        *host.input_activations.borrow(),
        vec![(forge::ds(1), forge::seat(0))]
    );

    // Parent (root) stays "activated": a transitive child holds focus —
    // the intent the C bool** bug corrupted (plan §10.3).
    assert_eq!(
        host.activated.borrow().last().copied(),
        Some((forge::ds(0), true))
    );
}

#[test]
fn focus_replacement_hunt_on_surface_death() {
    let host = MockHost {
        seats: vec![forge::seat(0)],
        ..Default::default()
    };
    host.tracked_for
        .borrow_mut()
        .insert(forge::ds(0), forge::surf(0));
    host.tracked_for
        .borrow_mut()
        .insert(forge::ds(1), forge::surf(1));
    let mut shell = Shell::new(ShellConfig::default());
    shell.on_event(
        &host,
        Event::SeatCreated {
            seat: forge::seat(0),
        },
    );
    for i in 0..2 {
        shell.on_event(
            &host,
            Event::SurfaceAdded {
                surface: forge::ds(i),
            },
        );
        commit(&mut shell, &host, forge::ds(i), false);
    }
    // Focused: ds1 (mapped last).  Kill it: removal + tracked-gone.
    shell.on_event(
        &host,
        Event::SurfaceRemoved {
            surface: forge::ds(1),
        },
    );
    host.input_activations.borrow_mut().clear();
    shell.on_event(
        &host,
        Event::TrackedSurfaceGone {
            surface: forge::surf(1),
            main: None,
        },
    );
    // The hunt activated the topmost remaining shell surface.
    assert_eq!(
        *host.input_activations.borrow(),
        vec![(forge::ds(0), forge::seat(0))]
    );
    assert!(host.destroyed.borrow().contains(&forge::ds(1)));
}

#[test]
fn shutdown_destroys_every_surface_and_clears_state() {
    let host = MockHost {
        seats: vec![forge::seat(0)],
        ..Default::default()
    };
    let mut shell = Shell::new(ShellConfig::default());
    shell.on_event(
        &host,
        Event::SeatCreated {
            seat: forge::seat(0),
        },
    );
    // Three mapped surfaces (last mapped = top of the stacking order)
    // plus one never-mapped straggler.
    for i in 0..3 {
        shell.on_event(
            &host,
            Event::SurfaceAdded {
                surface: forge::ds(i),
            },
        );
        commit(&mut shell, &host, forge::ds(i), false);
    }
    shell.on_event(
        &host,
        Event::SurfaceAdded {
            surface: forge::ds(3),
        },
    );
    shell.on_event(&host, Event::Shutdown);
    // Deterministic teardown journal: stacking order first (top-down,
    // C's layer walk), then never-stacked leftovers in id order.
    assert_eq!(
        *host.destroyed.borrow(),
        vec![forge::ds(2), forge::ds(1), forge::ds(0), forge::ds(3)]
    );
    // Post-shutdown events are harmless no-ops.
    commit(&mut shell, &host, forge::ds(0), true);
}

#[test]
fn activation_redirects_through_grandchild_chain() {
    let host = MockHost {
        seats: vec![forge::seat(0)],
        ..Default::default()
    };
    for i in 0..3 {
        host.tracked_for
            .borrow_mut()
            .insert(forge::ds(i), forge::surf(i));
    }
    let mut shell = Shell::new(ShellConfig::default());
    shell.on_event(
        &host,
        Event::SeatCreated {
            seat: forge::seat(0),
        },
    );
    // Chain: ds0 <- ds1 <- ds2, all mapped.
    for i in 0..3 {
        shell.on_event(
            &host,
            Event::SurfaceAdded {
                surface: forge::ds(i),
            },
        );
        if i > 0 {
            shell.on_event(
                &host,
                Event::ParentSet {
                    surface: forge::ds(i),
                    parent: Some(forge::ds(i - 1)),
                },
            );
        }
        commit(&mut shell, &host, forge::ds(i), false);
    }
    host.input_activations.borrow_mut().clear();

    // Clicking the root must walk the chain to the deepest mapped
    // descendant — depth 2, where the C bool** recursion corrupted
    // memory (plan §10.3).
    shell.on_event(
        &host,
        Event::Activate {
            seat: forge::seat(0),
            surface: forge::ds(0),
            via: ActivateVia::BusyClick,
        },
    );
    assert_eq!(
        *host.input_activations.borrow(),
        vec![(forge::ds(2), forge::seat(0))]
    );
    // The root stays "activated": a depth-2 transitive child holds
    // keyboard focus.
    assert_eq!(
        host.activated.borrow().last().copied(),
        Some((forge::ds(0), true))
    );
}

#[test]
fn parent_cycle_activation_terminates() {
    let host = MockHost {
        seats: vec![forge::seat(0)],
        ..Default::default()
    };
    let mut shell = Shell::new(ShellConfig::default());
    shell.on_event(
        &host,
        Event::SeatCreated {
            seat: forge::seat(0),
        },
    );

    // Self-parent: the redirect walk must hit the hop cap, not hang.
    shell.on_event(
        &host,
        Event::SurfaceAdded {
            surface: forge::ds(0),
        },
    );
    commit(&mut shell, &host, forge::ds(0), false);
    shell.on_event(
        &host,
        Event::ParentSet {
            surface: forge::ds(0),
            parent: Some(forge::ds(0)),
        },
    );
    host.input_activations.borrow_mut().clear();
    shell.on_event(
        &host,
        Event::Activate {
            seat: forge::seat(0),
            surface: forge::ds(0),
            via: ActivateVia::BusyClick,
        },
    );
    assert_eq!(
        *host.input_activations.borrow(),
        vec![(forge::ds(0), forge::seat(0))]
    );

    // A <-> B parent cycle: also terminates, landing on a cycle member.
    for i in 1..3 {
        shell.on_event(
            &host,
            Event::SurfaceAdded {
                surface: forge::ds(i),
            },
        );
        commit(&mut shell, &host, forge::ds(i), false);
    }
    shell.on_event(
        &host,
        Event::ParentSet {
            surface: forge::ds(1),
            parent: Some(forge::ds(2)),
        },
    );
    shell.on_event(
        &host,
        Event::ParentSet {
            surface: forge::ds(2),
            parent: Some(forge::ds(1)),
        },
    );
    host.input_activations.borrow_mut().clear();
    shell.on_event(
        &host,
        Event::Activate {
            seat: forge::seat(0),
            surface: forge::ds(1),
            via: ActivateVia::BusyClick,
        },
    );
    let log = host.input_activations.borrow().clone();
    assert_eq!(log.len(), 1);
    let (landed, seat) = log[0];
    assert!(landed == forge::ds(1) || landed == forge::ds(2));
    assert_eq!(seat, forge::seat(0));
}

#[test]
fn multi_seat_shared_focus_keeps_other_seats_tracking() {
    let host = MockHost {
        seats: vec![forge::seat(0), forge::seat(1)],
        ..Default::default()
    };
    host.tracked_for
        .borrow_mut()
        .insert(forge::ds(0), forge::surf(0));
    host.tracked_for
        .borrow_mut()
        .insert(forge::ds(1), forge::surf(1));
    let mut shell = Shell::new(ShellConfig::default());
    shell.on_event(
        &host,
        Event::SeatCreated {
            seat: forge::seat(0),
        },
    );
    shell.on_event(
        &host,
        Event::SeatCreated {
            seat: forge::seat(1),
        },
    );
    // X maps and activates on both seats: two tracked references.
    shell.on_event(
        &host,
        Event::SurfaceAdded {
            surface: forge::ds(0),
        },
    );
    commit(&mut shell, &host, forge::ds(0), false);
    assert_eq!(host.track_count(forge::surf(0)), 2);

    // Seat 0 refocuses to Y: exactly one reference dropped; seat 1's
    // reference (and its death tracking) must survive.
    shell.on_event(
        &host,
        Event::SurfaceAdded {
            surface: forge::ds(1),
        },
    );
    shell.on_event(
        &host,
        Event::Activate {
            seat: forge::seat(0),
            surface: forge::ds(1),
            via: ActivateVia::BusyClick,
        },
    );
    let drops = |s: SurfaceId| host.untracked.borrow().iter().filter(|u| **u == s).count();
    assert_eq!(drops(forge::surf(0)), 1);
    assert_eq!(host.track_count(forge::surf(0)), 1);

    // X dies: seat 1 must still run the replacement hunt and land on Y.
    shell.on_event(
        &host,
        Event::SurfaceRemoved {
            surface: forge::ds(0),
        },
    );
    host.input_activations.borrow_mut().clear();
    shell.on_event(
        &host,
        Event::TrackedSurfaceGone {
            surface: forge::surf(0),
            main: None,
        },
    );
    assert_eq!(
        *host.input_activations.borrow(),
        vec![(forge::ds(1), forge::seat(1))]
    );
    // Seat 1's reference was never dropped by the shell (the death
    // itself retires it wrapper-side).
    assert_eq!(drops(forge::surf(0)), 1);
    // Both seats now hold Y: balanced at one reference per seat.
    assert_eq!(host.track_count(forge::surf(1)), 2);
}

#[test]
fn resize_edge_commit_offsets_match_c_signs() {
    let host = MockHost {
        seats: vec![forge::seat(0)],
        ..Default::default()
    };
    let mut shell = Shell::new(ShellConfig::default());
    shell.on_event(
        &host,
        Event::SeatCreated {
            seat: forge::seat(0),
        },
    );
    shell.on_event(
        &host,
        Event::SurfaceAdded {
            surface: forge::ds(0),
        },
    );
    commit(&mut shell, &host, forge::ds(0), false); // maps at 100x100
    // Park the view at a known position, then a plain commit records
    // last_width/last_height = 100x100 (the map path leaves them 0,
    // exactly like C).
    host.view_pos
        .borrow_mut()
        .insert(forge::ds(0), (500.0, 400.0));
    commit(&mut shell, &host, forge::ds(0), true);
    assert_eq!(
        host.view_pos.borrow().get(&forge::ds(0)).copied(),
        Some((500.0, 400.0))
    );

    // Shrink 100x100 -> 80x90 while grabbing the LEFT|TOP edges: the
    // view moves by (last - new) = (+20, +10) — C's offset signs.
    shell.on_event(
        &host,
        Event::Committed {
            surface: forge::ds(0),
            buf_dx: 0.0,
            buf_dy: 0.0,
            resize_edges: ResizeEdges::LEFT | ResizeEdges::TOP,
            width: 80,
            height: 90,
            mapped: true,
        },
    );
    assert_eq!(
        host.view_pos.borrow().get(&forge::ds(0)).copied(),
        Some((520.0, 410.0))
    );

    // RIGHT|BOTTOM edges: client buffer offsets are zeroed and no edge
    // offset applies — the anchored corner stays put.
    shell.on_event(
        &host,
        Event::Committed {
            surface: forge::ds(0),
            buf_dx: -5.0,
            buf_dy: -7.0,
            resize_edges: ResizeEdges::RIGHT | ResizeEdges::BOTTOM,
            width: 60,
            height: 70,
            mapped: true,
        },
    );
    assert_eq!(
        host.view_pos.borrow().get(&forge::ds(0)).copied(),
        Some((520.0, 410.0))
    );
}
