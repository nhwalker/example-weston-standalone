# desktop-shell capability inventory

Scope: `desktop-shell/shell.c` (3072 lines) and `desktop-shell/shell.h`
(113 lines), as of the post-trim state (trim series T1–T5, logged in
`VENDOR.md`). Line numbers refer to this state.

Westonite's desktop-shell is a **pure window-manager plugin**: it serves
no client-facing shell protocol of its own, spawns no helper clients,
draws a built-in solid-color background, and has no hotkeys beyond
libweston's debug-key chain. Features
upstream weston's desktop-shell has that this one deliberately does
**not**: helper-client protocol (panel, wallpaper images, grab cursors),
lock screen, idle/DPMS handling (displays never sleep), all animations,
input-panel / on-screen-keyboard support (including the frontend
text-backend), multiple workspaces (already single-workspace upstream in
v14), and — after T5 — every hotkey binding except the debug-key chain,
along with the window rotation, tiled-snap, switcher, and per-surface
opacity machinery and the `allow-zap` / `binding-modifier` options.

Each numbered group below is a candidate unit for any future trimming.

---

## 1. Built-in background

One solid-color `weston_curtain` per output, drawn by the compositor
itself (no client involved):

- Created in `create_shell_output` (2788) via
  `shell_output_recreate_background` (2759), recreated on output resize
  (`handle_output_resized` 2733), destroyed on output destroy; label via
  `background_get_label` (2746).
- Color from `[shell] background-color` (0xAARRGGBB), read in
  `shell_configuration` (430), default `0xff002244`.
- Lives in the dedicated background layer; captures input (clicks on
  empty desktop go nowhere).

## 2. Core window management (libweston-desktop API, `shell_desktop_api` at 2319)

xdg-shell (and Xwayland) toplevel handling:

- **Surface lifecycle**: `desktop_surface_added/removed` (1717/1771),
  `desktop_surface_committed` (1886), `map` (1848), initial placement
  `weston_view_set_initial_position` (2545), output assignment.
- **Move/resize (client-initiated only)**: `desktop_surface_move` (2042)
  and `desktop_surface_resize` (2085) drive the pointer/touch/tablet grab
  machinery — `surface_move` (823), `surface_touch_move` (712),
  `surface_tablet_tool_move` (967), `surface_resize` (1106), and the
  `shell_grab_start` helpers (310/368/399) — in response to xdg-shell
  requests such as titlebar drags. Resize honors edges and min/max size.
- **Fullscreen**: `desktop_surface_fullscreen_requested` (2133),
  `set_fullscreen` (1995), `shell_set_view_fullscreen` (1523) with black
  curtain views, dedicated fullscreen layer, `lower_fullscreen_layer`
  (2348), `unset_fullscreen` (1409).
- **Maximize**: `desktop_surface_maximized_requested` (2171),
  `set_maximized` (2144), `get_maximized_size` (1982),
  `set_maximized_position` (1808), `unset_maximized` (1424). Work area =
  full output (`get_output_work_area` 328).
- **Minimize**: `desktop_surface_minimized_requested` (2181),
  `set_minimized` (1442), `minimized_layer`.
- **Transient/parent surfaces**: `desktop_surface_set_parent` (2112),
  child layer syncing `shell_surface_update_child_surface_layers` (200).
- **Xwayland integration**: `desktop_surface_set_xwayland_position`
  (2298), `set_position_from_xwayland` (1825), and `transform_handler`
  (2514) pushing view positions back to X windows via
  `weston_xwayland_surface_api` (why `shell.h` includes
  `<libweston/xwayland-api.h>`).
- **Unresponsive-client handling**: `desktop_surface_ping_timeout`/`pong`
  (2253/2285), busy-cursor pointer grab (`busy_cursor_grab_focus` 1144,
  `set_busy_cursor` 2192, `end_busy_cursor` 2210,
  `desktop_surface_set_unresponsive` 2242). Purely behavioral — no
  special cursor sprite is shown; left-click during the grab moves the
  unresponsive window.
- **Activation & focus**: `activate` (2392),
  click/touch/tablet-to-activate bindings (2470/2485/2499),
  per-workspace focus-state bookkeeping (`focus_state_create` 512,
  `ensure_focus_state` 537, `focus_state_set_focus` 553,
  `surface_keyboard_focus_lost` 627), pointer/tablet focus listeners
  (1206/1286), `shell_surface_update_layer` (1351).
- **Stacking/layers** (`shell.h`): cursor (libweston), fullscreen,
  workspace (single: `workspace_create` 608, `get_current_workspace`
  621), minimized, background; iteration helper `shell_for_each_layer`
  (2661).

## 3. Input bindings (`shell_add_bindings`, 2944)

Only the activation bindings and the debug-key chain remain: pointer
click (left/right), touch tap, and tablet-tool tap raise-and-focus the
target window, and Super+Shift+Space starts libweston's debug-key chain
(`weston_install_debug_key_binding`, hardcoded Super since
`binding-modifier` is gone). There are no other hotkeys — no zap, no
switcher, no move/resize/maximize/fullscreen/snap shortcuts, no rotate,
no opacity, no backlight keys, no force-kill. Window state changes
happen only through client-side requests (xdg-shell) and pointer/touch
activation.

Config read in `shell_configuration` (430): `background-color` — the
complete `[shell]` option set.

## 4. Multi-output / hotplug / session handling

- Per-output state `shell_output` (`create_shell_output` 2788) carrying
  the background curtain.
- Output hotplug: `handle_output_create/destroy/move` (2811/2696/2840),
  layer view migration, surface repositioning when an output disappears
  (`shell_reposition_view_on_output_change` 2607), resize handling
  (`handle_output_resized` 2733, `handle_output_resized_shsurfs` 2705),
  destroy wiring `setup_output_destroy_handler` (2851).
- VT/session switching: `desktop_shell_notify_session` (2962) — on
  session re-activation re-syncs surface activation state.

## 5. Seat management

`shell_seat` per `weston_seat` (`create_shell_seat` 1632,
`get_shell_seat` 1684, caps-changed 1613, tablet-tool tracking
`handle_tablet_tool_added` 1583, `handle_seat_created` 2991, destruction
`desktop_shell_destroy_seat` 1558, `seat_destroyed` 594) — carries the
focus listeners used by activation.

---

## Observations for any future trimming

- **Cheap, self-contained removals**: the busy-cursor grab (ping/pong
  would still detect unresponsiveness); the touch and tablet-tool move
  grab machinery, if client-initiated touch/tablet moves are not needed;
  minimize support (nothing can trigger it except client requests).
- **Entangled**: focus-state tracking underpins activation and
  lock-free seat handling; the fullscreen black-curtain machinery is
  shared with output-mirroring logic in the frontend; Xwayland
  positioning spans `shell.h`'s API surface, `transform_handler`, and
  frontend `xwayland.c`.
- Session-notify (§4) matters only for VT switching on DRM seats;
  headless/nested use never exercises it.
