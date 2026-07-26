# Callback inventory (R0 deliverable — plan §3e)

The audit surface for the fence: every place C calls into our code, one
row each, with its dispatch tier and (for sync-tier rows that borrow app
state) the required non-reentrancy proof (amendment A3).  A reviewer
checks the "no unsoundness outside `weston`" claim against this table,
not against 9.7k lines.

Row counts reconcile with the plan §1 figures (counted at trim T9):
**29** `wl_listener` fields (one of them a `main()` stack local, L7),
**23** signal/destroy-listener attach sites for those fields (the 24th
`wl_signal_add`-family call in the tree is
`weston_compositor_add_destroy_listener_once` at `shell.c:2184`, counted
under the compositor-destroy row; the two
`weston_compositor_add_screenshot_authority` calls attach L5/L10 outside
that count), **5** binding registrations with our own handlers +
`weston_install_debug_key_binding` (handler is libweston's) = 6
registrations total, and the `weston_desktop_api` vtable (14 struct
entries; the C shell installs 10, the 4 unfilled ones have "not
installed" rows).

Tier legend — **sync**: handled inside the trampoline (only
wrapper-held or callback-local data unless a proof is recorded);
**deferred**: trampoline captures payload eagerly, enqueues, drained at
depth zero.  *Sync+inval / deferred-policy*: destroy notifications split
per §3e — registry invalidation runs synchronously, the policy reaction
is a deferred event.

Port status legend: `—` not yet ported · `R0` implemented in the R0
foundation · statuses advance to `R1`/`R2x` as phases land.

**R1 status note (2026-07-25):** every shell-side row is live — the
desktop-shell listener set (L11–L28) is implemented in
`crates/weston/src/shell_init.rs` (seat/output/session/transform/
compositor-destroy wiring), `grab.rs` (all three pointer grabs + touch
move, L14/L15 deleted per §3c as designed), `desktop.rs` (the full
vtable), and `input_bindings.rs` (B3–B6); the shell policy behind them
is `crates/westonite-shell`.  Frontend rows (L1–L10, B1/B2) remain C
until R2.  The Status column below is updated per row group rather than
per cell; `git log` on the named modules is the per-row audit trail.

## 1. `wl_listener` fields and their attach sites

### frontend/main.c (7 fields)

| # | Field (owner struct) | Handler | Attach site | Signal | Tier | Notes / hazards | Status |
|---|---|---|---|---|---|---|---|
| L1 | `wet_head_tracker.head_destroy_listener` | `handle_head_destroy` (1918) | `main.c:1972` | head destroy | sync+inval / deferred-policy | destroys the output when its last head goes; output destroy cascades while head is dying (staleness shape 1) | R0 (wrapper head registry) |
| L2 | `wet_head_tracker.resized_listener` | `simple_heads_output_sharing_resize` (2581) | `main.c:2642` | `output_resized_signal` | deferred | mirror-of resize propagation (R2b) | — |
| L3 | `wet_output.output_destroy_listener` | `wet_output_handle_destroy` (2509) | `main.c:2668` | output destroy | sync+inval / deferred-policy | listener identity used as lookup key in C (`wet_output_from_weston_output`, 2674) — becomes a registry lookup (§3c) | R0 (wrapper output registry) |
| L4 | `wet_backend.heads_changed_listener` | `simple_heads_changed` (2099) / `drm_heads_changed` (3006) | `main.c:3358` | `heads_changed_signal` | **sync** (proof: emitted only from `weston_compositor_flush_heads_changed`, called by us or from backend event sources — never from an outbound call made while the app borrow is held; R0 handler touches wrapper state only) | must create+enable outputs inside the flush (§3e list) | R0 (canned configurator) |
| L5 | `wet_compositor.screenshot_auth` | `screenshot_allow_all` (4396) | via `weston_compositor_add_screenshot_authority` (4628) | auth request | sync (out-param decision; answers from config value) | — | — |
| L6 | `wet_compositor.output_created_listener` | `wet_output_handle_create` (2604) | `main.c:4245` | `output_created_signal` | deferred | remote/mirror output setup (R2c) | — |
| L7 | local in `main()` (main.c:4443), `primary_client_destroyed` | `handle_primary_client_destroyed` (926) | `main.c:4704` | client destroy | deferred | shutdown-on-client-exit policy | — |

### frontend/weston-screenshooter.c (3 fields)

| # | Field | Handler | Attach site | Signal | Tier | Notes | Status |
|---|---|---|---|---|---|---|---|
| L8 | `screenshooter.client_destroy_listener` | `screenshooter_client_destroy` (46) | `w-s.c:79` | client destroy | deferred | resets recorder client slot | — |
| L9 | `screenshooter.compositor_destroy_listener` | `screenshooter_destroy` (119) | `w-s.c:148` | compositor `destroy_signal` | sync+inval | teardown ordering only | — |
| L10 | `screenshooter.authorization` | `authorize_screenshooter` (108) | via auth API | auth request | sync (out-param) | — | — |

### desktop-shell/shell.c + shell.h (19 fields — all rows R1 unless marked vestigial/deleted)

| # | Field (owner) | Handler | Attach site | Signal | Tier | Notes / hazards | Status |
|---|---|---|---|---|---|---|---|
| L11 | `focus_state.seat_destroy_listener` | `focus_state_seat_destroy` (352) | `shell.c:422` | seat destroy | sync+inval / deferred-policy | staleness shape 1 | — |
| L12 | `focus_state.surface_destroy_listener` | `focus_state_surface_destroy` (363) | `shell.c:456` | surface destroy | sync+inval / deferred-policy | hunts replacement focus from inside a destroy handler — the canonical §3b hazard; policy must run deferred | — |
| L13 | `shell_surface.output_destroy_listener` | `notify_output_destroy` (1059) | `shell.c:1094` | output destroy | sync+inval / deferred-policy | weak backref nulling in C → stale `OutputId` in Rust | — |
| L14 | `shell_grab.shsurf_destroy_listener` | `destroy_shell_grab_shsurf` (178) | `shell.c:243` | shsurf destroy (shell-internal signal) | n/a — **deleted**: intra-Rust notification never routes through `wl_signal` (§3c); becomes registry invalidation + grab-local `SurfaceId` going stale | grabbed-surface-dies-mid-grab | — |
| L15 | `shell_touch_grab.shsurf_destroy_listener` | same (178) | `shell.c:301` | same | same as L14 | — | — |
| L16 | `shell_seat.seat_destroy_listener` | `destroy_shell_seat` (1119) | `shell.c:1161` | seat destroy | sync+inval / deferred-policy | — | — |
| L17 | `shell_seat.caps_changed_listener` | `shell_seat_caps_changed` (1129) | `shell.c:1170` | `updated_caps_signal` | **sync** (proof: emitted from seat capability updates during input-device dispatch, never from shell outbound calls; handler only re-attaches L18/L19 — wrapper state) | dynamic attach/detach of the focus listeners (the `wl_list_empty` idiom §3c) | — |
| L18 | `shell_seat.pointer_focus_listener` | `handle_pointer_focus` (928) | `shell.c:1139` (from caps_changed) | pointer `focus_signal` | deferred | — | — |
| L19 | `shell_seat.keyboard_focus_listener` | *(none — vestigial)* | never attached (only `wl_list_init`, shell.c:1164) | — | n/a | vestigial after the T-series trims: no handler exists; not ported (see `shell_init.rs` header note) | n/a |
| L20 | `workspace.seat_destroyed_listener` | `seat_destroyed` (477, notify set at 500) | never attached (only `wl_list_init`, shell.c:499/2207) | — | n/a | dead code in the trimmed tree; not ported as a listener | n/a |
| L21 | `shell_output.destroy_listener` | `handle_output_destroy` (1902) | `shell.c:1981` | output destroy | sync+inval / deferred-policy | curtain teardown (kind-2 ownership) | — |
| L22 | `desktop_shell.transform_listener` | `transform_handler` (1730) | `shell.c:2192` | `transform_signal` | sync (proof: emitted during commit processing of xwayland surfaces only; handler reads xwayland API + surface state, no app-borrow needed beyond the subject) | xwayland send_position | — |
| L23 | `desktop_shell.resized_listener` | `handle_output_resized` (1915) | `shell.c:2228` | `output_resized_signal` | deferred | background curtain resize | — |
| L24 | `desktop_shell.destroy_listener` | `shell_destroy` (2087) | `shell.c:2184` (`weston_compositor_add_destroy_listener_once`) | compositor destroy | **sync** (teardown; proof: emitted from `weston_compositor_destroy`, which the frontend calls at exit outside any app borrow) | full teardown; queue discarded after (§3e) | — |
| L25 | `desktop_shell.session_listener` | `desktop_shell_notify_session` (2133) | `shell.c:2231` | `session_signal` | deferred | VT switch / lock policy | — |
| L26 | `desktop_shell.pointer_focus_listener` | *(none — vestigial)* | never attached (field declared shell.h:62, unreferenced in shell.c) | — | n/a | dead field; busy-cursor policy is driven by `ping_timeout` + the per-seat L18 path; not ported | n/a |
| L27 | `desktop_shell.seat_create_listener` | `handle_seat_created` (2162) | `shell.c:2225` | `seat_created_signal` | deferred | registers shell_seat (registry insert on the Rust side is sync-in-trampoline, policy deferred) | — |
| L28 | `desktop_shell.output_create_listener` / `output_move_listener` | `handle_output_create` (1991) / `handle_output_move` (2020) | `shell.c:2040/2044` | `output_created/moved_signal` | deferred | background curtain creation / view move | — |

*(L28 folds two sibling fields into one row for brevity; both are
plain deferred notifications.  Field count: 7 + 3 + 19 = 29.)*

## 2. Bindings (6 registrations — B3–B6 are R1; B1/B2 remain C until R2)

| # | Binding | Handler | Site | Tier | Status |
|---|---|---|---|---|---|
| B1 | key Super+S | `screenshooter_binding` | `w-s.c:142` | deferred (spawns client) | — |
| B2 | key Super+R | `recorder_binding` | `w-s.c:144` | deferred | — |
| B3 | button BTN_LEFT | `click_to_activate_binding` | `shell.c:2120` | **sync** (reads pointer button/serial state valid only in the input frame; activation policy enqueued) | — |
| B4 | button BTN_RIGHT | `click_to_activate_binding` | `shell.c:2123` | sync (as B3) | — |
| B5 | touch | `touch_to_activate_binding` | `shell.c:2126` | sync (as B3) | — |
| B6 | debug key (Super) | libweston's own handler | `shell.c:2129` (`weston_install_debug_key_binding`) | n/a (no Rust callback) | — |

## 3. `weston_desktop_api` (14 struct entries; 10 installed at `shell.c:1607-1619` — all installed rows are R1, in `crates/weston/src/desktop.rs`)

| Entry | Installed | Tier | Notes | Status |
|---|---|---|---|---|
| `struct_size` | (size field) | — | set from the *bound* 14.0.1 headers (risk R-C) | — |
| `ping_timeout` | yes | deferred | unresponsive flag + busy cursor | — |
| `pong` | yes | deferred | — | — |
| `surface_added` | yes | **sync** (§3e closed list: brackets the C object's life; registry insert + user-data id write must happen inside the call) | — | — |
| `surface_removed` | yes | **sync policy + sync inval** (the half-dead window, §3a: `SurfaceRemoved` is dispatched sync while the id still resolves — A3: no app borrow can be live inside a desktop-api request — then state teardown and id invalidation follow in the same call) | — | — |
| `committed` | yes | **sync** (must map the surface inside commit processing) | — | — |
| `show_window_menu` | no — "not installed" | — | — | — |
| `set_parent` | yes | **sync** (deferring would reorder against stacking updates from the same request; A3: desktop-api request context, no app borrow live) | children fixup on death is shape-1 staleness | — |
| `move` | yes | **sync** (reads pointer serial/button inside the request frame; starts grab) | — | — |
| `resize` | yes | sync (as move) | — | — |
| `fullscreen_requested` | no — "not installed" | — | — | — |
| `maximized_requested` | no — "not installed" | — | — | — |
| `minimized_requested` | no — "not installed" | — | — | — |
| `set_xwayland_position` | yes | sync (writes position used by the following commit) | — | — |
| `get_position` | yes | **sync, out-params, NO app borrow** (A3: answers from wrapper-held geometry only) | `shell.c:1597-1605` | — |

## 4. Grabs (vtables, not listeners)

| Vtable | Entries | Tier | Status |
|---|---|---|---|
| pointer grab (`shell_grab`) | focus/motion/button/axis/axis_source/frame/cancel | sync, **no app borrow** (state lives in the grab box; policy events deferred; free deferred to depth-zero drain) | R1 (move/resize/busy live) |
| touch grab (`shell_touch_grab`) | down/up/motion/frame/cancel | same | R1 |
| busy-cursor grab | pointer iface | same | R1 |

## 5. Log handlers (§3k)

| Handler | Via | Tier | Status |
|---|---|---|---|
| `vlog` / `vlog_continue` | shim `wsys_install_log_handlers` → `wsys_rust_log_sink` | sync, no app borrow (writes stderr; R2a adds scopes/flight recorder) | R0 |

Maintenance rule: growing the sync tier or adding a callback without a
row here fails review; the R1/R2 porting PRs update the Status column
as rows go live.
