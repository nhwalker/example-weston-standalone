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

## Rebasing to a newer 14.0.x

1. `git -C <weston> diff 14.0.1..<new-tag> -- frontend/ desktop-shell/ shared/ protocol/weston-desktop-shell.xml`
2. Apply the hunks touching imported files (drop P0 if superseded).
3. Update the tag/commit above, the version in `meson.build`, and
   `rpm/westonite.spec`; rebuild and rerun the smoke tests.
