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

## Rebasing to a newer 14.0.x

1. `git -C <weston> diff 14.0.1..<new-tag> -- frontend/ desktop-shell/ shared/ protocol/weston-desktop-shell.xml`
2. Apply the hunks touching imported files (drop P0 if superseded).
3. Update the tag/commit above, the version in `meson.build`, and
   `rpm/westonite.spec`; rebuild and rerun the smoke tests.
