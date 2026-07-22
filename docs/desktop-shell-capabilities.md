# desktop-shell capability inventory

Scope: `desktop-shell/shell.c` (3841 lines) and `desktop-shell/shell.h`
(115 lines), as of the post-trim state (trim series T1–T4, logged in
`VENDOR.md`). Line numbers refer to this state.

Westonite's desktop-shell is a **pure window-manager plugin**: it serves
no client-facing shell protocol of its own, spawns no helper clients, and
draws a built-in solid-color background. Features upstream weston's
desktop-shell has that this one deliberately does **not**: helper-client
protocol (panel, wallpaper images, grab cursors), lock screen, idle/DPMS
handling (displays never sleep), all animations, input-panel /
on-screen-keyboard support (including the frontend text-backend), and
multiple workspaces (already single-workspace upstream in v14).

Each numbered group below is a candidate unit for any future trimming.

---

## 1. Built-in background

One solid-color `weston_curtain` per output, drawn by the compositor
itself (no client involved):

- Created in `create_shell_output` (3500) via
  `shell_output_recreate_background` (3442), recreated on output resize
  (`handle_output_resized` 3445), destroyed on output destroy; label via
  `background_get_label` (3458).
- Color from `[shell] background-color` (0xAARRGGBB), read in
  `shell_configuration` (449), default `0xff002244`.
- Lives in the dedicated background layer; captures input (clicks on
  empty desktop go nowhere).

## 2. Core window management (libweston-desktop API, `shell_desktop_api` at 2383)

xdg-shell (and Xwayland) toplevel handling:

- **Surface lifecycle**: `desktop_surface_added/removed` (1771/1828),
  `desktop_surface_committed` (1943), `map` (1905), initial placement
  `weston_view_set_initial_position` (1441), output assignment.
- **Move**: pointer grab (`surface_move` 849, `desktop_surface_move`
  2106), touch grab (`surface_touch_move` 738), tablet-tool grab
  (`surface_tablet_tool_move` 996); shared grab helpers
  `shell_grab_start` (329), `shell_touch_grab_start` (387),
  `shell_tablet_tool_grab_start` (418).
- **Resize**: `surface_resize` (1135), `desktop_surface_resize` (2149),
  edge-based, honors min/max size.
- **Fullscreen**: `desktop_surface_fullscreen_requested` (2197),
  `set_fullscreen` (2059), `shell_set_view_fullscreen` (1577) with black
  curtain views, dedicated fullscreen layer, `lower_fullscreen_layer`
  (2837), `unset_fullscreen` (1445).
- **Maximize**: `desktop_surface_maximized_requested` (2235),
  `set_maximized` (2208), `get_maximized_size` (222),
  `set_maximized_position` (1865), `unset_maximized` (1469). Work area =
  full output (`get_output_work_area` 347).
- **Minimize**: `desktop_surface_minimized_requested` (2245),
  `set_minimized` (1496), `minimized_layer`.
- **Tiled/snap states**: `set_tiled_orientation` (2468) — sends
  xdg-toplevel tiled state for left/right/up/down.
- **Transient/parent surfaces**: `desktop_surface_set_parent` (2176),
  child layer syncing `shell_surface_update_child_surface_layers` (219).
- **Xwayland integration**: `desktop_surface_set_xwayland_position`
  (2362), `set_position_from_xwayland` (1882), and `transform_handler`
  (3003) pushing view positions back to X windows via
  `weston_xwayland_surface_api` (why `shell.h` includes
  `<libweston/xwayland-api.h>`).
- **Unresponsive-client handling**: `desktop_surface_ping_timeout`/`pong`
  (2317/2349), busy-cursor pointer grab (`busy_cursor_grab_focus` 1176,
  `set_busy_cursor` 210, `end_busy_cursor` 2274,
  `desktop_surface_set_unresponsive` 2306). Purely behavioral now — no
  special cursor sprite is shown.
- **Activation & focus**: `activate` (2881),
  click/touch/tablet-to-activate bindings (2959/2974/2988),
  per-workspace focus-state bookkeeping (`focus_state_create` 538,
  `ensure_focus_state` 563, `focus_state_set_focus` 579,
  `surface_keyboard_focus_lost` 653), pointer/tablet focus listeners
  (1242/1322), `shell_surface_update_layer` (1387).
- **Stacking/layers** (`shell.h`): cursor (libweston), fullscreen,
  workspace (single: `workspace_create` 634, `get_current_workspace`
  647), minimized, background; iteration helper `shell_for_each_layer`
  (3373).

## 3. Input bindings (`shell_add_bindings`, 3656)

Fixed (always on):
- Click/touch/tablet-tap to activate.
- `KEY_BRIGHTNESSDOWN/UP` → backlight control (`backlight_binding` 3259).
- Ctrl+Alt+Backspace terminate compositor — only if `allow-zap=true`
  (`terminate_binding` 2641).

Gated on `binding-modifier` (default `super`; `none` disables the block —
note the 14.0.1 RPM config-parser bug makes `none` unreliable, see
`docs/phase0-findings.md` D1):
- mod+Shift+M maximize toggle (2426); mod+Shift+F fullscreen toggle (2445).
- mod+LeftDrag move (2402); mod+touch move (2547); mod+RightDrag and
  mod+Shift+LeftDrag resize (2569).
- mod+Shift+arrow tiled snap (2514).
- mod+MiddleDrag rotate — only when the renderer reports
  `WESTON_CAP_ROTATION_ANY` (`rotate_binding` 2803, `surface_rotate` 213).
- mod+Tab window switcher (`switcher_next` 3104, `switcher_binding` 3235).
- mod+F9/F10 backlight down/up.
- mod+K force-kill focused client (`force_kill_binding` 3292 — SIGKILL by
  pid, skips xwayland surfaces).
- Super+Alt+scroll surface opacity (`surface_opacity_binding` 2614).
- Debug key chain mod+Shift+Space (`weston_install_debug_key_binding`,
  implemented in libweston, registered here).

Config read in `shell_configuration` (449): `background-color`,
`allow-zap`, `binding-modifier` — the complete `[shell]` option set.

## 4. Multi-output / hotplug / session handling

- Per-output state `shell_output` (`create_shell_output` 3500) carrying
  the background curtain.
- Output hotplug: `handle_output_create/destroy/move` (3523/3408/3552),
  layer view migration, surface repositioning when an output disappears
  (`shell_reposition_view_on_output_change` 3319), resize handling
  (`handle_output_resized` 3445, `handle_output_resized_shsurfs` 3417),
  destroy wiring `setup_output_destroy_handler` (3563).
- VT/session switching: `desktop_shell_notify_session` (3731) — on
  session re-activation re-syncs surface activation state.

## 5. Seat management

`shell_seat` per `weston_seat` (`create_shell_seat` 1686,
`get_shell_seat` 1738, caps-changed 1667, tablet-tool tracking
`handle_tablet_tool_added` 1637, `handle_seat_created` 3760, destruction
`desktop_shell_destroy_seat` 1612, `seat_destroyed` 620) — carries the
focus listeners used by activation and the switcher.

---

## Observations for any future trimming

- **Cheap, self-contained removals**: individual bindings in §3 (each is
  one registration plus its handler/grab functions — e.g. rotate,
  opacity, switcher, backlight, force-kill, zap, tiled-snap); the
  touch and tablet-tool move grabs; the busy-cursor grab (ping/pong
  would still detect unresponsiveness).
- **Entangled**: focus-state tracking (§2) underpins activation and the
  switcher; the fullscreen black-curtain machinery is shared with
  output-mirroring logic in the frontend; Xwayland positioning spans
  `shell.h`'s API surface, `transform_handler`, and frontend
  `xwayland.c`.
- Session-notify (§4) matters only for VT switching on DRM seats;
  headless/nested use never exercises it.
