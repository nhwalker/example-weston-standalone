# RecreatePlan.md — rebuild `westonite` from the upstream Weston tree

This is an execution plan for an agent that has **only the upstream Weston
repository** (https://gitlab.freedesktop.org/wayland/weston, tag `14.0.1`)
as reference material. Following it phase by phase reproduces the
`example-weston-standalone` repository as it stood at the end of its C-only
history: a standalone build of the Weston 14 compositor **frontend** and
**desktop-shell** plugin, linked against the distribution's libweston 14
RPMs, renamed `westonite`, trimmed to a pure window-manager shell, packaged
as an RPM, and covered by a black-box end-to-end test suite that runs in
CI.

Everything in this document was reconstructed from the finished
repository's git history, design documents and code. Where the record was
incomplete or could not be re-verified from the repository alone, the
gap is stated explicitly (see Appendix C) rather than papered over.

---

## 0. How to use this document

**Audience.** One agent, working in a fresh git repository, with:

- the upstream Weston repository checked out locally (reference only —
  it is never modified);
- `docker` (or `podman`) able to pull `quay.io/centos/centos:stream10`
  and reach the CentOS Stream 10 and EPEL 10 repositories;
- push access to a GitHub repository with Actions enabled (for the CI
  phase).

**What you will produce.** A repository that builds `/usr/bin/westonite`,
`/usr/lib64/westonite/libexec_westonite.so` and
`/usr/lib64/westonite/desktop-shell.so`, installs a
`wayland-sessions/westonite.desktop` entry and an example config, packages
all of it as `westonite-14.0.1-1.el10.x86_64.rpm`, and verifies it with a
smoke script plus a 45-test pytest e2e suite driven over the VNC backend.

**How the plan is organised.** The work is ordered exactly as it was
originally done, because later steps depend on earlier verification:

| Stage | What it delivers |
|---|---|
| Phase 0 | Environment truth: which RPMs provide what, the build container |
| Phase 1 | Verbatim import + meson build + one upstream backport (P0) |
| Phase 2 | `westonite` identity + no helper clients (P2, P3, P4) |
| Phase 3 | Xwayland verified |
| Phase 4 | RPM spec, built and install-tested in a pristine container |
| Phase 5 | README, reusable verification scripts, GitHub Actions CI |
| Trims T1–T9 | Reduce desktop-shell to a pure window manager (with two capability inventories and one deferred feature design) |
| E2E E1–E5 | Black-box test suite over VNC, in-repo test clients, installed-RPM subset, CI artifacts |
| Hardening | Two CI flake fixes found in the first days of CI runs |

**Rules that apply throughout.**

1. **Never modify the upstream Weston tree.** It is reference-only.
2. **Every deviation from a verbatim upstream file is a discrete commit
   with an entry in `VENDOR.md`** (patches are numbered `P0`, `P2`, `P3`,
   `P4`; trims are `T1`…`T9`). There is no `P1`: it was planned and then
   dropped in Phase 0 (see §4).
3. **Every phase and every trim ends green**: a zero-warning build and the
   smoke suite passing in the build container (and, once it exists, the
   e2e suite). Do not stack unverified steps.
4. **Every decision listed in §1 is settled.** Do not reopen them, do not
   ask about them; execute them. Where the plan says "product decision",
   that is the reason and the whole reason.
5. **Scope is our code only.** Bugs inside the EPEL `weston-libs` RPM
   (libweston, backends, neatvnc) are documented and worked around, never
   fixed here and never tested as the subject of a test.
6. Commit messages describe what changed and what verification passed.
   Keep the `Co-Authored-By`/session trailer conventions of whatever
   harness you run under; they are not part of this plan.

**Out of scope for this plan** (they happened later in the real
repository and are deliberately not covered here): the Rust migration of
the frontend and shell, the DRM-in-a-VM test harness, and the e2e tests
added during that migration (screenshooter, nested backends, colour
management, DRM, logging).

---

## 1. Fixed decisions

These were made interactively by the project owner during the original
work. They are encoded here as settled facts.

### 1.1 Scope and identity

| ID | Decision |
|---|---|
| D-SCOPE-1 | Port only `frontend/` (the `weston` binary) and `desktop-shell/`. libweston itself is consumed from the distro RPM and never built. |
| D-SCOPE-2 | Rename to **`westonite`**: binary `weston`→`westonite`, module dir `$libdir/weston`→`$libdir/westonite`, config `weston.ini`→`westonite.ini`, session file `weston.desktop`→`westonite.desktop`. Internal env vars (`WESTON_MODULE_MAP`, `WESTON_CONFIG_FILE`, …) and all libweston interfaces stay untouched, so the result installs alongside the stock `weston` package with no file collisions. |
| D-SCOPE-3 | **No helper clients are ported or shipped**: no `weston-desktop-shell` (panel/background), no `weston-keyboard`, no toytoolkit/cairo stack, no `clients/` at all. |
| D-SCOPE-4 | `frontend/screen-share.c` and `frontend/systemd-notify.c` are not imported (screen-share needs a libweston *private* header). |
| D-SCOPE-5 | Xwayland support **is** enabled (`frontend/xwayland.c` imported; the RPM's `xwayland.so` module is loaded at runtime). |
| D-SCOPE-6 | Upstream baseline is tag **`14.0.1`**, matching the EPEL 10 RPM exactly. One fix from 14.0.2 is backported (P0). |
| D-SCOPE-7 | Target platform is RHEL 10 / UBI 10 / CentOS Stream 10 **with EPEL 10**. Unentitled CI builds on CentOS Stream 10; the same Containerfile accepts a UBI 10 base on an entitled host. |
| D-SCOPE-8 | The port itself is packaged as an RPM (`westonite.spec`) built inside the container from a `git archive` tarball. |

### 1.2 Desktop-shell trims (each is a separate commit; details in §10)

| ID | Decision |
|---|---|
| T1 | Remove the vestigial `weston_screensaver` interface from the private protocol XML. |
| T2 | Remove input-panel / on-screen-keyboard support entirely (`input-panel.c`, `text-backend.c`, and the two generated protocols). |
| T3 | Remove the helper-client protocol, lock screen, idle handling (**displays never sleep**), and all screen fades. Replace the client-drawn background with a compositor-side solid curtain per output, configured by `[shell] background-color`, default `0xff002244`. |
| T4 | Remove all animations (open, close, focus dim) and their config keys. |
| T5 | Remove every hotkey binding and the `binding-modifier` option. Then **restore only** libweston's debug-key chain with a hardcoded Super modifier. Window management is exclusively client-initiated (xdg-shell) plus click/touch-to-activate. |
| T6 | Remove fullscreen and maximize (drop both `weston_desktop_api` callbacks so they vanish from `wm_capabilities`). Windows are free-floating and client-sized only. |
| T7 | Remove tablet-tool (pen) window-management machinery. Pens still work inside apps; **touch keeps** tap-to-activate and window dragging. |
| T8 | A left-click on an unresponsive window only activates it (no move grab). |
| T9 | Remove minimize (`minimized_requested`). After this, `wm_capabilities` advertises **no** window-state requests. |
| D-MAINT | A "maintenance layer" feature (operator-toggled hidden layer, minimize-like) was **designed but deferred indefinitely**. Write the design doc (§11.3); do not implement it. |

### 1.3 Testing

| ID | Decision |
|---|---|
| D-TEST-1 | **Black-box scripts only.** The upstream weston test harness is not ported. No test hooks are compiled into shipped code. |
| D-TEST-2 | The **VNC backend** from EPEL's `weston-libs` is the test control plane: one authenticated RFB connection gives scripted pointer/keyboard injection and framebuffer capture. |
| D-TEST-3 | Pixel tests: yes, but every scene is flat-coloured, so all assertions are computed exact-match checks. **No reference images exist.** |
| D-TEST-4 | Renderer pinned to **pixman** for determinism. |
| D-TEST-5 | The stock `weston` package is **never installed** in a test container. Window-creating test drivers are two minimal C clients built in this repo (`wtest-client`, `wtest-xclient`), gated behind a meson option and never installed. |
| D-TEST-6 | VNC auth runs through a **real `pam_unix` stack** as a dedicated non-root user, not `pam_permit`. |
| D-TEST-7 | The RFB client is an in-repo, pure-Python implementation of **Apple DH** auth (needs only `python3-cryptography` from BaseOS). No pip anywhere. |
| D-TEST-8 | CI runs on push to `main` and on pull requests only. No scheduled jobs. |
| D-TEST-9 | No sleeps on the happy path: every wait polls a log line, a client stdout line, or a capture predicate with a deadline. Fixed short sleeps appear only in negative tests. |
| D-TEST-10 | Every test asserts exit code 0 on teardown; every test is also a clean-shutdown test. |

---

## 2. Platform facts (what Phase 0 establishes)

These are the facts the whole build rests on. Re-verify them in Phase 0;
they are recorded here so you know what to expect.

1. **RHEL 10 / CentOS Stream 10 / public UBI 10 ship no weston at all.**
   Neither AppStream nor CRB contains any `weston*` package.
2. **weston 14 comes from EPEL 10**: `weston-14.0.1-3.el10_0`, subpackages
   `weston`, `weston-libs`, `weston-devel`, `weston-demo`,
   `weston-session`.
3. **`weston-devel` installs all seven backend headers**
   (`/usr/include/libweston-14/libweston/backend-{drm,headless,pipewire,rdp,vnc,wayland,x11}.h`)
   plus `xwayland-api.h`, `libweston.h`, `desktop.h`, `shell-utils.h`,
   `config-parser.h`, `weston-log.h`, `windowed-output-api.h`,
   `plugin-registry.h`, `matrix.h`, `zalloc.h`, `version.h`,
   `remoting-plugin.h`, `pipewire-plugin.h`; pkg-config files
   `libweston-14.pc`, `weston.pc`, `libweston-14-protocols.pc`.
   Consequence: the unconditional backend `#include`s in `frontend/main.c`
   compile as-is; the originally planned patch **P1** (guarding those
   includes) is unnecessary and was dropped.
4. **`weston-libs` installs the full runtime**: `libweston-14.so.0.0.1`
   and, in `/usr/lib64/libweston-14/`: `drm-backend.so`,
   `headless-backend.so`, `wayland-backend.so`, `x11-backend.so`,
   `rdp-backend.so`, `vnc-backend.so`, `pipewire-backend.so`,
   `gl-renderer.so`, `color-lcms.so`, `xwayland.so`,
   `remoting-plugin.so`, `pipewire-plugin.so`.
5. **libweston 14 exports the config parser.** Upstream links `shared/`
   into libweston with `link_whole`, and `config-parser.c` carries
   `WL_EXPORT`, so `weston_config_*` symbols come from the RPM's
   `libweston-14.so`. **Do not vendor `shared/config-parser.c`.**
   `os-compatibility.c`, `process-util.c` and `option-parser.c` have no
   `WL_EXPORT` and must be vendored. `parse_options()` is declared in the
   installed `<libweston/config-parser.h>`; only its implementation needs
   vendoring.
6. **Build deps**: `meson` 1.7 and `libinput-devel` live in **CRB** (must
   be enabled). `wayland-devel` 1.25, `wayland-protocols-devel` 1.49,
   `libevdev-devel`, `gcc` 14 are in AppStream. `xorg-x11-server-Xwayland`
   24.1 is in AppStream.
7. **Runtime needs CRB too**: EPEL's `weston-libs` depends on `neatvnc`,
   which pulls `libturbojpeg` from CRB. A pristine install must enable
   CRB before `dnf install`.
8. Between tags 14.0.1 and 14.0.2, only two files in the vendored set
   changed (verify with
   `git -C <weston> log 14.0.1..14.0.2 -- frontend/ desktop-shell/ shared/ protocol/weston-desktop-shell.xml`):
   - `frontend/main.c` — upstream commit `51dfd1be` "frontend: Fix crash
     in output resize handler". We vendor this file → backport as **P0**.
   - `shared/config-parser.c` — upstream commit `ee92a531` "shared: fix
     binding-modifier none". This lives inside the RPM's libweston, so
     the bug is present at runtime and cannot be fixed from our side.
     Document as a known limitation (`binding-modifier=none` is broken
     until EPEL rebases). It becomes moot after T5 removes the option.
9. **`/tmp/.X11-unix` must exist before starting with `--xwayland`.**
   Real systems create it via systemd-tmpfiles; bare containers do not.
   If it is missing, the RPM's `xwayland.so` fails to bind and then
   **segfaults in its error path** — an upstream 14.0.1 teardown bug, not
   ours. Every script that starts Xwayland does `mkdir -p -m 1777 /tmp/.X11-unix`.

Package versions observed in the build image (for your sanity check;
newer is fine): `weston-devel-14.0.1-3.el10_0`, `weston-libs-14.0.1-3.el10_0`,
`meson-1.7.2-1.el10`, `gcc-14.4.1-1.el10`, `wayland-devel-1.25.0-1.el10`,
`wayland-protocols-devel-1.49-2.el10`, `libinput-devel-1.30.1-2.el10`,
`libevdev-devel-1.13.1-6.el10`, `xorg-x11-server-Xwayland-24.1.9-6.el10`.

---

## 3. Final repository layout

```
example-weston-standalone/
├── .github/workflows/ci.yml       # §9.4
├── COPYING                        # upstream MIT text (verbatim)
├── LICENSE                        # repo MIT license
├── PLAN.md                        # the port plan (living doc; phases ticked off)
├── README.md                      # §9.1
├── VENDOR.md                      # provenance + patch/trim log (§5.3)
├── containers/Containerfile.build # §4.2
├── data/
│   ├── meson.build
│   ├── westonite.desktop
│   └── westonite.ini.example
├── desktop-shell/
│   ├── meson.build
│   ├── shell.c                    # 2239 lines after T9
│   └── shell.h                    # 108 lines after T9
├── docs/
│   ├── phase0-findings.md
│   ├── desktop-shell-capabilities.md
│   ├── frontend-capabilities.md
│   ├── maintenance-layer-plan.md  # deferred design (D-MAINT)
│   └── e2e-test-plan.md
├── frontend/
│   ├── meson.build
│   ├── config-helpers.c  executable.c  main.c  weston-screenshooter.c  xwayland.c
│   ├── weston.h  weston-private.h
├── git-version.h.meson
├── meson.build
├── meson_options.txt
├── rpm/westonite.spec
├── scripts/
│   ├── smoke-test.sh  e2e-test.sh  rpm-build.sh  rpm-install-test.sh
├── shared/
│   ├── meson.build
│   ├── option-parser.c  os-compatibility.c  os-compatibility.h  process-util.c  process-util.h
│   └── fd-util.h  helpers.h  string-helpers.h  timespec-util.h  xalloc.h
└── tests/e2e/
    ├── .gitignore  pytest.ini  conftest.py
    ├── clients/meson.build  wtest-client.c  wtest-xclient.c
    ├── support/compositor.py  client.py  image.py  vncclient.py
    └── test_lifecycle.py  test_cli.py  test_children.py  test_outputs.py
        test_shell_background.py  test_shell_windows.py  test_xwayland.py
```

Files that exist at Phase 1 but are deleted by the trims:
`frontend/text-backend.c` (T2), `desktop-shell/input-panel.c` (T2),
`protocol/weston-desktop-shell.xml` and `protocol/meson.build` (T3).

---

## 4. Phase 0 — environment truth

**Goal.** Confirm §2 against the live repositories and produce the build
container.

### 4.1 Steps

1. Query package availability for `weston*` in CentOS Stream 10
   AppStream/CRB, public UBI 10, and EPEL 10 (`dnf repoquery` inside a
   `quay.io/centos/centos:stream10` container after
   `dnf -y install epel-release && dnf config-manager --set-enabled crb`).
   Record versions and the file lists of `weston-devel` and
   `weston-libs` (`dnf repoquery -l`).
2. Confirm §2 items 3–7.
3. Confirm §2 item 8 in the upstream tree.
4. Write `containers/Containerfile.build` (§4.2) and build it. Its final
   `RUN` is a sanity check that fails the image build if the libweston
   dev environment is unusable.
5. Write `docs/phase0-findings.md` recording all of the above (headline
   results, RPM contents, dependency version table, decisions D1–D3
   below, container validation output, risk status).

Decisions recorded in that document:

- **D1** — import base is tag `14.0.1` (not 14.0.2); P0 backported.
- **D2** — build image defaults to `quay.io/centos/centos:stream10` + EPEL
  + CRB; `--build-arg BASE_IMAGE=registry.access.redhat.com/ubi10/ubi`
  works on an entitled host (UBI containers inherit the host
  subscription; the Containerfile falls back to `subscription-manager
  repos --enable codeready-builder-for-rhel-10-$(arch)-rpms` when
  `dnf config-manager --set-enabled crb` fails).
- **D3** — drop patch P1; meson still probes each backend header and
  fails with a clear message if the RPM ever stops shipping one.

### 4.2 `containers/Containerfile.build` (final form)

The package list below is the final one. Packages were added over time
(`xorg-x11-server-Xwayland-devel` in Phase 3, `xdpyinfo` in Phase 5,
`python3-pytest`/`python3-cryptography` in E1, `wayland-utils` in E2,
`libxcb-devel` in E4); adding them all up front is fine.

```dockerfile
# Build/test image for westonite (Phase 0 deliverable).
#
# Default base is CentOS Stream 10 — the RHEL 10 content set without
# subscription entitlement, suitable for CI. On a subscription-entitled
# host the same file builds on UBI 10:
#
#   docker build -f containers/Containerfile.build \
#     --build-arg BASE_IMAGE=registry.access.redhat.com/ubi10/ubi .
#
# (UBI containers inherit the host's RHEL entitlement; EPEL and CRB
# — "codeready-builder-for-rhel-10-$(arch)-rpms" — resolve through it.)
#
# weston 14.0.1 comes from EPEL 10; meson and libinput-devel come from CRB.
# See docs/phase0-findings.md for the repository research behind this.

ARG BASE_IMAGE=quay.io/centos/centos:stream10
FROM ${BASE_IMAGE}

RUN dnf -y install epel-release \
    && (dnf config-manager --set-enabled crb \
        || subscription-manager repos --enable "codeready-builder-for-rhel-10-$(arch)-rpms") \
    && dnf -y install \
        gcc \
        meson \
        ninja-build \
        pkgconf-pkg-config \
        git-core \
        weston-devel \
        weston-libs \
        wayland-devel \
        wayland-protocols-devel \
        libinput-devel \
        libevdev-devel \
        libxkbcommon-devel \
        rpm-build \
        rpmdevtools \
        xorg-x11-server-Xwayland \
        xorg-x11-server-Xwayland-devel \
        xdpyinfo \
        python3-pytest \
        python3-cryptography \
        wayland-utils \
        libxcb-devel \
    && dnf clean all

# Sanity marker used by CI/smoke scripts: fail the image build early if the
# libweston 14 development environment is not actually usable.
RUN pkg-config --exists 'libweston-14 >= 14.0.1' \
    && test -e /usr/lib64/libweston-14/headless-backend.so \
    && test -e /usr/lib64/libweston-14/xwayland.so \
    && test -e /usr/include/libweston-14/libweston/xwayland-api.h

WORKDIR /src
```

Build: `docker build -f containers/Containerfile.build -t westonite-build .`

### 4.3 Gate

Image builds; the sanity `RUN` passes; `docs/phase0-findings.md`
committed.

---

## 5. Phase 1 — verbatim import + meson build + P0

**Goal.** `westonite --backend=headless` starts from the vendored
sources, loads the RPM's `headless-backend.so` and our `desktop-shell.so`
from `/usr/lib64/westonite/`, and exits 0 on SIGTERM.

### 5.1 Import (verbatim, from upstream tag `14.0.1`)

| Destination | Upstream path |
|---|---|
| `frontend/main.c`, `executable.c`, `text-backend.c`, `config-helpers.c`, `weston-screenshooter.c`, `xwayland.c`, `weston.h`, `weston-private.h` | `frontend/` (same names) |
| `desktop-shell/shell.c`, `shell.h`, `input-panel.c` | `desktop-shell/` (same names) |
| `shared/os-compatibility.c`, `os-compatibility.h`, `process-util.c`, `process-util.h`, `option-parser.c` | `shared/` |
| `shared/helpers.h`, `string-helpers.h`, `xalloc.h`, `timespec-util.h`, `fd-util.h` | `shared/` |
| `protocol/weston-desktop-shell.xml` | `protocol/weston-desktop-shell.xml` |
| `git-version.h.meson` | `libweston/git-version.h.meson` |
| `COPYING` | `COPYING` |

Do **not** import: `frontend/screen-share.c`, `frontend/systemd-notify.c`,
anything under `clients/`, `shared/config-parser.c`, anything under
`libweston/`. If the compiler demands another `shared/*.h`, import it and
add it to the table; "let the build drive completion" was the rule, and
the five headers above turned out to be the complete set.

Copy `weston.ini.in`/`weston.desktop` only as starting points for
`data/westonite.ini.example` and `data/westonite.desktop` (Phase 2); they
are not vendored verbatim.

Sizes at import, for orientation: `main.c` 4838 lines, `shell.c` 5029,
`input-panel.c` 425, `text-backend.c` 1120, `xwayland.c` 267,
`weston-screenshooter.c` 153, `config-helpers.c` 94, `executable.c` 34,
`shell.h` 201, `weston.h` 118, `weston-private.h` 55.

### 5.2 Patch P0

Apply upstream commit `51dfd1be` ("frontend: Fix crash in output resize
handler") to `frontend/main.c`. It is the only 14.0.1→14.0.2 change to a
vendored file. In the finished tree the guarded code is the mirror-of
resize path: `simple_heads_output_sharing_resize()` calls
`wet_config_find_head_to_mirror()` and **returns early if it yields
NULL** before touching `head_to_mirror->output`. Confirm the hunk against
`git -C <weston> show 51dfd1be`. Log it in `VENDOR.md` as P0.

### 5.3 `VENDOR.md`

Create it now and keep it current for the rest of the plan. Contents:

- Source URL, tag `14.0.1`, its commit hash, why (matches EPEL RPM
  `weston-14.0.1-3.el10_0`), license.
- The import table above ("verbatim unless listed under Local patches").
- The not-imported list with reasons.
- A note that all `meson.build` files and `meson_options.txt` are written
  for this repo (modelled on upstream's, not copies).
- `## Local patches` — one bullet per patch/trim, appended as they land.
- `## Rebasing to a newer 14.0.x` — the three-step procedure:
  1. `git -C <weston> diff 14.0.1..<new-tag> -- frontend/ desktop-shell/ shared/`
     (after T2/T3, hunks touching `protocol/`, `input-panel.c` or
     `text-backend.c` have nothing to apply to and are skipped);
  2. apply the hunks touching imported files (drop P0 if superseded);
  3. update the tag/commit, the version in `meson.build` and
     `rpm/westonite.spec`; rebuild; rerun the smoke tests.

### 5.4 Build design (meson)

Mirror upstream's target structure exactly:

- `shared/` → static `libshared` (the three `.c` files).
- `frontend/` → `libexec_westonite.so` (installed to `$libdir/westonite/`)
  + `westonite` executable built from `executable.c` with an rpath to the
  module dir.
- `desktop-shell/` → `desktop-shell.so` in `$libdir/westonite/` — same
  filename as upstream but a different directory, so no conflict. **One
  deviation from upstream: an explicit `pixman-1` dependency** for
  `desktop-shell.so`. Upstream gets pixman transitively; the RPM's
  `libweston-14.pc` keeps pixman in `Requires.private`, so the link fails
  without it.
- `protocol/` → wayland-scanner generation of `weston-desktop-shell`
  (vendored XML) and `text-input-unstable-v1` +
  `input-method-unstable-v1` (XMLs from `wayland-protocols-devel`).
  This whole directory is deleted at T3.
- `config.h` via `configuration_data()` providing exactly the macros the
  vendored files use. The full root `meson.build` is in Appendix A.1;
  the Phase-1 version differs from the final one in exactly three places:
  it contains `config_h.set_quoted('WESTON_SHELL_CLIENT', get_option('shell-client-default'))`
  (removed at T3), `subdir('protocol')` (removed at T3), and it lacks the
  `e2e-test-client` block (added at E3).
- Version: `project('westonite', 'c', version: '14.0.1', …)`; the git
  build id comes from `vcs_tag` over `git-version.h.meson`.
- Backend probe: for each of `drm headless pipewire rdp vnc wayland x11`,
  `cc.has_header('libweston/backend-X.h', dependencies: dep_libweston)`
  sets `BUILD_X_COMPOSITOR`, else `error(…)`. This is the P1 tripwire.
- `HAVE_XWAYLAND_LISTENFD` is set when `pkg-config xwayland` reports
  `have_listenfd=true` (needs `xorg-x11-server-Xwayland-devel`, Phase 3).
- `LIBWESTON_MODULEDIR` = `<libweston libdir>/libweston-14` — where the
  frontend loads `xwayland.so` from.

`meson_options.txt` at Phase 1: `backend-default` (combo, default `drm`),
`xwayland` (bool, default true), `xwayland-path` (string,
`/usr/bin/Xwayland`), and — until T3 — `shell-client-default` (string,
default empty; see P3). The `e2e-test-client` option is added at E3.

All per-directory `meson.build` files are in Appendix A.2–A.5. The
Phase-1 `desktop-shell/meson.build` lists `shell.c`, `input-panel.c`, and
the four generated protocol targets (`weston_desktop_shell_*`,
`input_method_unstable_v1_*`); the Phase-1 `frontend/meson.build` lists
`text-backend.c` and the four `text_input_unstable_v1_*` /
`input_method_unstable_v1_*` targets in `srcs_westonite`. Both shrink at
T2/T3 to the Appendix versions.

### 5.5 Gate (run inside the container)

```sh
meson setup build --prefix=/usr && ninja -C build && ninja -C build install
export XDG_RUNTIME_DIR=/tmp/xdg; mkdir -p -m 0700 $XDG_RUNTIME_DIR
timeout --preserve-status 5 westonite --backend=headless --log=/tmp/w.log; echo $?   # 0
grep "Loading module '/usr/lib64/westonite/desktop-shell.so'" /tmp/w.log
grep "headless-backend.so" /tmp/w.log
```

Commit: "Phase 1: verbatim import + meson build + P0" (or fold Phases 1–3
into one commit as the original did; the record shows Phases 0–3 landed
together as the first content commit, with Phases 4 and 5 as separate
commits).

---

## 6. Phase 2 — `westonite` identity + no helper clients

**Goal.** Config file is `westonite.ini`; nothing is spawned at startup;
clean exit with and without a config file.

### 6.1 Patch P2 — config filename

In `frontend/main.c`, change the default config lookup string from
`weston.ini` to `westonite.ini` (the `const char *file = "weston.ini";`
in the config-loading function) and the two usage-text lines:

```
  -c, --config=FILE	Config file to load, defaults to westonite.ini
  --no-config		Do not read westonite.ini
```

Same XDG search logic; `--config` unaffected. Leave the other
`weston.ini` mentions in log strings alone (they are libweston-style
messages such as "requires color-management=true in weston.ini" and were
not touched). `WESTON_CONFIG_FILE` is still exported to children.

### 6.2 Patch P3 — no panel client (superseded by T3, but required now)

`shell.c` unconditionally idle-spawns `shell->client` (default
`WESTON_SHELL_CLIENT`). Change `shell_configuration()` so that an
**empty** `[shell] client=` value (or an empty `WESTON_SHELL_CLIENT`
default, set via the `shell-client-default` meson option, default empty)
means "do not spawn": no spawn is scheduled, and the startup fade is
triggered immediately instead of waiting on the 15 s `desktop_ready`
timeout. The shape used:

```c
if (strlen(WESTON_SHELL_CLIENT) > 0)
	client = wet_get_libexec_path(WESTON_SHELL_CLIENT);
else
	client = strdup("");
weston_config_section_get_string(section, "client", &s, client);
free(client);
shell->client = s;
```

plus two guards in `wet_shell_init()`:

```c
loop = wl_display_get_event_loop(ec->wl_display);
if (shell->client && shell->client[0] != '\0')
	wl_event_loop_add_idle(loop, launch_desktop_shell_process,
			       shell);
/* ... */
/* With no helper client there is nothing to send desktop_ready;
 * fade in right away instead of waiting for the 15 s timeout. */
if (!shell->client || shell->client[0] == '\0')
	shell_fade_startup(shell);
```

Users can still point `client=` at
`/usr/libexec/weston-desktop-shell` from EPEL's `weston` package.

### 6.3 Patch P4 — no on-screen keyboard (superseded by T2)

In `frontend/text-backend.c`, `[input-method] path` defaulted to
`$libexecdir/weston-keyboard`. Default it to `""` (empty was already
handled as "no input method"):

```c
/* No input-method client by default: westonite does not ship
 * weston-keyboard. Set [input-method] path to enable one. */
weston_config_section_get_string(section, "path",
				 &text_backend->input_method.path, "");
```

### 6.4 Data files

`data/westonite.desktop` (installed to `$datadir/wayland-sessions/`):

```ini
[Desktop Entry]
Name=Westonite
Comment=Standalone Weston-based Wayland compositor
Exec=westonite
Type=Application
```

`data/westonite.ini.example` (installed to `$datadir/doc/westonite/`).
The Phase-2 version documents `[core] backend`, `idle-time`, `xwayland`;
`[shell] background-color` (example value `0xff103040`; at this stage
the C shell does **not** read it — only a helper client would — and
T3 is what makes the shell read it, default `0xff002244`), `client=`
(how to get the panel back), `startup-animation=none`;
`[input-method] path`; `[keyboard] keymap_layout`. After T3–T5 it shrinks
to the final form in Appendix A.7.

`data/meson.build` installs both (Appendix A.5).

### 6.5 Gate

- `westonite --backend=headless` with no config: no `launching` line in
  the log, exit 0.
- With `$XDG_CONFIG_HOME/westonite.ini` present: log contains
  `Using config file '<path>/westonite.ini'`, exit 0.

Log P2, P3, P4 in `VENDOR.md`.

---

## 7. Phase 3 — Xwayland

**Goal.** `westonite --backend=headless --xwayland` publishes an X display,
lazily spawns `/usr/bin/Xwayland` on the first X connection, and
`xdpyinfo` completes a round trip.

Steps:

1. Add `xorg-x11-server-Xwayland-devel` to the build image so
   `pkg-config xwayland` exists and `have_listenfd` is detected →
   `HAVE_XWAYLAND_LISTENFD`, which avoids Xwayland's deprecated `-listen`
   fd path.
2. `mkdir -p -m 1777 /tmp/.X11-unix` in the container (§2 item 9).
3. Verify:

```sh
westonite --backend=headless --xwayland --log=/tmp/w3.log &
sleep 2
DISP=$(grep -oP 'listening on display \K:[0-9]+' /tmp/w3.log)
DISPLAY=$DISP xdpyinfo | grep 'vendor string'
kill %1; wait %1; echo $?                       # 0
grep "launching '/usr/bin/Xwayland'" /tmp/w3.log
```

Note the two facts for the README: the display is advertised **before**
the server is spawned (lazy spawn on first connection), and
`/tmp/.X11-unix` must exist.

---

## 8. Phase 4 — RPM

**Goal.** `rpmbuild -ba rpm/westonite.spec` produces
`westonite-14.0.1-1.el10.x86_64.rpm` (+debuginfo/debugsource) in the build
container, and installing it in a **pristine** CentOS Stream 10 container
pulls `weston-libs` (not the full `weston`) and passes the headless +
Xwayland smoke checks from the installed files.

The spec is in Appendix A.8. Design points:

- `Name: westonite`, `Version: 14.0.1`, `Release: 1%{?dist}`, `License: MIT`,
  `Source0: %{name}-%{version}.tar.gz` (from `git archive`).
- `BuildRequires`: `gcc`, `meson >= 0.63`, `pkgconfig(libweston-14) >= 14.0.1`,
  `pkgconfig(wayland-server)`, `pkgconfig(wayland-scanner)`,
  `pkgconfig(wayland-protocols) >= 1.33`, `pkgconfig(libinput)`,
  `pkgconfig(libevdev)`, `pkgconfig(pixman-1)`, `pkgconfig(xwayland)`.
- `Requires: weston-libs%{?_isa} >= 14.0.1`; `Recommends: xorg-x11-server-Xwayland`.
  The full `weston` package is **not** required.
- `%build`/`%install` are the stock `%meson`, `%meson_build`,
  `%meson_install` macros.
- `%files`: `%{_bindir}/westonite`, `%dir %{_libdir}/westonite`,
  `%{_libdir}/westonite/desktop-shell.so`,
  `%{_libdir}/westonite/libexec_westonite.so`,
  `%{_libdir}/westonite/libexec_westonite.so.*`,
  `%{_datadir}/wayland-sessions/westonite.desktop`,
  `%{_datadir}/doc/westonite/westonite.ini.example`, `%license COPYING`,
  `%doc VENDOR.md`. No devel subpackage.

Build procedure (becomes `scripts/rpm-build.sh` in Phase 5):

```sh
git archive --prefix=westonite-14.0.1/ -o /tmp/rpm/SOURCES/westonite-14.0.1.tar.gz HEAD
rpmbuild --define '_topdir /tmp/rpm' -ba rpm/westonite.spec
```

Pristine install test (becomes `scripts/rpm-install-test.sh`): in a fresh
`quay.io/centos/centos:stream10` container, `dnf -y install epel-release`,
**enable CRB** (runtime need, §2 item 7), `dnf -y install <the rpm>
xorg-x11-server-Xwayland xdpyinfo`, assert `rpm -q weston` fails, then
run the headless + `--xwayland` + `xdpyinfo` check from §7.

Gate: both pass. Commit "Phase 4: add westonite.spec; RPM built and
install-tested".

---

## 9. Phase 5 — README, scripts, CI

### 9.1 README.md

Sections, in order: what westonite is (frontend + desktop-shell, built
against libweston 14 RPMs, renamed; upstream base `14.0.1` matching EPEL's
`weston-14.0.1-3.el10_0`; target platform; links to `VENDOR.md`,
`docs/phase0-findings.md`, `PLAN.md`); what you get (the three installed
artifacts, session file, example config; backends/renderers/Xwayland
module load from EPEL's `weston-libs` at `/usr/lib64/libweston-14/`); the
no-helper-clients default and how to point `[shell] client=` at EPEL's
binaries (this paragraph becomes stale at T3 — update it then to say the
panel cannot be re-enabled); **Required repositories** table (BaseOS/
AppStream, CRB, EPEL 10 — build vs runtime, what each provides, including
the CRB-at-runtime libturbojpeg note); **Building** (docker build, meson
setup/ninja/install); **Building the RPM**; **Running** (`westonite`,
`--backend=headless`, `--xwayland`; config search; the `/tmp/.X11-unix`
note); **Testing** (added at E5, §12.7); **License** (MIT/Expat).

### 9.2 `scripts/smoke-test.sh`

Runs inside the build image as root; builds from `/src`, installs, then
three checks. Final form (after T3 changed smoke 2 to exercise
`background-color`) is in Appendix A.9. The three checks:

1. headless, no config: exit 0 within `timeout --preserve-status 5`;
   log contains `Loading module '/usr/lib64/westonite/desktop-shell.so'`;
   log does **not** contain `launching`.
2. `$XDG_CONFIG_HOME/westonite.ini` with `[shell] background-color=0xff336699`
   is honoured: log contains `Using config file '/tmp/cfg/westonite.ini'`.
3. Xwayland round trip exactly as in §7.

### 9.3 `scripts/rpm-build.sh` and `scripts/rpm-install-test.sh`

Appendix A.10 and A.11. `rpm-install-test.sh` grows at E4 (session-file
validation + the installed e2e subset); the appendix shows the final
form.

### 9.4 `.github/workflows/ci.yml`

One job, `build-and-test`, on `ubuntu-latest`:

1. checkout
2. `docker build -f containers/Containerfile.build -t westonite-build .`
3. `docker run --rm -v "$PWD":/src westonite-build /src/scripts/smoke-test.sh`
4. (E1+) e2e suite with a mounted results dir
5. `rpm-build.sh /out` with `out/` mounted
6. `rpm-install-test.sh /rpms /src /results` in a bare
   `quay.io/centos/centos:stream10` container with `out/` mounted read-only
   at `/rpms` and the repo read-only at `/src`
7. upload `out/*.rpm` as artifact `westonite-rpms`
8. (E5+) upload `test-results/` as `test-results`, `if: always()`

Triggers (after the hardening fix in §13): `push: branches: [main]` and
`pull_request:`. The final file is Appendix A.12.

Gate: the workflow is green on the pushed commit.

---

## 10. Trim series T1–T9 — desktop-shell becomes a pure window manager

Each trim is **one commit**, ends with a **zero-warning build** and the
**full smoke suite passing** in the container, and adds a `VENDOR.md`
entry. Update `docs/desktop-shell-capabilities.md` (§11.1) as you go —
the original rewrote it after T4 and refreshed its line anchors after
each later trim. Line counts below are from the original commits; treat
them as targets, not exact requirements.

The end state, to aim at from the start:

- `shell.c` 2239 lines, `shell.h` 108 lines.
- `[shell]` config section contains exactly one key: `background-color`.
- `shell_desktop_api` has exactly these 10 entries: `surface_added`,
  `surface_removed`, `committed`, `move`, `resize`, `set_parent`,
  `ping_timeout`, `pong`, `set_xwayland_position`, `get_position`.
- `shell_add_bindings()` registers exactly: `BTN_LEFT` and `BTN_RIGHT`
  button bindings → `click_to_activate_binding`, one touch binding →
  `touch_to_activate_binding`, and `weston_install_debug_key_binding(ec, MODIFIER_SUPER)`.
- The shell creates **zero** `wl_global`s and generates no
  wayland-scanner code.
- The shell calls exactly two symbols from `libexec_westonite.so`:
  `wet_get_config()` and `screenshooter_create()`.

### T1 — remove the `weston_screensaver` interface

`protocol/weston-desktop-shell.xml`: delete the `weston_screensaver`
interface (never implemented or advertised by `shell.c`; upstream keeps it
as a fossil). XML goes 135 → 119 lines. No C changes.

### T2 — remove input-panel / on-screen-keyboard support

- Delete `desktop-shell/input-panel.c` (the `input_panel` surface role)
  and `frontend/text-backend.c` (the text-input-unstable-v1 ↔
  input-method-unstable-v1 wiring; note `shell.c` owned its lifecycle via
  `text_backend_init()`/`text_backend_destroy()`).
- `shell.c`/`shell.h`: remove the input-panel layer, `text_input` state,
  their listeners, and the `text_backend_init`/`destroy` calls
  (`shell.c` −16, `shell.h` −22 lines).
- `frontend/weston.h`: remove the `struct text_backend` forward
  declaration and the two prototypes.
- `protocol/meson.build`: drop the `text_input_unstable_v1_*` and
  `input_method_unstable_v1_*` custom targets; `frontend/meson.build` and
  `desktop-shell/meson.build` stop listing them and `text-backend.c` /
  `input-panel.c`.
- P4 is now moot (its file is gone); keep its `VENDOR.md` entry as history.

### T3 — remove the helper-client protocol, lock screen, idle handling, fades

The largest trim (`shell.c` +66/−988 lines; `shell.h` +2/−46; 92
insertions and 1201 deletions across the repo). Delete:

- `protocol/weston-desktop-shell.xml`, `protocol/meson.build`, and
  `subdir('protocol')` in the root `meson.build` (this supersedes T1).
- In `shell.c`: the `weston_desktop_shell` global, bind/unbind, client
  launch/respawn/crash handling (`launch_desktop_shell_process`,
  `respawn_desktop_shell_process`, `desktop_shell_client_destroy`,
  `check_desktop_shell_crash_too_early`), every request handler
  (background, panel, panel position, lock surface, grab surface,
  `desktop_ready`), `lock()`/`unlock()`/`resume_desktop()`, the idle/wake
  listeners (**displays never sleep now** — product decision), the screen
  fade machinery (curtains, `shell_fade*`, startup fade), the panel layer
  with its work-area and move-constraint logic (`get_output_work_area()`
  stays but now always returns the full output), grab-cursor feedback,
  and the P3 patch (nothing left to spawn).
- `meson_options.txt`: the `shell-client-default` option;
  `meson.build`: the `WESTON_SHELL_CLIENT` define.

Add — **the compositor-side background**, one solid `weston_curtain` per
output, created in `create_shell_output()`, recreated on output resize,
destroyed with the output:

```c
struct shell_output {
	struct desktop_shell  *shell;
	struct weston_output  *output;
	struct wl_listener    destroy_listener;
	struct wl_list        link;
	struct weston_curtain *background_curtain;
};

/* in shell_configuration(): */
config = wet_get_config(shell->compositor);
section = weston_config_get_section(config, "shell", NULL, NULL);
weston_config_section_get_color(section, "background-color",
				&shell->background_color, 0xff002244);

static int
background_get_label(struct weston_surface *surface, char *buf, size_t len)
{
	return snprintf(buf, len, "solid background for output %s",
			surface->output ? surface->output->name : "NULL");
}

static void
background_committed(struct weston_surface *es,
		     struct weston_coord_surface new_origin)
{
}

static void
shell_output_recreate_background(struct shell_output *shell_output)
{
	struct desktop_shell *shell = shell_output->shell;
	struct weston_output *output = shell_output->output;
	uint32_t color = shell->background_color;
	struct weston_curtain_params params = {
		.r = ((color >> 16) & 0xff) / 255.0,
		.g = ((color >> 8) & 0xff) / 255.0,
		.b = (color & 0xff) / 255.0,
		.a = ((color >> 24) & 0xff) / 255.0,
		.pos = output->pos,
		.width = output->width, .height = output->height,
		.surface_committed = background_committed,
		.get_label = background_get_label,
		.surface_private = shell_output,
		.capture_input = true,
	};

	if (shell_output->background_curtain)
		weston_shell_utils_curtain_destroy(shell_output->background_curtain);

	shell_output->background_curtain =
		weston_shell_utils_curtain_create(shell->compositor, &params);
	weston_view_set_output(shell_output->background_curtain->view, output);
	weston_view_move_to_layer(shell_output->background_curtain->view,
				  &shell->background_layer.view_list);
}
```

`handle_output_resized()` (listener on `output_resized_signal`) finds the
`shell_output` and calls `shell_output_recreate_background()`. The
curtain lives in the dedicated `background_layer` and captures input, so
clicks on empty desktop go nowhere. (Note: 14's `weston_curtain_params`
has a `get_label` callback; newer upstream has a `char *label` — do not
copy from a newer tree.)

This also fixes the undefined-framebuffer content the clientless
configuration previously showed on real hardware. Update smoke check 2 to
set `background-color=0xff336699` and the example config accordingly.

### T4 — remove all remaining animations

Remove window open (`zoom`/`fade`) and close (`fade`) animations
including the anim-fade surface refs in `desktop_surface_removed`/
`committed`, the focus dim-layer animation with its per-workspace
focus-surface curtains, `get_animation_type()`, and the `animation`,
`close-animation`, `startup-animation`, `focus-animation` config keys
(stale keys in existing configs are ignored harmlessly by the parser).
`shell.c` −252, `shell.h` −20.

### T5 — remove every hotkey binding

Per-binding decisions, all "remove": zap (Ctrl+Alt+Backspace, with the
`allow-zap` option), mod+drag move and resize triggers, maximize/
fullscreen toggles, tiled-snap (with the whole orientation state and its
save/restore), rotate (grab, matrices, saved-rotation restore, and the
busy-grab right-click rotate), the mod+Tab switcher, force-kill,
backlight control (fixed keys and mod+F9/F10), surface opacity, the
debug-key chain, and the now-meaningless `binding-modifier` option.
`shell_add_bindings()` is left registering only pointer/touch/tablet
click-to-activate. `shell.c` 3841 → 3070 lines.

**Then, as a separate commit, restore the debug-key chain**: add
`weston_install_debug_key_binding(ec, MODIFIER_SUPER);` back to
`shell_add_bindings()` with the modifier hardcoded (Super+Shift+Space
followed by a debug key toggles libweston's runtime debug aids). Record
this in the T5 `VENDOR.md` entry.

### T6 — remove fullscreen and maximize

Drop `.fullscreen_requested` and `.maximized_requested` from
`shell_desktop_api`. libweston-desktop then omits both from
`xdg_toplevel.wm_capabilities` (v5-aware toolkits hide the buttons) and
its NULL-callback guards ignore requests from legacy clients. Delete the
now-unreachable machinery: `set_fullscreen`/`unset_fullscreen`/
`set_maximized`/`unset_maximized`, the black letterbox curtains, the
fullscreen layer and `lower_fullscreen_layer()`, the maximize
sizing/positioning helpers, the shared saved-position restore, the
`surface_state` tracking struct, max/fullscreen guards in the
move/resize/touch/tablet grabs, and the output-resize window re-fitting.
Windows are free-floating and client-sized only. The frontend
`--fullscreen` option (nested-backend window size) is unrelated and
untouched. `shell.c` 3072 → 2592.

### T7 — remove tablet-tool (pen) window management

Delete the pen tap-to-activate binding, the tablet-tool move grab and its
branch in `desktop_surface_move`, the `shell_tablet_tool_grab` helpers,
and the per-seat tool tracking and focus-ping listeners. Pens remain fully
functional inside applications (tablet-v2 delivery is libweston core).
Touch keeps tap-to-activate and window dragging. `shell.c` 2592 → 2307.

### T8 — busy-cursor grab: no move on click

In the busy-cursor grab's button handler, a left click now only activates
the unresponsive window; remove the `surface_move()` call. The grab and
ping/pong unresponsiveness detection remain. (4-line change.)

### T9 — remove minimize

Drop `.minimized_requested` from `shell_desktop_api` (same mechanism as
T6). Delete `set_minimized`, the minimized layer, and the orphaned
`surface_keyboard_focus_lost`/`drop_focus_state` helpers only
minimization used. `shell.c` 2306 → 2239. `wm_capabilities` now advertises
nothing.

### Final `shell.h` (for comparison)

```c
struct workspace {
	struct weston_layer layer;
	struct wl_list focus_list;
	struct wl_listener seat_destroyed_listener;
};

struct shell_output {
	struct desktop_shell  *shell;
	struct weston_output  *output;
	struct wl_listener    destroy_listener;
	struct wl_list        link;
	struct weston_curtain *background_curtain;
};

struct weston_desktop;
struct desktop_shell {
	struct weston_compositor *compositor;
	struct weston_desktop *desktop;
	const struct weston_xwayland_surface_api *xwayland_surface_api;

	struct wl_listener transform_listener;
	struct wl_listener resized_listener;
	struct wl_listener destroy_listener;
	struct wl_listener session_listener;

	struct weston_layer background_layer;
	struct wl_listener pointer_focus_listener;
	struct workspace workspace;

	struct wl_listener seat_create_listener;
	struct wl_listener output_create_listener;
	struct wl_listener output_move_listener;
	struct wl_list output_list;
	struct wl_list seat_list;
	struct wl_list shsurf_list;

	uint32_t background_color;
	struct timespec startup_time;
};
```

Exported helpers that survive: `get_default_output`, `get_default_view`,
`get_shell_surface`, `get_current_workspace`, `get_output_work_area`,
`activate`, `shell_for_each_layer`. `shell.h` still includes
`<libweston/xwayland-api.h>` because of the Xwayland position sync.

---

## 11. Capability inventories and the deferred feature

### 11.1 `docs/desktop-shell-capabilities.md`

Written against the post-trim `shell.c`, with `file:line` anchors. Opening
paragraph states what westonite's shell is (a pure window-manager plugin:
no client-facing shell protocol, no helper clients, built-in solid
background, no hotkeys beyond the debug chain) and lists what upstream's
desktop-shell has that this one deliberately does not (T1–T9 in prose).
Then numbered groups, each a candidate unit for future trimming:

1. **Built-in background** — curtain per output, `[shell] background-color`.
2. **Core window management** (the `weston_desktop_api` vtable): surface
   lifecycle, client-initiated move/resize via the pointer and touch grab
   machinery (resize is pointer-only — upstream never implemented touch
   resize), transient/parent surfaces and child layer syncing, Xwayland
   integration (`set_xwayland_position`, `transform_handler` pushing view
   positions back to X windows), unresponsive-client handling (ping/pong,
   busy-cursor grab, no special cursor sprite), activation & focus
   (focus-state bookkeeping, pointer focus listener), stacking (cursor /
   workspace / background layers).
3. **Input bindings** — left/right click and touch tap to activate,
   Super+Shift+Space debug chain. `[shell]` config is `background-color`
   only.
4. **Multi-output / hotplug / session** — `shell_output` per output,
   output create/destroy/move handling, view repositioning when an output
   disappears, curtain resize, `desktop_shell_notify_session` (VT switch:
   re-syncs activation state).
5. **Seat management** — `shell_seat` per `weston_seat`, caps-changed,
   focus listeners.

Close with "Observations for any future trimming" (cheap self-contained
removals: busy-cursor grab, touch move grab; entangled: focus-state
tracking, Xwayland positioning; session-notify only matters on DRM).

### 11.2 `docs/frontend-capabilities.md`

Companion inventory of `frontend/` and `shared/` (sizes: `main.c` 4838,
`xwayland.c` 267, `weston-screenshooter.c` 153, `config-helpers.c` 94,
`executable.c` 34; `shared/` compiled subset 923 lines). Reminder of the
split: libweston (RPM) owns rendering, input delivery, protocol
implementation, backend internals, the xwayland module; the frontend owns
picking and configuring all of that. Sections:

1. Process & launch plumbing (binary split via `executable.c`; CLI/config
   basics; wayland socket naming; **autolaunch** — `westonite [--] /path/app`
   or `[autolaunch] path=` + `watch=`, which exits the compositor when the
   client exits: the kiosk primitive; child-process machinery).
2. Backend selection & configuration — a table of the seven backends with
   loader function and config surface.
3. Output management (`[output]` sections, clone-of, mirror-of, colour
   management/HDR, remoting/pipewire virtual-output plugin loaders).
4. Input configuration (`[keyboard]`, `[libinput]`, touch calibrator —
   whose interactive tool we do not ship).
5. Xwayland glue.
6. Screenshot / recording / debugging (Super+S spawns the unshipped
   `weston-screenshooter` client — a dead end at runtime; Super+R wcap
   recorder works but only upstream's unshipped `wcap-decode` reads the
   files; weston-log wiring: `--log`, `--debug`, `--logger-scopes`,
   `--flight-rec-scopes`).
7. Core misc (`--idle-time` is inert since T3; `modules=`; `--shell`;
   primary-client tracking; `config-helpers.c`).
8. `shared/` support code.

Then a **Drop assessment**: dead or semi-dead today (screenshooter Super+S
path, idle-time plumbing, touch calibrator enablement);
deployment-dependent questions (RDP/VNC/PipeWire backends, nested
backends — recommend keeping, `--backends`, clone/mirror, colour
management, autolaunch — keep, `--shell`/`modules=` genericity); keep
(DRM + headless, Xwayland, output basics, keyboard/libinput config,
logging, `shared/`, autolaunch). **None of these drops were executed in
this plan's scope**; the document records the questions.

### 11.3 `docs/maintenance-layer-plan.md` (design only — D-MAINT)

Status line: planned, not implemented, deferred indefinitely. Concept: a
"maintenance" `weston_layer` above the workspace, toggled by an operator
hotkey; windows there are fully interactive; while active, everything
below is dimmed by one translucent curtain per output (`capture_input =
true`) and blocked from input; while off, the layer is unpositioned so
its windows are unmapped (minimize-like, no client involvement). Layer
stack when active: cursor → maintenance → dim curtain → workspace →
background. Proposed controls: Super+M toggles the focused window between
workspace and maintenance; Super+Shift+M toggles the mode. Hotkeys are
the only way in or out (`minimized_requested` stays absent). Design
details: per-surface `in_maintenance` flag respected by
`shell_surface_calculate_layer_link`; focus hygiene on enter/leave;
transient children follow their parent via the existing child-layer
syncing; config `[shell] maintenance-dim=0xAARRGGBB` default `0x99000000`;
reuse `WESTON_LAYER_POSITION_UI` for the maintenance layer. Caveats to
resolve: Xwayland iconify behaviour (check `xwayland/window-manager.c`),
non-transient new toplevels from a maintenance app map to the workspace,
multi-seat (proposed: global mode state). Estimate ~200–250 lines in
`shell.c`. Sequencing: after T9; mode + dim + Super+Shift+M first, then
Super+M; log as trim-series entry F1.

---

## 12. E2E suite — phases E1–E5

Read §1.3 first; every design choice below follows from those decisions.
Write `docs/e2e-test-plan.md` before E1 (control plane, client stack,
runner design, test inventory tables, pixel determinism, CI wiring,
phases, risks) and keep it current: each phase's closeout and each
finding goes into it.

### 12.1 The VNC control plane (facts verified against the 14.0.1 sources and live)

Standard test invocation:

```
westonite --backend=vnc --renderer=pixman --width=W --height=H \
          --port=<per-instance> --disable-transport-layer-security \
          --log=<per-test log> [--no-config | with $XDG_CONFIG_HOME/westonite.ini]
```

- **Authentication is always required**, even with TLS disabled:
  `vnc.c` calls `nvnc_enable_auth(NVNC_AUTH_REQUIRE_AUTH, …)` backed by
  `weston_authenticate_user()` → PAM service **`weston-remote-access`**,
  and the client must authenticate *as the user running westonite*. The
  container therefore needs `/etc/pam.d/weston-remote-access` containing
  `auth required pam_unix.so` / `account required pam_unix.so`, a
  dedicated non-root user `e2e` with a real password, and the suite runs
  as that user via `runuser`. The setuid `unix_chkpwd` helper makes
  `pam_unix` work for a non-root process in the container.
- **Security types offered with TLS disabled (EPEL neatvnc 0.9.0)**:
  **129 (RSA-AES-256), 5 (RSA-AES), 30 (Apple DH)**. No classic VNC auth.
  `vncdotool` and `asyncvnc` both fail the handshake. Hence the in-repo
  client implementing **Apple DH** (type 30).
- **neatvnc is a single-client server**: a second connection kicks the
  first, and — found the hard way — a probe-connect that opens and closes
  the port is treated as a client whose teardown races the real client's
  connect. **Never probe the RFB port for readiness** (§13).
- The VNC listener starts during backend load, strictly before the
  wayland socket is created, so "wayland socket exists" implies "VNC is
  listening".
- One instance per test, on a free TCP port, with its own
  `XDG_RUNTIME_DIR`, `XDG_CONFIG_HOME`, log file and wayland socket.

### 12.2 The RFB client — `tests/e2e/support/vncclient.py`

Inline in full in Appendix A.14 (it is the non-obvious, hard-won piece).
Summary of what it does:

- Version handshake (`RFB 003.008`), security-type selection of 30,
  Apple DH: read generator (2 bytes), key length (2), prime, server
  public key; generate a random private key mod prime; compute the
  shared secret; AES key = MD5 of the shared secret (big-endian,
  key-length bytes); a 128-byte credential block (username in bytes
  0–63, password in 64–127, each NUL-terminated, unused bytes random)
  AES-128-ECB encrypted; send ciphertext + client public key; read the
  4-byte security result (non-zero → reason string).
- ClientInit shared=1; parse ServerInit (width, height, name).
- SetPixelFormat 32 bpp, depth 24, little-endian, true-colour, max
  255/255/255, shifts 16/8/0 → capture buffers are **BGRX** bytes.
- SetEncodings: Raw (0), DesktopSize (−223), ExtendedDesktopSize (−308).
- `capture()`: non-incremental FramebufferUpdateRequest for the whole
  frame; assemble Raw rects into a `width*height*4` buffer; absorb
  DesktopSize/ExtendedDesktopSize pseudo-rects (a geometry *change*
  re-requests; servers repeat the current layout in normal updates);
  handle SetColourMapEntries, Bell, ServerCutText.
- `pointer(x, y, buttons)`, `click`, `drag(x0,y0,x1,y1,button,steps)`,
  `key(keysym, down)`, `key_tap`.
- `set_desktop_size()` exists but is unusable (see §12.6).
- Socket timeout 30 s (see §13).

### 12.3 Harness — `conftest.py`, `support/compositor.py`, `support/client.py`, `support/image.py`

`conftest.py` (Appendix A.15): a `westonite` factory fixture that
launches `Westonite(tmp_path / "wN", **kw)` instances, waits for
readiness unless `wait=False`, and at teardown SIGTERMs every instance
still running, **asserting exit code 0**, killing on timeout, and
re-raising collected errors; plus a `pytest_runtest_makereport`
hookwrapper that, on a failed `call`, copies the test's `tmp_path` tree
(regular files only — runtime dirs contain sockets) into
`$WESTONITE_E2E_ARTIFACTS/<test name>/`.

`support/compositor.py` — class `Westonite`:

- ctor args: `extra_args=()`, `config=None` (ini text → written to
  `<tmp>/config/westonite.ini`, else `--no-config` unless
  `no_config_flag=False`), `backend="headless"`, `width=640`,
  `height=480` (`None` → no geometry flags), `env=None` (values of `None`
  unset a var), `socket_name=None`, `runtime_dir=None`.
- argv: `$WESTONITE_BIN` (default `westonite`), `--backend=`, `--log=`;
  for vnc add `--renderer=pixman --port=<free port>
  --disable-transport-layer-security`; `--width/--height` for
  vnc/headless; `--socket=` if given; then `extra_args`.
- env: `XDG_RUNTIME_DIR`, `XDG_CONFIG_HOME` set per instance;
  `WAYLAND_DISPLAY` removed.
- `wait_until(predicate, deadline=10, interval=0.1)` helper raising
  `Timeout(AssertionError)`.
- `log()`, `wait_for_log(regex, deadline)`, `wait_ready()` = wait until
  the process is alive **and a wayland socket exists** in the runtime dir
  (named socket if `socket_name`, else any `wayland-*` socket) — nothing
  else; `wayland_display` property; `x_display` property = the `:N` from
  `listening on display (:\d+)` in the log.
- `vnc()`: connect the RFB client as `$WESTONITE_VNC_USER` (default the
  current user) with `$WESTONITE_VNC_PASSWORD`; on `TimeoutError`,
  `OSError` or `RfbError`, assert the compositor is still alive, sleep 2 s,
  retry once.
- `client_env()`, `spawn(argv)`, `run_client(argv, timeout=15)` (asserts
  exit 0, returns captured output), `wait_exit(deadline=15)`,
  `terminate(sig, deadline=10)` (asserts exit 0, includes the log in the
  message), `kill()`.

`support/client.py` — class `WtestClient(compositor, *args, binary=None,
extra_env=None)`: spawns `$WTEST_CLIENT` (or `binary`) with the
compositor's client env, reads stdout on a daemon thread into a list;
`output()`, `count(regex)`, `wait_for_count(regex, n)`,
`wait_for_line(regex)` → match, `wait_mapped()` → `(w, h)` from
`mapped: WxH`, `pause()` (SIGUSR1, waits for `paused`), `resume()`
(SIGUSR2, waits for `resumed`), `terminate()`, context manager.

`support/image.py` (Appendix A.16): `pick_pixel(fb, w, rgb)`,
`region_of(fb, w, rgb)` → bounding box `(x, y, w, h)` of exact-colour
pixels (per-row `bytes.find` of the 3-byte BGR pattern, checking 4-byte
alignment), `wait_for_region(client, rgb)`, `solid_color(fb, rgb)`,
`wait_for_solid_color(client, rgb)` (never asserts a single capture: the
first frames after connect may predate the shell's first repaint; on
failure reports the colours seen).

### 12.4 Test clients — `tests/e2e/clients/`

Built only with `-De2e-test-client=true` (root `meson.build` adds
`subdir('tests/e2e/clients')`; option added to `meson_options.txt`);
never installed, not in the RPM. `tests/e2e/clients/meson.build` is
Appendix A.6 (generates xdg-shell client header/code with
`wayland-scanner` from `wayland-protocols`' `stable/xdg-shell/xdg-shell.xml`).

**`wtest-client.c`** (~550 lines, plain `wayland-client` + `wl_shm`,
no toolkit). Spec:

- Options: `--size WxH` (default 200x150), `--color AARRGGBB` (default
  `ffcc0000`), `--focus-color AARRGGBB` (0 = none), `--interactive`,
  `--title T`, `--request-fullscreen`, `--request-maximized`.
- Binds `wl_compositor` v4, `wl_shm` v1, `wl_seat` ≤ v5, `xdg_wm_base`
  ≤ v6 (v5+ needed for `wm_capabilities`). Sets title and app id
  `org.westonite.wtest-client`; sends `set_fullscreen`/`set_maximized`
  before the first commit if requested.
- Draws a solid-colour ARGB8888 buffer via `memfd_create` + `wl_shm`
  on every `xdg_surface.configure` (ack first), using `focus_color` when
  keyboard-focused and non-zero, else `color`. Destroys buffers on
  `release`.
- stdout protocol (one line per event, flushed):
  `wm-capabilities: [ n n … ]`, `configure: WxH [ state … ]` (states
  named `maximized|fullscreen|resizing|activated|other`; a 0x0 configure
  is printed too), `mapped: WxH` (first configure), `focus: enter|leave`,
  `pointer: enter`, `pointer: button N`, `key: N` (pressed only),
  `close-requested`, `paused`, `resumed`.
- `--interactive`: on `BTN_LEFT` (0x110) press →
  `xdg_toplevel_move(seat, serial)`; on `BTN_RIGHT` (0x111) press →
  `xdg_toplevel_resize(seat, serial, BOTTOM_RIGHT)`.
- SIGUSR1 sets a flag that makes the main loop print `paused` and stop
  dispatching entirely (unanswered pings → the shell's unresponsive path)
  until SIGUSR2 (`resumed`). The loop is `poll`-based
  (`prepare_read`/`read_events`/`dispatch_pending`, 200 ms poll) rather
  than `wl_display_dispatch`, which restarts its internal poll on EINTR
  and would delay the pause.

**`wtest-xclient.c`** (~105 lines, xcb): `--size WxH` (200x150),
`--color RRGGBB` (default magenta `cc00cc`); creates an InputOutput window
with `back_pixel` = colour and Exposure|StructureNotify mask, sets
`WM_NAME` `wtest-xclient`, maps; prints `mapped` on MapNotify and
`position: X Y WxH` (root-relative via `xcb_translate_coordinates`) after
map and on every ConfigureNotify. EL10 ships no `xeyes`/`xclock`/
`xwininfo`, so this replaces all of them.

### 12.5 Test inventory (final: 45 tests; 8 marked `installed`)

`tests/e2e/pytest.ini` declares one marker: `installed` ("also runs
against the installed RPM in the pristine container (no build tree, no
wtest-client)"). Markers below: **[I]** = `@pytest.mark.installed`.

**`test_lifecycle.py`** (E1)

| Test | Asserts |
|---|---|
| `test_clean_shutdown_sigterm` [I] | headless instance; `terminate(SIGTERM)` exits 0 |
| `test_clean_shutdown_sigint` | same with SIGINT |
| `test_clean_shutdown_vnc_backend` | vnc backend instance; SIGTERM exits 0 |

**`test_shell_background.py`** (E1)

| Test | Asserts |
|---|---|
| `test_background_default` [I] | vnc 640x480; RFB geometry is (640, 480); whole frame becomes solid `(0x00,0x22,0x44)` |
| `test_background_from_config` [I] | `[shell] background-color=0xff336699` → solid `(0x33,0x66,0x99)`; log mentions `westonite.ini` |

**`test_cli.py`** (E2) — immediate-exit cases use `subprocess.run` directly.

| Test | Asserts |
|---|---|
| `test_version` | `--version` exits 0; first stdout token is `westonite` |
| `test_help` | `--help` exits 0; contains `--backend` |
| `test_unknown_backend` | `--backend=bogus` (with `--log`, `--no-config`, `XDG_RUNTIME_DIR` set) exits non-zero; log contains `unknown backend "bogus"` |
| `test_unhandled_option_is_fatal` | `--bogus-option` exits non-zero; log contains `unhandled option: --bogus-option` |
| `test_missing_xdg_runtime_dir_refused` | without `XDG_RUNTIME_DIR`: non-zero; stderr mentions `XDG_RUNTIME_DIR` |
| `test_socket_name` | `--socket=wibble-0` → that socket exists and is the display |
| `test_two_instances_share_runtime_dir` | two instances in one runtime dir get two distinct `wayland-*` sockets (`add_socket_auto`) |
| `test_config_found_in_xdg_config_home` [I] | `[core]` ini in `$XDG_CONFIG_HOME` → log `Using config file '…/westonite.ini'` |
| `test_config_explicit_path` | `--config=<custom-name.ini>` is used |
| `test_stock_weston_ini_is_ignored` [I] | a `weston.ini` in `$XDG_CONFIG_HOME` is never mentioned in the log (P2 negative) |
| `test_home_config_fallback` | with `XDG_CONFIG_HOME` unset and `HOME=<tmp>`, `~/.config/westonite.ini` is found |
| `test_no_config_flag_ignores_existing_ini` | `--no-config` with an ini present → no `Using config file` |

**`test_children.py`** (E2) — stub clients are `#!/bin/sh` scripts that
`env > <marker>` then run a body; "spawned" = marker file appears.

| Test | Asserts |
|---|---|
| `test_no_helper_clients_by_default` [I] | after `desktop-shell.so` loads, 1 s grace, no `launching` in log |
| `test_shell_client_setting_is_ignored` | `[shell] client=<stub>` spawns nothing, no `launching` (T3 made it dead config) |
| `test_autolaunch_config_spawns` | `[autolaunch] path=<stub>` → stub runs with `WAYLAND_DISPLAY` == the instance's socket and `WESTON_CONFIG_FILE` == the ini path |
| `test_autolaunch_watch_exits_with_client` [I] | `watch=true`, stub `sleep 1` → compositor exits 0 by itself (kiosk primitive) |
| `test_autolaunch_no_watch_survives_client_exit` | `watch=false`, stub `exit 0` → compositor still running after 1 s |
| `test_autolaunch_child_crash_tolerated` | stub `kill -SEGV $$` → compositor still running |
| `test_autolaunch_nonexecutable_is_fatal` | `path=/nonexistent-client` → log `autolaunch path (/nonexistent-client) is not executable`, non-zero exit |
| `test_positional_command_runs_and_watch_applies` | `westonite -- <stub> arg1` → stub sees `arg1`, compositor exits 0 when it exits |
| `test_clean_shutdown_with_live_child` | SIGTERM with a running autolaunched client exits 0 |

**`test_outputs.py`** (E2) — geometry via `wayland-info` (`wayland-utils`).

| Test | Asserts |
|---|---|
| `test_headless_geometry_from_cli` | `--width=800 --height=500` → `wl_output` block shows `width: 800 px, height: 500 px` |
| `test_output_scale_and_transform_from_config` | `[output] name=headless mode=640x480 scale=2 transform=rotate-90` → `scale: 2`, `transform: 90` |
| `test_vnc_output_mode_from_config` | `[output] name=vnc mode=800x500` → RFB geometry (800, 500) |
| `test_vnc_client_resize_repaints_background` | **`@pytest.mark.skip`** with the reason from §12.6 (SetDesktopSize segfaults the RPM stack) |
| `test_multi_backend_headless_plus_vnc` | `--backend=vnc --backends=headless,vnc` → both `headless` and `vnc` outputs advertised (poll: the second backend's output can appear after the socket), VNC capture works |

**`test_shell_windows.py`** (E3) — vnc backend + `wtest-client`. Colours:
RED `ffcc0000`/BRIGHT_RED `ffff4444`, BLUE `ff0000cc`/BRIGHT_BLUE
`ff4444ff`, GREEN `ff00cc00`. Helpers: `grab_drag()` presses, **waits
until the client reports the button** (so its move/resize request reached
the shell while the button is still down), then moves in steps and
releases; `move_window_to()` re-drags until the window origin lands on
the target (the RPM's VNC input path occasionally drops trailing motions;
each iteration is still a real grab).

| Test | Asserts |
|---|---|
| `test_window_maps_inside_output` | `mapped: 200x150`; a 200x150 RED region exists fully inside 640x480 |
| `test_new_window_gets_keyboard_focus` | `focus: enter` and the BRIGHT_RED region appears |
| `test_click_moves_activation_between_windows` | two windows; find whichever is drawn unfocused (spawn order ≠ map order), click a pixel of it; it gets `focus: enter`, the other `focus: leave`; framebuffer shows clicked bright, other dim; a typed `a` keysym (0x61) lands in the clicked client |
| `test_pointer_move_grab` | `--interactive`; `move_window_to(60,60)` succeeds with size unchanged |
| `test_pointer_resize_grab` | park at (40,40); right-drag from 5 px inside the bottom-right corner by (+80,+60), up to 3 attempts, until a `configure: WxH [ resizing` line with W,H > original; then poll until the on-screen region grew **and** the last sized configure equals the on-screen size. Exact deltas deliberately not asserted (§12.6) |
| `test_wm_capabilities_empty` | `wm-capabilities: [ ]` with nothing inside (T6/T9) |
| `test_fullscreen_request_ignored` | `--request-fullscreen` → maps 200x150, region 200x150, no configure contains `fullscreen` |
| `test_maximize_request_ignored` | same for `--request-maximized` / `maximized` |
| `test_background_clicks_are_swallowed` | click on bare background (x=5 or 630, y=470 away from the window); after 0.5 s no `focus: leave`; window still drawn focused |
| `test_unresponsive_client_handled` | `--interactive`; `pause()`; three clicks on it 0.2 s apart; region origin unchanged (no move grab, T8); `resume()`; a click yields `pointer: button`; client alive |

**`test_xwayland.py`** (E4) — `--xwayland`; `wtest-xclient` with
`DISPLAY=<x_display>`. `titlebar_drag()` presses on the xwm titlebar 10 px
above the content, **holds 0.5 s** (xwm handles frame clicks
asynchronously), moves in 12 steps with 30 ms pauses, holds 0.3 s,
releases.

| Test | Asserts |
|---|---|
| `test_lazy_spawn_and_roundtrip` [I] | display advertised while log lacks `launching '/usr/bin/Xwayland'`; `xdpyinfo -display :N` prints `vendor string`; then the launching line appears |
| `test_x11_window_renders_and_position_syncs` | MAGENTA region 200x150; poll until the last `position:` report equals the current region origin (recapture inside the poll: placement can still shift after the first frame) |
| `test_x11_window_drag_updates_x_position` | titlebar drag by (80,50); poll until the region moved and the last reported X position equals the new origin |
| `test_two_x11_clients` | two X clients (magenta, orange `cc6600`) both render |

### 12.6 Findings recorded during E1–E5 (all RPM-side; document, do not fix)

1. **S1 (E1)** — security types as in §12.1; resolved by the in-repo
   Apple DH client. `pam_unix` with a real password works for a non-root
   compositor in the container; wrong passwords are rejected.
2. **Client-initiated VNC resize crashes the RPM stack (E2).** A
   `SetDesktopSize` request segfaults the compositor inside neatvnc
   0.9.0's raw-encoder worker (`pixel_to_cpixel`), even with
   `[output] resizeable=false`, and the server never advertises an
   ExtendedDesktopSize layout first. Operational implication: an
   authenticated VNC client can kill the session. Suite consequence:
   `vnc-resize` and `background-resize` are skip-marked; the RFB client
   keeps `set_desktop_size()` for when EPEL ships a fix.
3. **VNC drag deltas reach resize grabs halved (E3).** Only every second
   pointer motion reaches the resize grab, at half its delta (an 8-step
   +70,+50 drag yields sized configures matching motions 2/4/6/8 at
   half-delta, ending +35,+25), while the same stream lands *move* grabs
   pixel-exactly. Lives in the RPM's input translation. Consequence: the
   resize test asserts growth + consistency, not exact deltas.
4. **Grabs need the button held until the client's request reaches the
   shell (E3)** — encoded in `grab_drag`.
5. **Trailing drag motions occasionally dropped (E5, ~1 in 10 runs)** —
   drag-driven tests converge by re-dragging.
6. **S2 / P0 has no reachable black-box trigger.** The P0 backport guards
   the mirror-of output-resize path; the only runtime output resize
   reachable black-box is a VNC `SetDesktopSize`, which crashes first
   (finding 2). Recorded as a documented gap; the P0 coverage is the
   verbatim backport itself.
7. Deferred from the inventory: a `transient` popup/child stacking test
   (initial placement is randomised, so overlap cannot be arranged
   deterministically without more client machinery); touch/tablet paths
   (VNC injects only pointer/keyboard); DRM on real hardware.

### 12.7 Phase deliverables and gates

- **E1** — `docs/e2e-test-plan.md`; `vncclient.py`; `conftest.py`;
  `support/compositor.py`, `support/image.py`; `scripts/e2e-test.sh`
  (Appendix A.13: builds with `-De2e-test-client=true`, sets up
  `/tmp/.X11-unix`, the PAM file, the `e2e` user with password
  `westonite-e2e`, results dir ownership, and `runuser`s pytest with
  `WESTONITE_VNC_USER`, `WESTONITE_VNC_PASSWORD`, `WTEST_CLIENT`,
  `WTEST_XCLIENT`, `WESTONITE_E2E_ARTIFACTS`, `--junit-xml`); build image
  gains `python3-pytest`, `python3-cryptography`; CI step added. Gate:
  the 5 E1 tests green in the container.
- **E2** — `test_cli.py`, `test_children.py`, `test_outputs.py` (26 tests
  incl. the skip); RFB client gains DesktopSize/ExtendedDesktopSize;
  `wayland-utils` added to the image. Gate: green.
- **E3** — `wtest-client.c` + `clients/meson.build` + the meson option;
  `support/client.py`; `test_shell_windows.py` (10 tests); `image.py`
  gains `pick_pixel`/`region_of`/`wait_for_region`. Gate: green.
- **E4** — `wtest-xclient.c` (`libxcb-devel` in the image);
  `test_xwayland.py` (4 tests); `pytest.ini` with the `installed` marker
  and the 8 `[I]` marks; `rpm-install-test.sh` gains
  `desktop-file-validate` of the session file + `Exec=` lookup and the
  `-m installed` pytest run against the installed RPM (needs
  `python3-pytest python3-cryptography desktop-file-utils util-linux`
  installed in the pristine container). Gate: full suite green in the
  build container **and** the installed subset green in the pristine
  container.
- **E5** — both scripts take a results dir; conftest artifact hook;
  CI uploads JUnit XML + failures as `test-results` (`if: always()`);
  flake hardening (findings 5 and the map-order fix); README gains the
  Testing section (how the suite works: VNC control plane, in-repo RFB
  client, PAM stack, the two test clients, flat-colour pixel assertions,
  no sleeps, CI artifacts). Gate: 12× stress on the input-driven files
  plus 3× full suite green.

Runtime budget: the e2e stage runs ~45 tests in roughly 20 s after the
build; the whole CI stage should stay under ~5 minutes.

---

## 13. CI hardening (first days of CI)

Two commits, both in the harness only:

1. **Slow-runner hardening.** The Apple-DH + PAM handshake can stall past
   10 s on loaded 2-core runners (seen once in `test_two_x11_clients`).
   Raise the RFB socket timeout to 30 s and retry the connect once after
   asserting the compositor is still alive. Also change the workflow
   trigger from `push: branches: ['**']` to `push: branches: [main]` so
   PR commits run CI once, not twice.
2. **Never probe-connect the RFB port.** `wait_ready()` used to open and
   close a TCP connection to the VNC port. neatvnc is single-client and
   treats the probe as a client; on slow runners its teardown raced the
   real client's connect, which then died with an instant
   connection-closed (10 tests in one run) or a stalled handshake. The
   probe was also redundant (§12.1). Remove it; extend the one-retry to
   `RfbError` too.

---

## 14. Final acceptance checklist

- [ ] `docker build -f containers/Containerfile.build -t westonite-build .` succeeds.
- [ ] `scripts/smoke-test.sh` passes in the image (3 checks).
- [ ] `scripts/e2e-test.sh /results` passes: 44 passed, 1 skipped.
- [ ] `scripts/rpm-build.sh /out` produces `westonite-14.0.1-1.el10.x86_64.rpm` (+debuginfo, debugsource).
- [ ] `scripts/rpm-install-test.sh /rpms /src /results` in a pristine `centos:stream10` container: `weston` not pulled in, session file valid, legacy smoke passes, `-m installed` subset 8 passed.
- [ ] GitHub Actions workflow green on `main`.
- [ ] `VENDOR.md` lists P0, P2, P3, P4, T1–T9 (with the debug-key restore noted under T5).
- [ ] `docs/` contains `phase0-findings.md`, `desktop-shell-capabilities.md`, `frontend-capabilities.md`, `maintenance-layer-plan.md`, `e2e-test-plan.md`; `PLAN.md` has every phase ticked with its verification summary.
- [ ] `desktop-shell/shell.c` has the 10-entry `shell_desktop_api`, the 4-registration `shell_add_bindings`, and reads only `background-color`.
- [ ] `grep -r wl_global_create desktop-shell/` finds nothing.

---

## Appendix A — exact file contents (final C-only state)

These are reproduced verbatim from the finished repository so they can be
copied rather than re-derived. Where a file changed over the phases, the
final form is shown and the earlier differences are noted in the body
text above.

### A.1 `meson.build` (root)

```meson
project('westonite',
	'c',
	version: '14.0.1',
	default_options: [
		'warning_level=3',
		'c_std=gnu99',
		'b_lundef=true',
	],
	meson_version: '>= 0.63.0',
	license: 'MIT/Expat',
)

version_westonite = meson.project_version()

dir_prefix = get_option('prefix')
dir_bin = dir_prefix / get_option('bindir')
dir_data = dir_prefix / get_option('datadir')
dir_lib = dir_prefix / get_option('libdir')
dir_libexec = dir_prefix / get_option('libexecdir')
dir_module_westonite = dir_lib / 'westonite'

common_inc = include_directories('.')

pkgconfig = import('pkgconfig')

git_version_h = vcs_tag(
	input: 'git-version.h.meson',
	output: 'git-version.h',
	fallback: version_westonite
)

config_h = configuration_data()

cc = meson.get_compiler('c')

global_args = cc.get_supported_arguments(
	'-Wmissing-prototypes',
	'-Wno-unused-parameter',
	'-Wno-shift-negative-value',
	'-Wno-missing-field-initializers',
	'-Wno-pedantic',
	'-Wundef',
	'-fvisibility=hidden',
)
add_project_arguments(global_args, language: 'c')

if cc.has_header_symbol('sys/sysmacros.h', 'major')
	config_h.set('MAJOR_IN_SYSMACROS', 1)
elif cc.has_header_symbol('sys/mkdev.h', 'major')
	config_h.set('MAJOR_IN_MKDEV', 1)
endif

optional_libc_funcs = [
	'mkostemp', 'strchrnul', 'initgroups', 'posix_fallocate',
	'memfd_create', 'unreachable',
]
foreach func : optional_libc_funcs
	if cc.has_function(func)
		config_h.set('HAVE_' + func.to_upper(), 1)
	endif
endforeach

optional_builtins = {
	'builtin_clz': 'return __builtin_clz(1);',
	'builtin_bswap32': 'return __builtin_bswap32(0);',
	'builtin_popcount': 'return __builtin_popcount(0);',
}
foreach name, check : optional_builtins
	if cc.links('int main(void) { @0@ }'.format(check), name: name)
		config_h.set('HAVE_' + name.to_upper(), 1)
	endif
endforeach

config_h.set('_GNU_SOURCE', '1')
config_h.set('_ALL_SOURCE', '1')

config_h.set_quoted('PACKAGE_STRING', 'westonite @0@'.format(version_westonite))
config_h.set_quoted('PACKAGE_VERSION', version_westonite)
config_h.set_quoted('VERSION', version_westonite)
config_h.set_quoted('PACKAGE_URL', 'https://github.com/nhwalker/example-weston-standalone')
config_h.set_quoted('PACKAGE_BUGREPORT', 'https://github.com/nhwalker/example-weston-standalone/issues')

config_h.set_quoted('BINDIR', dir_bin)
config_h.set_quoted('DATADIR', dir_data)
config_h.set_quoted('LIBEXECDIR', dir_libexec)
config_h.set_quoted('MODULEDIR', dir_module_westonite)

dep_libweston = dependency('libweston-14', version: '>= 14.0.1')

# libweston's own loadable modules (backends, renderers, xwayland.so) come
# from the weston-libs RPM; the frontend loads xwayland.so from there.
dir_module_libweston = dep_libweston.get_variable(pkgconfig: 'libdir') / 'libweston-14'
config_h.set_quoted('LIBWESTON_MODULEDIR', dir_module_libweston)

# The EPEL 10 libweston build ships every backend header (verified in
# Phase 0); probe each anyway so a repackaged libweston fails loudly at
# configure time instead of at compile time.
foreach backend : [ 'drm', 'headless', 'pipewire', 'rdp', 'vnc', 'wayland', 'x11' ]
	if cc.has_header('libweston/backend-@0@.h'.format(backend), dependencies: dep_libweston)
		config_h.set('BUILD_@0@_COMPOSITOR'.format(backend.to_upper()), '1')
	else
		error('libweston-14 does not install backend-@0@.h; frontend/main.c expects all backend headers (see docs/phase0-findings.md).'.format(backend))
	endif
endforeach

backend_default = get_option('backend-default')
config_h.set_quoted('WESTON_NATIVE_BACKEND', backend_default)

if get_option('xwayland')
	config_h.set('BUILD_XWAYLAND', '1')
	config_h.set_quoted('XSERVER_PATH', get_option('xwayland-path'))
	dep_xwayland = dependency('xwayland', required: false)
	if dep_xwayland.found() and dep_xwayland.get_variable(pkgconfig: 'have_listenfd', default_value: 'false') == 'true'
		config_h.set('HAVE_XWAYLAND_LISTENFD', '1')
	endif
endif

dep_wayland_server = dependency('wayland-server')
dep_pixman = dependency('pixman-1')
dep_libinput = dependency('libinput')
dep_libevdev = dependency('libevdev')
dep_libdl = cc.find_library('dl', required: false)
dep_libm = cc.find_library('m', required: false)
dep_threads = dependency('threads')

subdir('shared')
subdir('frontend')
subdir('desktop-shell')
subdir('data')
if get_option('e2e-test-client')
	subdir('tests/e2e/clients')
endif

configure_file(output: 'config.h', configuration: config_h)
```

`meson_options.txt` (final):

```meson
option(
	'backend-default',
	type: 'combo',
	choices: [ 'drm', 'wayland', 'x11', 'headless' ],
	value: 'drm',
	description: 'Default backend when none is specified on the command line'
)
option(
	'xwayland',
	type: 'boolean',
	value: true,
	description: 'Build the Xwayland glue (requires libweston built with xwayland)'
)
option(
	'xwayland-path',
	type: 'string',
	value: '/usr/bin/Xwayland',
	description: 'Path to the Xwayland binary'
)
option(
	'e2e-test-client',
	type: 'boolean',
	value: false,
	description: 'Build the wtest-client test driver used by tests/e2e (never installed)'
)
```

Until T3 there is a fourth option:

```meson
option(
	'shell-client-default',
	type: 'string',
	value: '',
	description: 'Default shell helper client spawned by desktop-shell (empty: none; override per-config with [shell] client=)'
)
```

`git-version.h.meson`: `#define BUILD_ID "@VCS_TAG@"`

### A.2 `shared/meson.build`

```meson
# Subset of upstream shared/: only the pieces whose symbols are NOT
# exported by libweston-14.so (config-parser is exported and comes from
# the RPM; see docs/phase0-findings.md).
srcs_libshared = [
	'os-compatibility.c',
	'process-util.c',
	'option-parser.c',
]

lib_libshared = static_library(
	'shared',
	srcs_libshared,
	include_directories: common_inc,
	dependencies: [ dep_libweston, dep_wayland_server ],
	pic: true,
	install: false
)
dep_libshared = declare_dependency(
	link_with: lib_libshared,
	dependencies: [ dep_libweston ],
)
```

### A.3 `frontend/meson.build`

```meson
srcs_westonite = [
	git_version_h,
	'main.c',
	'config-helpers.c',
	'weston-screenshooter.c',
]
deps_westonite = [
	dep_libshared,
	dep_libweston,
	dep_libinput,
	dep_libevdev,
	dep_libdl,
	dep_threads,
]

if get_option('xwayland')
	srcs_westonite += 'xwayland.c'
endif

libexec_westonite = shared_library(
	'exec_westonite',
	sources: srcs_westonite,
	include_directories: common_inc,
	dependencies: deps_westonite,
	install_dir: dir_module_westonite,
	install: true,
	version: '0.0.0',
	soversion: 0
)
dep_libexec_westonite = declare_dependency(
	link_with: libexec_westonite,
	include_directories: include_directories('.'),
	dependencies: dep_libweston
)

exe_westonite = executable(
	'westonite',
	'executable.c',
	include_directories: common_inc,
	dependencies: dep_libexec_westonite,
	install_rpath: dir_module_westonite,
	install: true
)
```

(Phase 1/2 form additionally lists `'text-backend.c'` and the four
`text_input_unstable_v1_*` / `input_method_unstable_v1_*` generated
targets in `srcs_westonite`.)

### A.4 `desktop-shell/meson.build`

```meson
srcs_shell_desktop = [
	'shell.c',
]
deps_shell_desktop = [
	dep_libm,
	dep_libexec_westonite,
	dep_libshared,
	dep_libweston,
	dep_pixman,
]
plugin_shell_desktop = shared_library(
	'desktop-shell',
	srcs_shell_desktop,
	include_directories: common_inc,
	dependencies: deps_shell_desktop,
	name_prefix: '',
	install: true,
	install_dir: dir_module_westonite,
	install_rpath: '$ORIGIN'
)
```

(Phase 1/2 form: `srcs_shell_desktop` also lists `'input-panel.c'`,
`weston_desktop_shell_server_protocol_h`, `weston_desktop_shell_protocol_c`,
`input_method_unstable_v1_server_protocol_h`, `input_method_unstable_v1_protocol_c`.)

Phase 1/2 `protocol/meson.build` (deleted at T3):

```meson
dep_scanner = dependency('wayland-scanner', native: true)
prog_scanner = find_program(dep_scanner.get_variable(pkgconfig: 'wayland_scanner'))

dep_wp = dependency('wayland-protocols', version: '>= 1.33')
dir_wp_base = dep_wp.get_variable(pkgconfig: 'pkgdatadir')

xml_text_input = dir_wp_base / 'unstable' / 'text-input' / 'text-input-unstable-v1.xml'
xml_input_method = dir_wp_base / 'unstable' / 'input-method' / 'input-method-unstable-v1.xml'
xml_desktop_shell = files('weston-desktop-shell.xml')

text_input_unstable_v1_protocol_c = custom_target(
	'text-input-unstable-v1 protocol.c',
	input: xml_text_input,
	output: 'text-input-unstable-v1-protocol.c',
	command: [ prog_scanner, 'private-code', '@INPUT@', '@OUTPUT@' ],
)
text_input_unstable_v1_server_protocol_h = custom_target(
	'text-input-unstable-v1 server-header.h',
	input: xml_text_input,
	output: 'text-input-unstable-v1-server-protocol.h',
	command: [ prog_scanner, 'server-header', '@INPUT@', '@OUTPUT@' ],
)

input_method_unstable_v1_protocol_c = custom_target(
	'input-method-unstable-v1 protocol.c',
	input: xml_input_method,
	output: 'input-method-unstable-v1-protocol.c',
	command: [ prog_scanner, 'private-code', '@INPUT@', '@OUTPUT@' ],
)
input_method_unstable_v1_server_protocol_h = custom_target(
	'input-method-unstable-v1 server-header.h',
	input: xml_input_method,
	output: 'input-method-unstable-v1-server-protocol.h',
	command: [ prog_scanner, 'server-header', '@INPUT@', '@OUTPUT@' ],
)

weston_desktop_shell_protocol_c = custom_target(
	'weston-desktop-shell protocol.c',
	input: xml_desktop_shell,
	output: 'weston-desktop-shell-protocol.c',
	command: [ prog_scanner, 'private-code', '@INPUT@', '@OUTPUT@' ],
)
weston_desktop_shell_server_protocol_h = custom_target(
	'weston-desktop-shell server-header.h',
	input: xml_desktop_shell,
	output: 'weston-desktop-shell-server-protocol.h',
	command: [ prog_scanner, 'server-header', '@INPUT@', '@OUTPUT@' ],
)
```

### A.5 `data/meson.build`

```meson
install_data(
	'westonite.desktop',
	install_dir: dir_data / 'wayland-sessions'
)
install_data(
	'westonite.ini.example',
	install_dir: dir_data / 'doc' / 'westonite'
)
```

### A.6 `tests/e2e/clients/meson.build`

```meson
# wtest-client: xdg-shell test driver for the e2e suite.
# Built only with -De2e-test-client=true; never installed.

dep_wayland_client = dependency('wayland-client')
dep_wayland_protocols = dependency('wayland-protocols')
prog_scanner = find_program('wayland-scanner')

dir_wp = dep_wayland_protocols.get_variable(pkgconfig: 'pkgdatadir')
xml_xdg_shell = dir_wp / 'stable' / 'xdg-shell' / 'xdg-shell.xml'

xdg_shell_client_h = custom_target(
	'xdg-shell-client-protocol.h',
	command: [ prog_scanner, 'client-header', '@INPUT@', '@OUTPUT@' ],
	input: xml_xdg_shell,
	output: 'xdg-shell-client-protocol.h',
)
xdg_shell_c = custom_target(
	'xdg-shell-protocol.c',
	command: [ prog_scanner, 'private-code', '@INPUT@', '@OUTPUT@' ],
	input: xml_xdg_shell,
	output: 'xdg-shell-protocol.c',
)

executable(
	'wtest-client',
	[ 'wtest-client.c', xdg_shell_client_h, xdg_shell_c ],
	dependencies: [ dep_wayland_client ],
	install: false,
)

executable(
	'wtest-xclient',
	'wtest-xclient.c',
	dependencies: [ dependency('xcb') ],
	install: false,
)
```

### A.7 `data/westonite.ini.example` (final)

```ini
# Example configuration for westonite.
#
# Copy to one of the locations westonite searches (first hit wins):
#   $XDG_CONFIG_HOME/westonite.ini   (~/.config/westonite.ini)
#   ~/.config/westonite.ini
# or pass explicitly with:  westonite -c /path/to/westonite.ini
#
# The format follows weston.ini(5), but westonite supports no helper
# clients (no panel, no on-screen keyboard, no lock screen) and no
# animations; options for those features do not exist here.

[core]
# Backend: drm-backend.so (default), headless, wayland, x11, ...
#backend=drm
# Xwayland support (requires xorg-x11-server-Xwayland installed):
#xwayland=true

[shell]
# Solid background color drawn by the compositor (0xAARRGGBB):
#background-color=0xff002244

[keyboard]
#keymap_layout=us
```

### A.8 `rpm/westonite.spec`

```spec
Name:           westonite
Version:        14.0.1
Release:        1%{?dist}
Summary:        Standalone Weston-based Wayland compositor

# Vendored weston sources; see VENDOR.md
License:        MIT
URL:            https://github.com/nhwalker/example-weston-standalone
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  gcc
BuildRequires:  meson >= 0.63
BuildRequires:  pkgconfig(libweston-14) >= 14.0.1
BuildRequires:  pkgconfig(wayland-server)
BuildRequires:  pkgconfig(wayland-scanner)
BuildRequires:  pkgconfig(wayland-protocols) >= 1.33
BuildRequires:  pkgconfig(libinput)
BuildRequires:  pkgconfig(libevdev)
BuildRequires:  pkgconfig(pixman-1)
# Enables HAVE_XWAYLAND_LISTENFD; the build works without it
BuildRequires:  pkgconfig(xwayland)

# libweston runtime, backends, renderers and xwayland.so (EPEL 10)
Requires:       weston-libs%{?_isa} >= 14.0.1
Recommends:     xorg-x11-server-Xwayland

%description
Westonite is the Weston 14 compositor frontend and desktop-shell plugin,
built standalone against the distribution's libweston 14 packages and
renamed so it installs alongside the stock weston package. It ships no
helper clients: there is no panel or on-screen keyboard unless configured
to use external ones (see westonite.ini.example).

%prep
%autosetup

%build
%meson
%meson_build

%install
%meson_install

%files
%license COPYING
%doc VENDOR.md
%{_bindir}/westonite
%dir %{_libdir}/westonite
%{_libdir}/westonite/desktop-shell.so
%{_libdir}/westonite/libexec_westonite.so
%{_libdir}/westonite/libexec_westonite.so.*
%{_datadir}/wayland-sessions/westonite.desktop
%{_datadir}/doc/westonite/westonite.ini.example

%changelog
* Tue Jul 21 2026 <packager> - 14.0.1-1
- Initial package: weston 14.0.1 frontend + desktop-shell built against
  EPEL 10 libweston-14 (weston-libs), renamed to westonite, no helper
  clients, Xwayland enabled.
```

(The `%description` wording predates T3; after T3 "unless configured to
use external ones" is no longer true for the panel. Updating it is
optional.)

### A.9 `scripts/smoke-test.sh`

```bash
#!/bin/bash
# Build westonite from /src and run the Phase 1-3 smoke tests.
# Runs inside the containers/Containerfile.build image.
set -euo pipefail

cd /src
rm -rf build
meson setup build --prefix=/usr
ninja -C build
ninja -C build install

export XDG_RUNTIME_DIR=/tmp/xdg
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"
mkdir -p -m 1777 /tmp/.X11-unix

fail() { echo "FAIL: $1" >&2; [ -f "$2" ] && cat "$2" >&2; exit 1; }

echo "== smoke 1: headless, no config, no helper clients"
timeout --preserve-status 5 westonite --backend=headless --log=/tmp/w1.log \
	|| fail "westonite exited non-zero" /tmp/w1.log
grep -q "Loading module '/usr/lib64/westonite/desktop-shell.so'" /tmp/w1.log \
	|| fail "desktop-shell.so not loaded" /tmp/w1.log
grep -q "launching" /tmp/w1.log \
	&& fail "unexpected helper client launch" /tmp/w1.log

echo "== smoke 2: westonite.ini is honored"
export XDG_CONFIG_HOME=/tmp/cfg
mkdir -p "$XDG_CONFIG_HOME"
printf '[shell]\nbackground-color=0xff336699\n' > "$XDG_CONFIG_HOME/westonite.ini"
timeout --preserve-status 5 westonite --backend=headless --log=/tmp/w2.log \
	|| fail "westonite exited non-zero" /tmp/w2.log
grep -q "Using config file '/tmp/cfg/westonite.ini'" /tmp/w2.log \
	|| fail "westonite.ini not picked up" /tmp/w2.log

echo "== smoke 3: Xwayland round-trip"
westonite --backend=headless --xwayland --log=/tmp/w3.log &
WPID=$!
sleep 2
DISP=$(grep -oP 'listening on display \K:[0-9]+' /tmp/w3.log) \
	|| fail "no X display advertised" /tmp/w3.log
DISPLAY=$DISP xdpyinfo > /tmp/xdpy.out 2>&1 \
	|| fail "xdpyinfo could not query Xwayland" /tmp/xdpy.out
kill $WPID
wait $WPID || fail "westonite exited non-zero after Xwayland test" /tmp/w3.log
grep -q "launching '/usr/bin/Xwayland'" /tmp/w3.log \
	|| fail "Xwayland was not spawned" /tmp/w3.log

echo "ALL SMOKE TESTS PASSED"
```

### A.10 `scripts/rpm-build.sh`

```bash
#!/bin/bash
# Build the westonite RPM from the /src checkout.
# Runs inside the containers/Containerfile.build image.
# Usage: rpm-build.sh [output-dir]   (default: /src/build-rpms)
set -euo pipefail

OUT="${1:-/src/build-rpms}"
VERSION=$(sed -n 's/^Version: *//p' /src/rpm/westonite.spec)

git config --global --add safe.directory /src
mkdir -p /tmp/rpm/{SOURCES,SPECS} "$OUT"
cd /src
git archive --prefix="westonite-$VERSION/" \
	-o "/tmp/rpm/SOURCES/westonite-$VERSION.tar.gz" HEAD
cp rpm/westonite.spec /tmp/rpm/SPECS/
rpmbuild --define '_topdir /tmp/rpm' -ba /tmp/rpm/SPECS/westonite.spec
cp /tmp/rpm/RPMS/x86_64/*.rpm "$OUT"/
ls -l "$OUT"
```

### A.11 `scripts/rpm-install-test.sh` (final, after E4/E5)

```bash
#!/bin/bash
# Install the built RPM into a PRISTINE CentOS Stream 10 container,
# rerun the runtime smoke tests from the installed files, and run the
# @pytest.mark.installed subset of the e2e suite against them.
# Usage: rpm-install-test.sh [rpm-dir] [repo-dir] [results-dir]
#        (defaults: /rpms /src /tmp)
set -euo pipefail

RPMDIR="${1:-/rpms}"
SRCDIR="${2:-/src}"
RESULTS="${3:-/tmp}"

dnf -y install epel-release
dnf config-manager --set-enabled crb
dnf -y install "$RPMDIR"/westonite-[0-9]*.x86_64.rpm \
	xorg-x11-server-Xwayland xdpyinfo \
	python3-pytest python3-cryptography desktop-file-utils util-linux

rpm -q westonite weston-libs
if rpm -q weston >/dev/null 2>&1; then
	echo "FAIL: full weston package was pulled in" >&2
	exit 1
fi

echo "== session file is valid and points at real binaries"
desktop-file-validate /usr/share/wayland-sessions/westonite.desktop
EXEC=$(sed -n 's/^Exec=\([^ ]*\).*/\1/p' \
	/usr/share/wayland-sessions/westonite.desktop)
command -v "$EXEC" >/dev/null \
	|| { echo "FAIL: Exec=$EXEC not found in PATH" >&2; exit 1; }

echo "== legacy smoke: headless + Xwayland round-trip"
export XDG_RUNTIME_DIR=/tmp/xdg
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"
mkdir -p -m 1777 /tmp/.X11-unix

westonite --backend=headless --xwayland --log=/tmp/w.log &
WPID=$!
sleep 2
DISP=$(grep -oP 'listening on display \K:[0-9]+' /tmp/w.log)
DISPLAY=$DISP xdpyinfo | grep 'vendor string'
kill $WPID
wait $WPID
grep -q "launching" /tmp/w.log && ! grep -q "launching '/usr/bin/Xwayland'" /tmp/w.log \
	&& { echo "FAIL: unexpected client launch" >&2; exit 1; }

echo "== e2e installed-subset against the RPM files"
E2E_USER=e2e
E2E_PASSWORD=westonite-e2e
printf 'auth     required pam_unix.so\naccount  required pam_unix.so\n' \
	> /etc/pam.d/weston-remote-access
id -u "$E2E_USER" >/dev/null 2>&1 || useradd -m "$E2E_USER"
echo "$E2E_USER:$E2E_PASSWORD" | chpasswd

mkdir -p "$RESULTS/failures-installed"
chown -R "$E2E_USER" "$RESULTS/failures-installed"
touch "$RESULTS/e2e-installed.xml" && chown "$E2E_USER" "$RESULTS/e2e-installed.xml"

runuser -u "$E2E_USER" -- env \
	WESTONITE_VNC_USER="$E2E_USER" \
	WESTONITE_VNC_PASSWORD="$E2E_PASSWORD" \
	WESTONITE_E2E_ARTIFACTS="$RESULTS/failures-installed" \
	python3 -m pytest "$SRCDIR/tests/e2e" -v -m installed \
		-p no:cacheprovider --junit-xml="$RESULTS/e2e-installed.xml"

echo "RPM INSTALL TEST PASSED"
```

### A.12 `.github/workflows/ci.yml` (final)

```yaml
name: CI

# push covers main; pull_request covers PR branches -- without the
# split, every PR commit runs the whole (docker-heavy) workflow twice.
on:
  push:
    branches: [main]
  pull_request:

jobs:
  build-and-test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build the CentOS Stream 10 + EPEL build image
        run: docker build -f containers/Containerfile.build -t westonite-build .

      - name: Build westonite and run smoke tests
        run: docker run --rm -v "$PWD":/src westonite-build /src/scripts/smoke-test.sh

      - name: Run the e2e suite
        run: |
          mkdir -p test-results
          docker run --rm -v "$PWD":/src -v "$PWD/test-results":/results \
            westonite-build /src/scripts/e2e-test.sh /results

      - name: Build the RPM
        run: |
          mkdir -p out
          docker run --rm -v "$PWD":/src -v "$PWD/out":/out \
            westonite-build /src/scripts/rpm-build.sh /out

      - name: Install-test the RPM in a pristine container
        run: |
          mkdir -p test-results
          docker run --rm -v "$PWD/out":/rpms:ro -v "$PWD":/src:ro \
            -v "$PWD/test-results":/results \
            quay.io/centos/centos:stream10 /src/scripts/rpm-install-test.sh /rpms /src /results

      - name: Upload RPMs
        uses: actions/upload-artifact@v4
        with:
          name: westonite-rpms
          path: out/*.rpm

      - name: Upload test results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: test-results
          path: test-results/
          if-no-files-found: ignore
```

### A.13 `scripts/e2e-test.sh`

```bash
#!/bin/bash
# Build westonite from /src and run the e2e suite (tests/e2e).
# Runs inside the containers/Containerfile.build image, as root.
# Usage: e2e-test.sh [results-dir]   (default: /tmp; CI mounts a host
# dir here to collect the JUnit XML and per-failure compositor logs)
#
# The VNC backend always requires PAM authentication (service
# weston-remote-access) as the user running the compositor, so the
# suite itself runs as a dedicated non-root user with a real password
# checked by pam_unix -- the same stack a production system uses.
set -euo pipefail

RESULTS="${1:-/tmp}"

cd /src
rm -rf build
meson setup build --prefix=/usr -De2e-test-client=true
ninja -C build
ninja -C build install

mkdir -p -m 1777 /tmp/.X11-unix   # Xwayland needs it; bare containers lack it

E2E_USER=e2e
E2E_PASSWORD=westonite-e2e
printf 'auth     required pam_unix.so\naccount  required pam_unix.so\n' \
	> /etc/pam.d/weston-remote-access
id -u "$E2E_USER" >/dev/null 2>&1 || useradd -m "$E2E_USER"
echo "$E2E_USER:$E2E_PASSWORD" | chpasswd

mkdir -p "$RESULTS/failures"
chown -R "$E2E_USER" "$RESULTS/failures"
touch "$RESULTS/e2e-results.xml" && chown "$E2E_USER" "$RESULTS/e2e-results.xml"

exec runuser -u "$E2E_USER" -- env \
	WESTONITE_VNC_USER="$E2E_USER" \
	WESTONITE_VNC_PASSWORD="$E2E_PASSWORD" \
	WTEST_CLIENT=/src/build/tests/e2e/clients/wtest-client \
	WTEST_XCLIENT=/src/build/tests/e2e/clients/wtest-xclient \
	WESTONITE_E2E_ARTIFACTS="$RESULTS/failures" \
	python3 -m pytest /src/tests/e2e -v -p no:cacheprovider \
		--junit-xml="$RESULTS/e2e-results.xml"
```

`tests/e2e/pytest.ini`:

```ini
[pytest]
markers =
    installed: also runs against the installed RPM in the pristine container (no build tree, no wtest-client)
```

`tests/e2e/.gitignore`: `__pycache__/` and `*.pyc`. There was no root
`.gitignore` in the C-only state (CI writes `test-results/` and `out/`
in the workspace; add one if you prefer).

### A.14 `tests/e2e/support/vncclient.py`

```python
"""Minimal RFB (VNC) client for driving westonite's VNC backend in tests.

Deliberately tiny and dependency-light: pure Python plus the
`cryptography` package (RPM: python3-cryptography) for the Apple
Diffie-Hellman authentication (RFB security type 30), which is one of
the three security types neatvnc offers when TLS is disabled
(RSA-AES-256, RSA-AES, Apple DH -- there is no classic VNC auth, which
is why off-the-shelf scriptable clients fail here; see
docs/e2e-test-plan.md spike S1).

Supports exactly what the e2e suite needs:
  - authenticate as a PAM user (Apple DH)
  - full-framebuffer capture (Raw encoding only)
  - pointer and keyboard event injection
"""

import hashlib
import os
import socket
import struct

APPLE_DH = 30
RAW_ENCODING = 0
DESKTOP_SIZE = -223
EXTENDED_DESKTOP_SIZE = -308


class RfbError(AssertionError):
    pass


class VncClient:
    def __init__(self, host, port, username, password, timeout=30.0):
        # generous timeout: the Apple-DH handshake plus PAM check can
        # take several seconds on loaded 2-core CI runners
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.sock.settimeout(timeout)
        self._handshake(username, password)
        self._client_init()

    # -- wire helpers ---------------------------------------------------

    def _read(self, n):
        buf = b""
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise RfbError(f"server closed connection ({len(buf)}/{n} bytes)")
            buf += chunk
        return buf

    def _send(self, data):
        self.sock.sendall(data)

    # -- handshake ------------------------------------------------------

    def _handshake(self, username, password):
        version = self._read(12)
        if not version.startswith(b"RFB 003."):
            raise RfbError(f"unexpected protocol version {version!r}")
        self._send(b"RFB 003.008\n")

        ntypes = self._read(1)[0]
        if ntypes == 0:
            reason_len = struct.unpack(">I", self._read(4))[0]
            raise RfbError("handshake refused: "
                           + self._read(reason_len).decode(errors="replace"))
        types = self._read(ntypes)
        if APPLE_DH not in types:
            raise RfbError(f"server offers {list(types)}, need {APPLE_DH} (Apple DH)")
        self._send(bytes([APPLE_DH]))
        self._apple_dh_auth(username, password)

        result = struct.unpack(">I", self._read(4))[0]
        if result != 0:
            reason_len = struct.unpack(">I", self._read(4))[0]
            raise RfbError("authentication failed: "
                           + self._read(reason_len).decode(errors="replace"))

    def _apple_dh_auth(self, username, password):
        from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

        generator = int.from_bytes(self._read(2), "big")
        key_len = int.from_bytes(self._read(2), "big")
        prime = int.from_bytes(self._read(key_len), "big")
        server_pub = int.from_bytes(self._read(key_len), "big")

        priv = int.from_bytes(os.urandom(key_len), "big") % prime
        client_pub = pow(generator, priv, prime)
        shared = pow(server_pub, priv, prime)

        aes_key = hashlib.md5(shared.to_bytes(key_len, "big")).digest()
        # 128-byte credential block: username and password each occupy a
        # 64-byte null-terminated field; unused bytes stay random.
        creds = bytearray(os.urandom(128))
        user_bytes = username.encode()[:63] + b"\0"
        pass_bytes = password.encode()[:63] + b"\0"
        creds[0:len(user_bytes)] = user_bytes
        creds[64:64 + len(pass_bytes)] = pass_bytes

        enc = Cipher(algorithms.AES(aes_key), modes.ECB()).encryptor()
        ciphertext = enc.update(bytes(creds)) + enc.finalize()
        self._send(ciphertext + client_pub.to_bytes(key_len, "big"))

    def _client_init(self):
        self._send(b"\x01")  # shared
        head = self._read(24)
        self.width, self.height = struct.unpack(">HH", head[:4])
        name_len = struct.unpack(">I", head[20:24])[0]
        self.name = self._read(name_len).decode(errors="replace")

        # SetPixelFormat: 32bpp truecolor little-endian BGRX
        pixel_format = struct.pack(">BBBBHHHBBBxxx", 32, 24, 0, 1,
                                   255, 255, 255, 16, 8, 0)
        self._send(struct.pack(">Bxxx", 0) + pixel_format)
        # SetEncodings: Raw pixels (uncompressed) plus the desktop-size
        # pseudo-encodings so the server tells us about output resizes
        encodings = [RAW_ENCODING, DESKTOP_SIZE, EXTENDED_DESKTOP_SIZE]
        self._send(struct.pack(">BxH", 2, len(encodings))
                   + b"".join(struct.pack(">i", e) for e in encodings))
        # screen layout, learned from ExtendedDesktopSize rects
        self.screens = [(0, 0, 0, self.width, self.height, 0)]

    # -- framebuffer ----------------------------------------------------

    def capture(self):
        """Request a full non-incremental update; return (width, height,
        bytes) with 4-byte BGRX pixels, row-major. Desktop-size changes
        arriving in between are absorbed (self.width/height follow)."""
        while True:
            self._send(struct.pack(">BBHHHH", 3, 0, 0, 0,
                                   self.width, self.height))
            fb = bytearray(self.width * self.height * 4)
            got_pixels = False
            resized = False
            while not got_pixels and not resized:
                msg_type = self._read(1)[0]
                if msg_type == 0:  # FramebufferUpdate
                    self._read(1)
                    nrects = struct.unpack(">H", self._read(2))[0]
                    for _ in range(nrects):
                        x, y, w, h, enc = struct.unpack(">HHHHi",
                                                        self._read(12))
                        if enc == RAW_ENCODING:
                            data = self._read(w * h * 4)
                            for row in range(h):
                                dst = ((y + row) * self.width + x) * 4
                                src = row * w * 4
                                fb[dst:dst + w * 4] = data[src:src + w * 4]
                            got_pixels = True
                        elif enc == DESKTOP_SIZE:
                            if (w, h) != (self.width, self.height):
                                self.width, self.height = w, h
                                resized = True
                        elif enc == EXTENDED_DESKTOP_SIZE:
                            # servers repeat the current layout in normal
                            # updates; only a geometry *change* is a resize
                            nscreens = self._read(4)[0]
                            self.screens = [
                                struct.unpack(">IHHHHI", self._read(16))
                                for _ in range(nscreens)]
                            if (w, h) != (self.width, self.height):
                                self.width, self.height = w, h
                                resized = True
                        else:
                            raise RfbError(f"unexpected encoding {enc}")
                elif msg_type == 1:  # SetColourMapEntries
                    head = self._read(5)
                    ncolours = struct.unpack(">H", head[3:5])[0]
                    self._read(6 * ncolours)
                elif msg_type == 2:  # Bell
                    pass
                elif msg_type == 3:  # ServerCutText
                    length = struct.unpack(">I", self._read(7)[3:])[0]
                    self._read(length)
                else:
                    raise RfbError(f"unhandled server message type {msg_type}")
            if got_pixels and not resized:
                return self.width, self.height, bytes(fb)
            # a resize invalidates this update's geometry: re-request

    def set_desktop_size(self, width, height):
        """Ask the server to resize the output (SetDesktopSize). The
        result shows up asynchronously; poll capture() for the change."""
        screen_id, _, _, _, _, flags = self.screens[0]
        msg = struct.pack(">BxHHBx", 251, width, height, 1)
        msg += struct.pack(">IHHHHI", screen_id, 0, 0, width, height, flags)
        self._send(msg)

    def pixel(self, fb_bytes, x, y):
        """Return (r, g, b) at x,y from a capture() buffer."""
        off = (y * self.width + x) * 4
        b, g, r = fb_bytes[off], fb_bytes[off + 1], fb_bytes[off + 2]
        return (r, g, b)

    # -- input injection ------------------------------------------------

    def pointer(self, x, y, buttons=0):
        self._send(struct.pack(">BBHH", 5, buttons, x, y))

    def click(self, x, y, button=1):
        mask = 1 << (button - 1)
        self.pointer(x, y, 0)
        self.pointer(x, y, mask)
        self.pointer(x, y, 0)

    def drag(self, x0, y0, x1, y1, button=1, steps=8):
        """Press at (x0, y0), move in steps, release at (x1, y1)."""
        mask = 1 << (button - 1)
        self.pointer(x0, y0, 0)
        self.pointer(x0, y0, mask)
        for i in range(1, steps + 1):
            self.pointer(x0 + (x1 - x0) * i // steps,
                         y0 + (y1 - y0) * i // steps, mask)
        self.pointer(x1, y1, 0)

    def key(self, keysym, down):
        self._send(struct.pack(">BBxxI", 4, 1 if down else 0, keysym))

    def key_tap(self, keysym):
        self.key(keysym, True)
        self.key(keysym, False)

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
```

### A.15 `tests/e2e/conftest.py`

```python
import os
import shutil
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent))

from support.compositor import Westonite  # noqa: E402


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    """On failure, copy the test's working dir (compositor logs, config
    files, stub-client output) into $WESTONITE_E2E_ARTIFACTS so CI can
    upload it. Regular files only -- runtime dirs contain sockets --
    and never let collection problems break the test run itself."""
    outcome = yield
    report = outcome.get_result()
    artifacts = os.environ.get("WESTONITE_E2E_ARTIFACTS")
    if not artifacts or report.when != "call" or not report.failed:
        return
    try:
        tmp_path = item.funcargs.get("tmp_path")
        if not tmp_path or not Path(tmp_path).exists():
            return
        dest = Path(artifacts) / item.name
        for src in Path(tmp_path).rglob("*"):
            if not src.is_file() or src.is_symlink() or src.is_socket():
                continue
            target = dest / src.relative_to(tmp_path)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, target)
    except OSError as exc:
        sys.stderr.write(f"artifact collection failed for {item.name}: {exc}\n")


@pytest.fixture
def westonite(tmp_path):
    """Factory fixture: launch westonite instances, always torn down.

    Instances started with the factory are SIGTERMed at test end and
    must exit 0 -- every test doubles as a clean-shutdown test. Tests
    that already stopped an instance themselves are skipped by the
    teardown.
    """
    instances = []

    def launch(*, wait=True, **kw):
        w = Westonite(tmp_path / f"w{len(instances)}", **kw)
        instances.append(w)
        if wait:
            w.wait_ready()
        return w

    yield launch

    errors = []
    for w in instances:
        try:
            if w.proc.poll() is None:
                w.terminate()
        except AssertionError as e:
            errors.append(str(e))
        finally:
            w.kill()
    if errors:
        raise AssertionError("teardown failures:\n" + "\n".join(errors))
```

### A.16 `tests/e2e/support/image.py`

```python
"""Frame assertions for VNC captures (4-byte BGRX pixel buffers)."""

from .compositor import wait_until


def _first_match(row, pattern):
    """Leftmost pixel index whose BGR bytes equal pattern, or None."""
    i = row.find(pattern)
    while i != -1:
        if i % 4 == 0:
            return i // 4
        i = row.find(pattern, i + 1)
    return None


def _last_match(row, pattern):
    i = row.rfind(pattern)
    while i != -1:
        if i % 4 == 0:
            return i // 4
        i = row.rfind(pattern, 0, i + len(pattern) - 1)
    return None


def pick_pixel(fb, fb_width, rgb):
    """Coordinates (x, y) of some pixel exactly matching rgb, or None.
    Unlike a bounding-box center, the returned pixel is guaranteed to
    actually show the color (safe to click even with overlap)."""
    pattern = bytes((rgb[2], rgb[1], rgb[0]))
    stride = fb_width * 4
    for y in range(len(fb) // stride):
        x = _first_match(fb[y * stride:(y + 1) * stride], pattern)
        if x is not None:
            return (x, y)
    return None


def region_of(fb, fb_width, rgb):
    """Bounding box (x, y, w, h) of pixels exactly matching rgb, or None.

    Scenes are all flat colors (shell background + wtest-client fills),
    so exact matching is safe; per-row bytes.find keeps it fast.
    """
    pattern = bytes((rgb[2], rgb[1], rgb[0]))  # BGR in the capture buffer
    stride = fb_width * 4
    xmin = ymin = xmax = ymax = None
    for y in range(len(fb) // stride):
        row = fb[y * stride:(y + 1) * stride]
        first = _first_match(row, pattern)
        if first is None:
            continue
        last = _last_match(row, pattern)
        if ymin is None:
            ymin = y
        ymax = y
        if xmin is None or first < xmin:
            xmin = first
        if xmax is None or last > xmax:
            xmax = last
    if ymin is None:
        return None
    return (xmin, ymin, xmax - xmin + 1, ymax - ymin + 1)


def wait_for_region(client, rgb, deadline=10.0):
    """Poll captures until a region of the exact color exists; return
    (bounding box, framebuffer)."""
    state = {}

    def check():
        _, _, fb = client.capture()
        box = region_of(fb, client.width, rgb)
        state.update(box=box, fb=fb)
        return box

    wait_until(check, deadline=deadline,
               message=f"a region of rgb{tuple(rgb)} to appear")
    return state["box"], state["fb"]


def solid_color(fb, rgb):
    """True if every pixel of the BGRX buffer is exactly rgb."""
    r, g, b = rgb
    return (set(fb[0::4]) == {b}
            and set(fb[1::4]) == {g}
            and set(fb[2::4]) == {r})


def wait_for_solid_color(client, rgb, deadline=10.0):
    """Poll captures until the whole framebuffer is exactly rgb.

    The first frames after connect may predate the shell's first repaint,
    so a single capture is never asserted directly.
    """
    last = {}

    def check():
        w, h, fb = client.capture()
        last.update(w=w, h=h, fb=fb)
        return solid_color(fb, rgb)

    try:
        wait_until(check, deadline=deadline,
                   message=f"framebuffer to be solid #{bytes(rgb).hex()}")
    except AssertionError:
        seen = sorted({(last["fb"][i + 2], last["fb"][i + 1], last["fb"][i])
                       for i in range(0, len(last["fb"]), 4)})[:8]
        raise AssertionError(
            f"framebuffer never became solid rgb{tuple(rgb)}; "
            f"last frame {last['w']}x{last['h']} contained colors {seen}")
    return last["fb"]
```

---

## Appendix B — pitfalls index (things that cost time the first time)

| # | Where | Pitfall |
|---|---|---|
| 1 | Phase 0 | `meson` and `libinput-devel` are in CRB, not AppStream. Enable CRB or the image build fails. |
| 2 | Phase 0 | Public UBI 10 repos contain no weston and no `-devel` packages; unentitled builds must use CentOS Stream 10 + EPEL. |
| 3 | Phase 1 | Do not vendor `shared/config-parser.c`; its symbols come from `libweston-14.so`. Vendoring it duplicates symbols. |
| 4 | Phase 1 | `desktop-shell.so` needs an explicit `pixman-1` dependency; the RPM's `.pc` keeps pixman private. |
| 5 | Phase 3 | `/tmp/.X11-unix` missing → the RPM's `xwayland.so` segfaults in its bind-error path. Always `mkdir -p -m 1777`. |
| 6 | Phase 3 | Without `xorg-x11-server-Xwayland-devel` there is no `xwayland.pc`, so `HAVE_XWAYLAND_LISTENFD` stays off and the deprecated `-listen` path is used. |
| 7 | Phase 4 | Runtime install of `weston-libs` needs CRB enabled (`neatvnc` → `libturbojpeg`). |
| 8 | Phase 4 | `git archive` inside the container needs `git config --global --add safe.directory /src` (git refuses to operate on a checkout owned by a different user, which a bind mount usually is). |
| 9 | T3 | `weston_curtain_params` in 14.0.1 has a `get_label` callback and `surface_committed`; newer upstream differs. Write against the installed 14 headers. |
| 10 | E1 | neatvnc with TLS off offers only RSA-AES-256 / RSA-AES / Apple DH. No off-the-shelf Python client works; implement Apple DH. |
| 11 | E1 | VNC always requires PAM auth via service `weston-remote-access` as the compositor's own user; `pam_unix` needs a real password and a non-root user. |
| 12 | E1/E5 | neatvnc is single-client: never probe-connect the port; a second connection kicks the first; serialise connections per instance. |
| 13 | E2 | VNC `SetDesktopSize` segfaults the RPM stack. Skip-mark resize tests; do not try to work around it. |
| 14 | E3 | Grabs: hold the button until the client reports it, or the shell never sees the move/resize request while the button is down. |
| 15 | E3 | Resize grabs over VNC receive half-delta, every-other motions. Assert growth and consistency, not deltas. |
| 16 | E3 | Spawn order ≠ map order for two clients; discover which one is unfocused from the framebuffer. |
| 17 | E3 | `wtest-client` must use a `poll`-based loop, not `wl_display_dispatch`, or SIGUSR1 pausing is delayed by EINTR restarts. |
| 18 | E4 | EL10 ships no `xeyes`/`xclock`/`xwininfo`; carry an xcb client. xwm handles titlebar clicks asynchronously: press-and-hold before moving. |
| 19 | E4 | The pristine container also needs `desktop-file-utils` and `util-linux` (for `runuser`) for the installed subset. |
| 20 | E5 | The VNC input path occasionally drops trailing drag motions; converge by re-dragging. |
| 21 | CI | `push: branches: ['**']` plus `pull_request` runs every PR commit twice; restrict push to `main`. |
| 22 | CI | Apple-DH + PAM can take > 10 s on loaded runners: 30 s socket timeout and one retry. |
| 23 | Always | Never let the `weston` package into a test container: it brings a second `desktop-shell.so` and the helper clients. |

## Appendix C — what could not be re-verified from the finished repository

Stated so you do not treat it as more certain than it is:

1. The upstream commit ids `51dfd1be` (P0) and `ee92a531` (config-parser
   fix) and the claim that these are the *only* 14.0.1→14.0.2 changes to
   the vendored set come from the repository's own records, not from a
   fresh diff of the upstream tree. Verify with the `git log` in §2
   item 8 before applying P0.
2. The exact set of hunks in P0 was not reconstructed; only the resulting
   guard in `simple_heads_output_sharing_resize()` was observed in the
   finished `main.c`. Apply the upstream commit rather than hand-writing
   it.
3. The RPM package versions in §2 are those observed in July 2026 builds
   of the image; newer EPEL/CS10 content is expected and fine as long as
   weston stays at 14.0.x. If EPEL rebases weston, follow the rebase
   procedure in `VENDOR.md` (§5.3) — in particular re-check whether P0 is
   already included.
4. The original Phase 0 work was done in a proxied sandbox and built the
   image on top of a thin local wrapper that installed the proxy CA and
   set `proxy=` in `dnf.conf`; the committed Containerfile is unchanged
   by that. If your environment needs the same, do it outside the repo.
5. Line counts for intermediate trim states are taken from commit
   messages and may be off by a few lines from what you get.
6. `test_two_instances_share_runtime_dir` and
   `test_multi_backend_headless_plus_vnc` rely on log/socket timing that
   was tuned against 2-core GitHub runners; on much slower hosts raise
   the harness deadlines uniformly rather than per test.
