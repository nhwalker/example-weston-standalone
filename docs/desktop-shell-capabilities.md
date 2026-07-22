# desktop-shell capability inventory (research for trimming)

> **Status after trim series T1–T4** (see `VENDOR.md`): groups **1**
> (helper-client protocol — replaced by a built-in solid-color background
> curtain, `[shell] background-color`), **4** (lock screen & idle — idle
> no longer does anything, displays never sleep), **5** (all animations),
> and **7** (input panel / on-screen keyboard, including frontend
> text-backend) have been **removed**. Groups 2, 3, 6, 8 (window
> management, bindings, multi-output/session, seats) remain. Line
> numbers below refer to the pre-trim state on `main`.

Scope: `desktop-shell/shell.c` (5029 lines), `desktop-shell/shell.h`,
`desktop-shell/input-panel.c` (425 lines), and
`protocol/weston-desktop-shell.xml`, as vendored from weston 14.0.1 (plus
our P3 no-client patch). Line numbers refer to the current branch state.

This is the complete feature map of the `desktop-shell.so` plugin, grouped
so each group is a candidate unit of removal. Interdependencies that make
a removal easy or hard are called out per group.

---

## 1. Helper-client protocol (`weston_desktop_shell`)

The private protocol served to the (optional) `weston-desktop-shell`
client. With our default of no client, all of this is dormant but still
compiled and exported.

| Capability | Protocol | Implementation |
|---|---|---|
| Background surface per output | `set_background` | `desktop_shell_set_background` (2815), `background_committed` (2781), background layer |
| Panel surface per output | `set_panel` | `desktop_shell_set_panel` (2926), `panel_committed` (2870), panel layer |
| Panel position (top/bottom/left/right) | `set_panel_position` | `desktop_shell_set_panel_position` (3098); also affects work-area math in `get_output_work_area` (371) |
| Lock-screen UI surface | `set_lock_surface`, `prepare_lock_surface` event, `unlock` | `desktop_shell_set_lock_surface` (3012), `lock_surface_committed` (2979), `desktop_shell_unlock` (3066) |
| Grab cursor feedback | `set_grab_surface`, `grab_cursor` event | `desktop_shell_set_grab_surface` (3078); used by move/resize/rotate/busy grabs to show cursors |
| Startup handshake | `desktop_ready` | `desktop_shell_desktop_ready` (3089) → `shell_fade_startup` |
| Client lifecycle | — | `launch_desktop_shell_process` (4188), crash detection `check_desktop_shell_crash_too_early` (4111), respawn (4140), `bind_desktop_shell`/`unbind_desktop_shell` (4219/4207) |

Note: the XML also declares a `weston_screensaver` interface; **shell.c
does not implement it** (vestigial in weston 14 — the screensaver feature
was removed upstream long ago).

## 2. Core window management (libweston-desktop API, `shell_desktop_api` at 2753)

The heart of the shell — xdg-shell (and Xwayland) toplevel handling:

- **Surface lifecycle**: `desktop_surface_added/removed` (2083/2140),
  `desktop_surface_committed` (2305), `map` (2252), initial placement
  `weston_view_set_initial_position` (4049), output assignment
  `shell_surface_set_output` (1705).
- **Move**: pointer grab (`move_grab_*` 1087–1121, `surface_move` 1141),
  touch grab (`touch_move_grab_*` 939–1004), tablet-tool grab
  (`tablet_tool_move_grab_*` 1177–1288).
- **Resize**: `resize_grab_*` (1317–1400), `surface_resize` (1427),
  edge-based, honors min/max size.
- **Fullscreen**: `desktop_surface_fullscreen_requested` (2566),
  `set_fullscreen` (2428), `shell_set_view_fullscreen` (1870) with black
  curtain views (`black_surface_*` 1821–1855), dedicated fullscreen
  layer, `lower_fullscreen_layer` (3563), `unset_fullscreen` (1738).
- **Maximize**: `desktop_surface_maximized_requested` (2604),
  `set_maximized` (2577), work-area-aware `get_maximized_size` (2415),
  `set_maximized_position` (2212), `unset_maximized` (1762).
- **Minimize**: `desktop_surface_minimized_requested` (2614),
  `set_minimized` (1789), `minimized_layer`.
- **Tiled/snap states**: `set_tiled_orientation*` (3194–3265) — sends
  xdg-toplevel tiled state for left/right/up/down.
- **Transient/parent surfaces**: `desktop_surface_set_parent` (2545),
  child layer syncing `shell_surface_update_child_surface_layers` (233).
- **Xwayland integration**: `desktop_surface_set_xwayland_position`
  (2732), `set_position_from_xwayland` (2229), and `transform_handler`
  (4018) which pushes view positions back to X windows via
  `weston_xwayland_surface_api` (this is why `shell.h` includes
  `<libweston/xwayland-api.h>`).
- **Unresponsive-client handling**: ping/pong
  (`desktop_surface_ping_timeout`/`pong` 2687/2719), busy-cursor grab
  (`busy_cursor_grab_*` 1469–1516, `set_busy_cursor` 2625,
  `desktop_surface_set_unresponsive` 2676).
- **Activation & focus**: `activate` (3607), click/touch/tablet-to-
  activate bindings (3694/3709/3723), keyboard-focus bookkeeping per
  workspace (`focus_state_*` 671–840, `restore_focus_state` 795),
  pointer/tablet focus listeners (1535/1615), activated-state sync to
  clients (1595), `surface_keyboard_focus_lost` (919),
  `unfocus_all_seats` (3737).
- **Stacking/layers** (`shell.h` 95–99, 149): background, panel,
  workspace (single — multi-workspace was removed upstream pre-14), lock,
  input-panel, fullscreen, minimized layers; per-view re-layering
  (`shell_surface_update_layer` 1680).

## 3. Input bindings (`shell_add_bindings`, 4808)

Fixed (always on):
- Click/touch/tablet-tap to activate (BTN_LEFT/BTN_RIGHT/touch/tool).
- `KEY_BRIGHTNESSDOWN/UP` → backlight control (`backlight_binding` 4408).
- Ctrl+Alt+Backspace terminate compositor — only if `allow-zap=true`
  (`terminate_binding` 3367).

Gated on `binding-modifier` (default `super`, `[shell]
binding-modifier=none` disables the whole block — note the 14.0.1 RPM
config-parser bug makes `none` unreliable, see phase0 findings D1):
- mod+Shift+M maximize toggle (3152); mod+Shift+F fullscreen toggle (3171).
- mod+LeftDrag move (3128); mod+touch move (3273); mod+RightDrag and
  mod+Shift+LeftDrag resize (3295).
- mod+Shift+arrow tiled snap (3240–3265).
- mod+MiddleDrag rotate — only when the renderer reports
  `WESTON_CAP_ROTATION_ANY` (`rotate_binding` 3529, grab 3376–3529).
- mod+Tab window switcher with dimming (`switcher_*` 4249–4384).
- mod+F9/F10 backlight down/up.
- mod+K force-kill focused client (`force_kill_binding` 4441 — SIGKILL
  by pid, skips xwayland surfaces).
- Super+Alt+scroll surface opacity (`surface_opacity_binding` 3340).
- Debug key chain mod+Shift+Space (`weston_install_debug_key_binding` —
  implemented in libweston, registered here).

## 4. Lock screen & idle handling

- `idle_handler` (3994): on compositor idle → break grabs, fade out;
  `wake_handler` (4009): fade in / `resume_desktop` (3043).
- `lock` (3753): unmaps panel/fullscreen/workspace/input-panel layers,
  installs `lock_layer`, sleeps outputs (DPMS via
  `weston_compositor_sleep`), drops keyboard focus.
- `unlock` (3790): if a helper client exists, sends
  `prepare_lock_surface` and waits for `set_lock_surface`/`unlock`;
  **without a client it resumes directly** (already works with our
  no-client default).
- Screen fade machinery shared with startup: `shell_fade*`
  (3814–3962), fade curtain views.

## 5. Animations / visual effects

Config-selected in `shell_configuration` (520):
- `startup-animation` (fade/none) + `desktop_ready` handshake + 15 s
  timeout (`shell_fade_init` 3962; our P3 fades in immediately with no
  client).
- Window open `animation` (none/zoom/fade) and `close-animation`
  (fade/none) — applied in `map`/`desktop_surface_removed`.
- `focus-animation` (none/dim-layer): per-output focus dimming surfaces
  (`focus_surface_*` 576–630, `animate_focus_change` 638).
- Busy cursor animation during unresponsive grabs.

## 6. Multi-output / hotplug / session handling

- Per-output state `shell_output` (`create_shell_output` 4624), with
  background/panel per output.
- Output hotplug: `handle_output_create/destroy/move` (4646/4562/4675),
  layer view migration (`shell_output_changed_move_layer` 4534,
  `handle_output_move_layer` 4656), surface repositioning when an
  output disappears (`shell_reposition_view_on_output_change` 4468),
  resize handling (`handle_output_resized*` 4585/4610,
  `shell_resize_surface_to_output` 4571).
- Work-area computation honoring panel position/size
  (`get_output_work_area` 371) — feeds maximize, snap, and initial
  placement.
- VT/session switching: `desktop_shell_notify_session` (4883) — on
  session re-activation re-syncs surface activation state.

## 7. On-screen-keyboard (input panel) support

`input-panel.c` + text-input plumbing in shell.c/h:
- Implements the `input_panel` surface role of
  `input-method-unstable-v1` (the *compositor half* only; the actual
  keyboard client is external — disabled by default via our P4).
- Show/hide/update on text-input focus (`show_input_panels` 127,
  `hide_input_panels` 152, `update_input_panels` 174, listeners in
  `shell.h` 90–92), positioning bottom-center or as overlay
  (`calc_input_panel_position` 63), dedicated `input_panel_layer`,
  interaction with lock (panels hidden while locked).
- `text-backend.c` (frontend, not desktop-shell) provides the
  text-input↔input-method wiring these listeners hang off.

## 8. Seat management

`shell_seat` per `weston_seat` (`create_shell_seat` 1979, caps-changed
1960, tablet tool tracking 1919/1930, `handle_seat_created` 4912,
destruction 1905/1950) — carries focus listeners and the popup-grab
state used by activation and the switcher.

---

## Observations relevant to trimming

- **Already dormant by config** (but still compiled): the entire helper-
  client protocol surface (§1), input-panel support (§7 — client
  disabled by P4), and the lock-screen *UI* path (client-side half of
  §4).
- **Cheap, self-contained removals**: individual bindings in §3 (each is
  one registration + its handler/grab functions — e.g. rotate, opacity,
  switcher, backlight, force-kill, zap); animations in §5 (each type is
  independently selectable already); tiled-snap; touch/tablet move
  grabs.
- **Entangled, higher-effort removals**: §1 background/panel removal
  touches work-area math (§6) and layer setup; lock/idle (§4) is wired
  into fade machinery shared with startup animation; focus-state
  tracking (§2) underpins activation, the switcher, and lock/unlock
  restore; removing Xwayland positioning (§2) changes `shell.h`'s API
  surface and `transform_handler`.
- **Protocol XML**: if §1 goes entirely, `weston-desktop-shell.xml`, the
  generated protocol code, the `bind_desktop_shell` global, and the
  client-lifecycle code all fall out together, and `desktop-shell.so`
  stops depending on the `WESTON_SHELL_CLIENT` define.
- The `weston_screensaver` interface in the XML is dead weight today —
  removable at zero functional cost.
