# Vendored code provenance

Source: https://gitlab.freedesktop.org/wayland/weston (via the
`nhwalker/weston` mirror), tag **`14.0.1`**, commit
`61f2248de2239b64aa2eab09bbbb3a6ce5e79065` — chosen to match the EPEL 10
RPM `weston-14.0.1-3.el10_0` this port links against
(see `docs/phase0-findings.md`). License: MIT/Expat (`COPYING`).

## Imported files (verbatim unless listed under "Local patches")

| Here | Upstream |
|---|---|
| `frontend/{main,executable,text-backend,config-helpers,weston-screenshooter,xwayland}.c`, `frontend/{weston,weston-private}.h` | `frontend/` (same names) |
| `desktop-shell/{shell.c,shell.h,input-panel.c}` | `desktop-shell/` (same names) |
| `shared/{os-compatibility,process-util}.{c,h}`, `shared/option-parser.c`, `shared/{helpers,string-helpers,xalloc,timespec-util,fd-util}.h` | `shared/` (same names) |
| `protocol/weston-desktop-shell.xml` | `protocol/weston-desktop-shell.xml` |
| `git-version.h.meson` | `libweston/git-version.h.meson` |
| `COPYING` | `COPYING` |

Upstream files NOT imported (deliberate, see PLAN.md):
`frontend/{screen-share,systemd-notify}.c`, all of `clients/`,
`shared/config-parser.c` (exported by the RPM's libweston-14.so),
everything under `libweston/` (consumed as the RPM).

All `meson.build` files and `meson_options.txt` are written for this repo
(modeled on upstream's but not copies).

## Local patches

- **P0** — backport of upstream `51dfd1be` ("frontend: Fix crash in output
  resize handler", the only 14.0.1→14.0.2 change to a vendored file) into
  `frontend/main.c`.
- **P2** — `frontend/main.c`: default config file name `weston.ini` →
  `westonite.ini` (lookup string + usage text; same XDG search logic).
- **P3** — `desktop-shell/shell.c`: an empty `[shell] client=` (or empty
  `WESTON_SHELL_CLIENT` default, set via the `shell-client-default` meson
  option) disables the helper client: no spawn is scheduled, and the
  startup fade is triggered immediately instead of waiting on the 15 s
  desktop_ready timeout.
- **P4** — `frontend/text-backend.c`: `[input-method] path` defaults to
  empty (upstream defaulted to `$libexecdir/weston-keyboard`, which
  westonite does not ship); empty was already handled as "disabled".
- **T1** — `protocol/weston-desktop-shell.xml`: removed the vestigial
  `weston_screensaver` interface (never implemented or advertised by
  shell.c; upstream keeps it only as a fossil). First change of the
  desktop-shell trim series; see `docs/desktop-shell-capabilities.md`.
- **T2** — removed input-panel / on-screen-keyboard support: deleted
  `desktop-shell/input-panel.c` and `frontend/text-backend.c` (whose
  lifecycle shell.c owned), their layer/listener/state members, and the
  `text-input-unstable-v1` + `input-method-unstable-v1` generated
  protocols.
- **T3** — removed the helper-client (`weston_desktop_shell`) protocol,
  lock screen, idle handling, and the screen-fade machinery: deleted the
  protocol XML (supersedes T1) and all client lifecycle/request handling,
  `lock()`/`unlock()`/`resume_desktop()`, idle/wake listeners (displays
  never sleep now, per project decision), fade curtains, panel layer and
  panel-aware work-area logic, grab-cursor feedback, and the P3 patch
  (nothing left to spawn). Replaced the client-drawn background with a
  compositor-side solid curtain per output, configurable via
  `[shell] background-color` (default `0xff002244`).
- **T4** — removed all remaining animations: window open (`zoom`/`fade`)
  and close (`fade`) animations, the focus dim-layer animation with its
  focus-surface curtains, `get_animation_type`, and the `animation`,
  `close-animation`, `startup-animation`, `focus-animation` config
  options (stale keys in existing configs are ignored harmlessly).
- **T5** — removed every hotkey binding (per-binding user decisions):
  zap (Ctrl+Alt+Backspace, with the `allow-zap` option), move/resize
  drags, maximize/fullscreen toggles, tiled-snap (with the whole
  orientation state), rotate (with the whole rotation machinery,
  including the busy-grab right-click rotate), the mod+Tab switcher,
  force-kill, backlight keys (fixed and mod+F9/F10), and surface
  opacity — plus the now-meaningless `binding-modifier` option. The debug-key
  chain was subsequently restored with a hardcoded Super modifier.
  Otherwise only pointer/touch/tablet click-to-activate bindings remain;
  window management is exclusively client-initiated (xdg-shell).
- **T6** — removed fullscreen and maximize support: dropped the
  `fullscreen_requested`/`maximized_requested` entries from
  `shell_desktop_api` (libweston-desktop then omits both from
  `xdg_toplevel.wm_capabilities`, so v5-aware toolkits hide the buttons,
  and NULL-guards ignore requests from legacy clients), and deleted
  `set_fullscreen`/`unset_fullscreen`/`set_maximized`/`unset_maximized`,
  the black letterbox curtains, the fullscreen layer and lowering logic,
  the maximize sizing/positioning helpers, the shared saved-position
  restore machinery, the `surface_state` tracking struct, and the
  output-resize window re-fitting. Windows are free-floating and
  client-sized only. (Frontend `--fullscreen` — the nested-backend
  window option — is unrelated and untouched.)
- **T7** — removed the tablet-tool (pen) machinery: pen tap-to-activate
  binding, the pen-driven window-move grab (and its branch in
  `desktop_surface_move`), and the per-seat tool tracking / focus-ping
  listeners. Pens remain fully functional *inside* applications — the
  tablet-v2 protocol (pressure, tilt, proximity) is delivered by
  libweston directly to clients — but can no longer focus or drag
  windows. Touch keeps both tap-to-activate and window dragging, per
  project decision.

## Rebasing to a newer 14.0.x

1. `git -C <weston> diff 14.0.1..<new-tag> -- frontend/ desktop-shell/ shared/ protocol/weston-desktop-shell.xml`
2. Apply the hunks touching imported files (drop P0 if superseded).
3. Update the tag/commit above, the version in `meson.build`, and
   `rpm/westonite.spec`; rebuild and rerun the smoke tests.
