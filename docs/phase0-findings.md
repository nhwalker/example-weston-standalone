# Phase 0 Findings — UBI 10 / RHEL 10 environment truth

Date: 2026-07-21. Method: package availability and file lists were first
verified against the **public repository metadata** (repomd/primary/
filelists XML) of UBI 10, CentOS Stream 10, and EPEL 10 — equivalent to
`dnf repoquery` / `rpm -ql` against those repos — and then **confirmed by
building `containers/Containerfile.build` in docker** (CentOS Stream 10
base) and inspecting the resulting image (see "Container validation").

## Headline results

1. **RHEL 10 / CentOS Stream 10 do not ship weston at all.** Neither
   AppStream (15790 pkgs) nor CRB (3834 pkgs) contains any `weston*`
   package (only boolean deps in other packages that *accept* weston as one
   of several compositors).
2. **Public UBI 10 repos don't either** — and they are a small subset
   (appstream: 1122 pkgs; only `libwayland-*` runtime libs, no `-devel`).
3. **weston 14 comes from EPEL 10**: `weston-14.0.1-3.el10_0` with
   subpackages `weston`, `weston-libs`, `weston-devel`, `weston-demo`,
   `weston-session` (verified in `epel/10.3/Everything/x86_64`).

So on UBI10 the stack is: **EPEL 10 provides weston/libweston 14.0.1**, and
its build deps come from BaseOS/AppStream/CRB (entitled RHEL repos, or
CentOS Stream 10 for unentitled CI builds).

## What the EPEL 10 weston RPMs contain (from repo filelists)

**`weston-devel` — resolves plan risks R2 and R3 in our favor:**

- ALL seven backend headers are installed:
  `/usr/include/libweston-14/libweston/backend-{drm,headless,pipewire,rdp,vnc,wayland,x11}.h`
  → the unconditional includes in `frontend/main.c` compile as-is; the
  planned include-guard patch (P1) is **not needed** for this target (kept
  as a contingency only).
- `xwayland-api.h` is installed → Xwayland glue compiles.
- Also: `libweston.h`, `desktop.h`, `shell-utils.h`, `config-parser.h`,
  `weston-log.h`, `windowed-output-api.h`, `plugin-registry.h`,
  `matrix.h`, `zalloc.h`, `version.h`, `remoting-plugin.h`,
  `pipewire-plugin.h`; `libweston-14.so` symlink; pkg-config
  `libweston-14.pc`, `weston.pc`, `libweston-14-protocols.pc`; and even the
  upstream plugin-SDK header `/usr/include/weston/weston.h`.

**`weston-libs` — full runtime, resolves R3's module concern:**

- `/usr/lib64/libweston-14.so.0.0.1` and modules in
  `/usr/lib64/libweston-14/`: `drm-backend.so`, `headless-backend.so`,
  `wayland-backend.so`, `x11-backend.so`, `rdp-backend.so`,
  `vnc-backend.so`, `pipewire-backend.so`, `gl-renderer.so`,
  `color-lcms.so`, `xwayland.so`, `remoting-plugin.so`,
  `pipewire-plugin.so`. **Every backend + Xwayland module is present.**

**`weston`** ships the upstream frontend/shells (`/usr/bin/weston`,
`/usr/lib64/weston/{desktop-shell,kiosk-shell,ivi-shell,fullscreen-shell,
screen-share,systemd-notify}.so`, `libexec_weston.so*`) and the helper
clients `/usr/libexec/weston-desktop-shell`, `weston-keyboard`,
`weston-simple-im`. Two consequences:

- Our install layout (`/usr/bin/westonite`, `/usr/lib64/westonite/`)
  **cannot collide** with it, as planned.
- A panel can be re-enabled later with `[shell] client=
  /usr/libexec/weston-desktop-shell` (requires installing EPEL's `weston`
  package) — no need to ever port the clients.

## Build dependency versions (CentOS Stream 10 = RHEL 10 content set)

| Package | Version | Repo | Upstream requirement |
|---|---|---|---|
| meson | 1.7.2 | **CRB** | >= 0.63 ✓ |
| wayland-devel | 1.25.0 | AppStream | ✓ |
| wayland-protocols-devel | 1.49 | AppStream | >= 1.33 ✓ |
| libinput-devel | 1.30.1 | **CRB** | ✓ |
| libevdev-devel | 1.13.1 | AppStream | ✓ |
| gcc | 14.4.1 | AppStream | ✓ |
| xorg-x11-server-Xwayland | 24.1.9 | AppStream | runtime, Phase 3 ✓ |
| rpm-build | 4.19.1.1 | AppStream | Phase 4 ✓ |

Note `meson` and `libinput-devel` live in **CRB**, which must be enabled.

## Decisions taken

- **D1 — import base is tag `14.0.1`**, matching the EPEL RPM exactly
  (plan previously said 14.0.2). Between 14.0.1 and 14.0.2 only two files
  in our vendored set changed:
  - `frontend/main.c` — commit `51dfd1be` "frontend: Fix crash in output
    resize handler". We vendor this file, so we **backport it as patch P0**
    (logged in VENDOR.md).
  - `shared/config-parser.c` — commit `ee92a531` "shared: fix
    binding-modifier none". This file lives **inside the RPM's
    libweston-14.so**, so the 14.0.1 bug is present at runtime and we
    cannot fix it from our side. Known limitation: don't use
    `binding-modifier=none` in westonite.ini until EPEL rebases to 14.0.2.
- **D2 — build image**: default `quay.io/centos/centos:stream10`
  (+ `epel-release`, + CRB enabled) so CI needs no entitlement; the same
  Containerfile accepts `--build-arg BASE_IMAGE=registry.access.redhat.com/ubi10/ubi`
  on an entitled host (UBI containers inherit host subscription; EPEL and
  CRB enabled the RHEL way). See `containers/Containerfile.build`.
- **D3 — drop patch P1** (backend include guards) from the required set;
  meson still probes each `libweston/backend-*.h` and fails with a clear
  message if the RPM ever stops shipping one.

## Container validation

`containers/Containerfile.build` was built successfully with docker from
the CentOS Stream 10 base: `dnf install epel-release` works out of the box,
`dnf config-manager --set-enabled crb` works, every package resolves, and
the image's built-in sanity check passed
(`pkg-config --exists 'libweston-14 >= 14.0.1'`, `headless-backend.so`,
`xwayland.so`, `xwayland-api.h`). Installed versions in the image:

```
weston-devel-14.0.1-3.el10_0    weston-libs-14.0.1-3.el10_0
meson-1.7.2-1.el10              gcc-14.4.1-1.el10
wayland-devel-1.25.0-1.el10     wayland-protocols-devel-1.49-2.el10
libinput-devel-1.30.1-2.el10    libevdev-devel-1.13.1-6.el10
xorg-x11-server-Xwayland-24.1.9-6.el10
```

`/usr/lib64/libweston-14/` in the image contains all modules predicted by
the metadata: color-lcms, drm-backend, gl-renderer, headless-backend,
pipewire-backend, pipewire-plugin, rdp-backend, remoting-plugin,
vnc-backend, wayland-backend, x11-backend, xwayland.

(Build note for sandboxed/proxied environments like this session: the
image was built on top of a thin local wrapper of the CS10 base that
installs the outbound-proxy CA into the trust store and sets `proxy=` in
dnf.conf; the repo Containerfile itself is unchanged and works as-is in
normally-networked environments via its `BASE_IMAGE` build-arg.)

## Plan risk status after Phase 0

- **R1 (package availability): resolved** — EPEL 10 on top of RHEL 10
  (entitled) or CentOS Stream 10 (CI). Public-UBI-only builds are not
  possible; this is inherent to UBI, not fixable by us.
- **R2 (backend header subset): resolved** — all headers shipped.
- **R3 (Xwayland availability): resolved** — header and module shipped;
  `xorg-x11-server-Xwayland` available in AppStream.
- **R4 (version skew): active but pinned** — port tracks `14.0.1` until
  EPEL moves; re-check this document's queries when it does.
