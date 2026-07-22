# desktop-shell capability inventory

Scope: `desktop-shell/shell.c` (2239 lines) and `desktop-shell/shell.h`
(108 lines), as of the post-trim state (trim series T1–T9, logged in
`VENDOR.md`). Line numbers refer to this state.

Westonite's desktop-shell is a **pure window-manager plugin**: it serves
no client-facing shell protocol of its own, spawns no helper clients,
draws a built-in solid-color background, and has no hotkeys beyond
libweston's debug-key chain. Features upstream weston's desktop-shell has
that this one deliberately does **not**: helper-client protocol (panel,
wallpaper images, grab cursors), lock screen, idle/DPMS handling
(displays never sleep), all animations, input-panel / on-screen-keyboard
support (including the frontend text-backend), multiple workspaces
(already single-workspace upstream in v14), every hotkey binding except
the debug-key chain (T5), and — after T6 — **fullscreen and maximize**:
both `weston_desktop_api` callbacks are absent, so libweston-desktop
omits them from `xdg_toplevel.wm_capabilities` (v5-aware toolkits hide
the buttons) and silently ignores requests from clients that send them
anyway. Windows are free-floating and client-sized only; the black
letterbox curtains, fullscreen layer, and maximize/fullscreen
saved-position machinery are all gone. T7 removed the tablet-pen
machinery: pens are in-app input only (full tablet-v2 delivery via
libweston is unaffected) and cannot focus or drag windows; touch
retains both tap-to-activate and window dragging. T9 removed minimize —
`wm_capabilities` now advertises **no** window-state requests at all;
clicks on unresponsive windows only activate them (T8).

Each numbered group below is a candidate unit for any future trimming.

---

## 1. Built-in background

One solid-color `weston_curtain` per output, drawn by the compositor
itself (no client involved):

- Created in `create_shell_output` (1968) via
  `shell_output_recreate_background` (1939), recreated on output resize
  (`handle_output_resized` 1915), destroyed on output destroy; label via
  `background_get_label` (1926).
- Color from `[shell] background-color` (0xAARRGGBB), read in
  `shell_configuration` (323), default `0xff002244`.
- Lives in the dedicated background layer; captures input (clicks on
  empty desktop go nowhere).

## 2. Core window management (libweston-desktop API, `shell_desktop_api` at 1607)

xdg-shell (and Xwayland) toplevel handling:

- **Surface lifecycle**: `desktop_surface_added/removed` (1268/1320),
  `desktop_surface_committed` (1346), `map` (1320), initial placement
  `weston_view_set_initial_position` (1761) using `get_output_work_area`
  (251, now always the full output).
- **Move/resize (client-initiated only)**: `desktop_surface_move` and
  `desktop_surface_resize` drive the pointer and touch grab machinery
  (`surface_move`, `surface_touch_move`, `surface_resize`, and the
  `shell_grab_start`/`shell_touch_grab_start` helpers) in response to
  xdg-shell requests such as titlebar drags. Resize is pointer-only
  (upstream never implemented touch resize); pen move grabs were
  removed in T7.
- **Transient/parent surfaces**: `desktop_surface_set_parent` (1458),
  child layer syncing `shell_surface_update_child_surface_layers` (161).
- **Xwayland integration**: `desktop_surface_set_xwayland_position`
  (1586), `set_position_from_xwayland` (1297), and `transform_handler`
  (1730) pushing view positions back to X windows via
  `weston_xwayland_surface_api` (why `shell.h` includes
  `<libweston/xwayland-api.h>`).
- **Unresponsive-client handling**: `desktop_surface_ping_timeout`/`pong`
  (1541/1573), busy-cursor pointer grab (`busy_cursor_grab_focus` 868,
  `set_busy_cursor` 1480, `end_busy_cursor` 1498,
  `desktop_surface_set_unresponsive` 1530). Purely behavioral — no
  special cursor sprite is shown; left-click during the grab only
  activates the unresponsive window (move-on-click was removed in T8).
- **Activation & focus**: `activate` (1635),
  click/touch-to-activate bindings (1700/1715),
  per-workspace focus-state bookkeeping (`focus_state_create` 405,
  `ensure_focus_state` 430, `focus_state_set_focus` 446,
), the pointer focus listener
  (`handle_pointer_focus` 928), `shell_surface_update_layer` (1047).
- **Stacking/layers** (`shell.h`): cursor (libweston), workspace
  (single: `workspace_create` 491, `get_current_workspace` 504), and
  background; iteration helper `shell_for_each_layer` (1868).

## 3. Input bindings (`shell_add_bindings`, 2185)

Only the activation bindings and the debug-key chain remain: pointer
click (left/right) and touch tap raise-and-focus the target window, and
Super+Shift+Space starts libweston's debug-key chain
(`weston_install_debug_key_binding`, hardcoded Super since
`binding-modifier` is gone). There are no other hotkeys. Window state
changes happen only through client-side requests (xdg-shell) and
pointer/touch activation.

Config read in `shell_configuration` (323): `background-color` — the
complete `[shell]` option set.

## 4. Multi-output / hotplug / session handling

- Per-output state `shell_output` (`create_shell_output` 1968) carrying
  the background curtain.
- Output hotplug: `handle_output_create/destroy/move` (2057/1968/2086),
  layer view migration, surface repositioning when an output disappears
  (`shell_reposition_view_on_output_change` 1823), background-curtain
  resize (`handle_output_resized` 1915 — windows are no longer re-fitted
  since maximize/fullscreen are gone), destroy wiring
  `setup_output_destroy_handler` (2031).
- VT/session switching: `desktop_shell_notify_session` (2133) — on
  session re-activation re-syncs surface activation state.

## 5. Seat management

`shell_seat` per `weston_seat` (`create_shell_seat`, `get_shell_seat`,
caps-changed handling, `handle_seat_created`, destruction via
`desktop_shell_destroy_seat` and `seat_destroyed`) — carries the focus
listeners used by activation.

---

## Observations for any future trimming

- **Cheap, self-contained removals**: the busy-cursor grab (ping/pong
  would still detect unresponsiveness); the touch move grab, if finger
  window-dragging is ever deemed unnecessary (tap-to-activate is
  separate and would stay).
- **Entangled**: focus-state tracking underpins activation; Xwayland
  positioning spans `shell.h`'s API surface, `transform_handler`, and
  frontend `xwayland.c`.
- Session-notify (§4) matters only for VT switching on DRM seats;
  headless/nested use never exercises it.
