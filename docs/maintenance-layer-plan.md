# Future plan: maintenance layer

Status: **planned, not yet implemented — deferred indefinitely**
behind the Rust migration (`rust-migration-plan.md` decision D7);
reschedule, if at all, only after that plan's R3/R4. Agreed in design
discussion after T9 (minimize removal). This plan assumes T9 is
merged — the maintenance feature deliberately does **not** bring back
client-facing minimize. If implemented post-migration, the sketch
below translates to the `westonite-shell` crate rather than
`shell.c`.

## Concept

A "maintenance" layer stacked above the normal workspace, toggled by an
operator hotkey. Windows placed there are **fully interactive** (click,
type, move, resize). While the mode is active, everything below it is
dimmed and blocked from input. While the mode is off, the layer is
unpositioned, so its windows are invisible — giving a classic
minimize-like behavior with no client involvement.

Layer stack when active (top → bottom):

```
cursor → maintenance → dim curtain → workspace → background
```

## Controls (hotkeys proposed, pending final approval)

- **Super+M** — toggle the *focused* window between the workspace and
  maintenance layers. With the mode off this hides the window
  (minimize-like); with the mode on it moves the focused window
  whichever direction it isn't.
- **Super+Shift+M** — toggle maintenance mode itself (position the
  layer, create the dim curtains). Mnemonic pair with the existing
  Super+Shift+Space debug chain; shift = "bigger action".

Hotkeys are the **only** paths in or out of the maintenance layer:

- The minimize *button* stays gone: `minimized_requested` remains absent
  from `shell_desktop_api`, so `wm_capabilities` advertises no
  window-state requests, v5-aware toolkits draw no minimize button, and
  legacy `set_minimized` requests are NULL-guard ignored (T9 behavior,
  unchanged).
- Apps therefore cannot move themselves into or out of the maintenance
  layer programmatically — it is purely an operator tool.

## Design details

- **Maintenance layer**: a `weston_layer` given a position (above the
  dim layer) only while the mode is active; unpositioned otherwise.
  Views in an unpositioned layer are unmapped, so "off" mode hides them
  with zero extra code.
- **Dimming**: one translucent curtain per output (same `weston_curtain`
  primitive as the background), created on toggle-on / destroyed on
  toggle-off, in its own layer between maintenance and workspace.
  `capture_input = true` so dimmed windows cannot be clicked; only
  maintenance windows are interactable while the mode is on. No fade
  animation (consistent with the no-animations policy).
  - Config: `[shell] maintenance-dim=0xAARRGGBB`, default ~60 % black
    (`0x99000000`). Handle output hotplug/resize like the background
    curtain does (create in output-create path, recreate on resize).
- **Per-surface flag**: `in_maintenance` on `shell_surface`, respected
  by `shell_surface_calculate_layer_link` so activation/re-stacking
  does not silently re-layer a maintenance window back to the workspace
  (same pattern the old fullscreen `lowered` flag used).
- **Focus hygiene**:
  - Entering the mode drops keyboard focus from workspace windows (no
    typing into dimmed apps).
  - Super+M on a maintenance window while the mode is active returns it
    below the dim → unfocus it as part of the move.
  - Leaving the mode drops focus from maintenance windows (they become
    unmapped) and normal click-to-activate resumes.
- **Transient children** follow their parent's layer via the existing
  `shell_surface_update_child_surface_layers` syncing, so dialogs of a
  maintenance app appear in the maintenance layer with it.

## Why the toggle semantics are safe

`xdg_toplevel.set_minimized` was a one-shot request with no
client-visible state; compositor-side minimize policy is invisible to
apps by design. Since this feature never tells clients anything, apps
keep rendering and behaving normally in either layer. The mode-gating
also polices itself: while the mode is off, maintenance windows are
invisible and unclickable, so window-level toggles can effectively only
happen where they make sense.

## Known caveats to resolve during implementation

1. **Xwayland**: X11 apps iconify via `_NET_WM`; with `minimized_requested`
   absent that path is ignored (fine), but verify Weston's xwm does not
   itself mark the X window iconified/unmapped in a way that makes an X
   app stop drawing if an operator Super+M's it into the maintenance
   layer. Check `xwayland/window-manager.c` in the Weston 14.0.1 source.
2. **Non-transient new toplevels** opened by a maintenance-layer app map
   to the workspace layer (dimmed, unreachable until toggle-off).
   Probably acceptable; document, or optionally map new toplevels of
   maintenance-owned clients into the maintenance layer.
3. **Multi-seat**: Super+M acts on the binding seat's focused window;
   with multiple seats decide whether mode state is global (proposed:
   yes, global).

## Implementation sketch

All primitives exist in-tree today: layers (`weston_layer_set_position`
/ `unset_position`), curtains (`weston_shell_utils_curtain_create` — see
`shell_output_recreate_background`), key bindings
(`weston_compositor_add_key_binding` — see `shell_add_bindings`), config
(`weston_config_section_get_color` — see `shell_configuration`).

Estimated ~200–250 lines in `desktop-shell/shell.c` (+`shell.h`
members), one config option, docs updates (capability inventory §
"Input bindings" and a new feature section, `westonite.ini.example`,
`VENDOR.md` entry). One or two commits; build + smoke as usual, plus
eyes-on verification of the toggle in a nested (wayland/x11 backend) or
DRM session since the headless smoke suite cannot see dimming.

Layer positions: reuse `WESTON_LAYER_POSITION_UI` (freed by the panel
removal) for the maintenance layer and a value just below it for the
dim layer, keeping `CURSOR` above.

## Sequencing

1. Merge PR #8 (T9 minimize removal) — this plan builds on it.
2. New branch; implement mode + dim + Super+Shift+M first, then the
   Super+M window toggle.
3. Update capability doc (bindings section, new feature section) and
   `westonite.ini.example`; log as the next trim-series entry (feature
   addition F1) in `VENDOR.md`.
