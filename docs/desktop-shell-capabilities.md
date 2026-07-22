# desktop-shell capability inventory

Scope: `desktop-shell/shell.c` (2592 lines) and `desktop-shell/shell.h`
(109 lines), as of the post-trim state (trim series T1–T6, logged in
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
saved-position machinery are all gone.

Each numbered group below is a candidate unit for any future trimming.

---

## 1. Built-in background

One solid-color `weston_curtain` per output, drawn by the compositor
itself (no client involved):

- Created in `create_shell_output` (2316) via
  `shell_output_recreate_background` (2287), recreated on output resize
  (`handle_output_resized` 2263), destroyed on output destroy; label via
  `background_get_label` (2274).
- Color from `[shell] background-color` (0xAARRGGBB), read in
  `shell_configuration` (371), default `0xff002244`.
- Lives in the dedicated background layer; captures input (clicks on
  empty desktop go nowhere).

## 2. Core window management (libweston-desktop API, `shell_desktop_api` at 1940)

xdg-shell (and Xwayland) toplevel handling:

- **Surface lifecycle**: `desktop_surface_added/removed` (1524/1576),
  `desktop_surface_committed` (1657), `map` (1631), initial placement
  `weston_view_set_initial_position` (2109) using `get_output_work_area`
  (269, now always the full output).
- **Move/resize (client-initiated only)**: `desktop_surface_move` (1711)
  and `desktop_surface_resize` (1754) drive the pointer/touch/tablet
  grab machinery — `surface_move` (762), `surface_touch_move` (653),
  `surface_tablet_tool_move` (906), `surface_resize` (1043), and the
  `shell_grab_start` helpers (251/309/340) — in response to xdg-shell
  requests such as titlebar drags. Resize honors edges and min/max size.
- **Minimize**: `desktop_surface_minimized_requested` (1802),
  `set_minimized` (1338), `minimized_layer`. The only surviving
  state-change request; advertised in `wm_capabilities`.
- **Transient/parent surfaces**: `desktop_surface_set_parent` (1781),
  child layer syncing `shell_surface_update_child_surface_layers` (179).
- **Xwayland integration**: `desktop_surface_set_xwayland_position`
  (1919), `set_position_from_xwayland` (1608), and `transform_handler`
  (2078) pushing view positions back to X windows via
  `weston_xwayland_surface_api` (why `shell.h` includes
  `<libweston/xwayland-api.h>`).
- **Unresponsive-client handling**: `desktop_surface_ping_timeout`/`pong`
  (1874/1906), busy-cursor pointer grab (`busy_cursor_grab_focus` 1081,
  `set_busy_cursor` 1813, `end_busy_cursor` 1831,
  `desktop_surface_set_unresponsive` 1863). Purely behavioral — no
  special cursor sprite is shown; left-click during the grab moves the
  unresponsive window.
- **Activation & focus**: `activate` (1969),
  click/touch/tablet-to-activate bindings (2034/2049/2063),
  per-workspace focus-state bookkeeping (`focus_state_create` 453,
  `ensure_focus_state` 478, `focus_state_set_focus` 494,
  `surface_keyboard_focus_lost` 568), pointer/tablet focus listeners
  (1143/1223), `shell_surface_update_layer` (1282).
- **Stacking/layers** (`shell.h`): cursor (libweston), workspace
  (single: `workspace_create` 549, `get_current_workspace` 562),
  minimized, background; iteration helper `shell_for_each_layer` (2216).

## 3. Input bindings (`shell_add_bindings`, 2467)

Only the activation bindings and the debug-key chain remain: pointer
click (left/right), touch tap, and tablet-tool tap raise-and-focus the
target window, and Super+Shift+Space starts libweston's debug-key chain
(`weston_install_debug_key_binding`, hardcoded Super since
`binding-modifier` is gone). There are no other hotkeys. Window state
changes happen only through client-side requests (xdg-shell) and
pointer/touch activation.

Config read in `shell_configuration` (371): `background-color` — the
complete `[shell]` option set.

## 4. Multi-output / hotplug / session handling

- Per-output state `shell_output` (`create_shell_output` 2316) carrying
  the background curtain.
- Output hotplug: `handle_output_create/destroy/move` (2339/2250/2368),
  layer view migration, surface repositioning when an output disappears
  (`shell_reposition_view_on_output_change` 2171), background-curtain
  resize (`handle_output_resized` 2263 — windows are no longer re-fitted
  since maximize/fullscreen are gone), destroy wiring
  `setup_output_destroy_handler` (2379).
- VT/session switching: `desktop_shell_notify_session` (2485) — on
  session re-activation re-syncs surface activation state.

## 5. Seat management

`shell_seat` per `weston_seat` (`create_shell_seat` 1439,
`get_shell_seat` 1491, caps-changed 1420, tablet-tool tracking
`handle_tablet_tool_added` 1390, `handle_seat_created` 2514, destruction
`desktop_shell_destroy_seat` 1365, `seat_destroyed` 535) — carries the
focus listeners used by activation.

---

## Observations for any future trimming

- **Cheap, self-contained removals**: minimize (NULL its callback like
  T6 did for maximize/fullscreen — capability un-advertises the same
  way); the busy-cursor grab (ping/pong would still detect
  unresponsiveness); the touch and tablet-tool move grab machinery, if
  client-initiated touch/tablet moves are not needed.
- **Entangled**: focus-state tracking underpins activation; Xwayland
  positioning spans `shell.h`'s API surface, `transform_handler`, and
  frontend `xwayland.c`.
- Session-notify (§4) matters only for VT switching on DRM seats;
  headless/nested use never exercises it.
