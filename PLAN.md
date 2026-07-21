# Port Plan: Weston Frontend + desktop-shell → `westonite`

Standalone build of the Weston compositor frontend and desktop-shell plugin,
in this repository, linking against **libweston 14 as installed from RPMs on
UBI 10 / RHEL 10**. The result is renamed **`westonite`** so it can be
installed alongside (and never collides with) the distro `weston` package.

Upstream baseline: **weston `14.0.2`** (tag in the weston repo; the `14.0`
branch tip only adds DRM-backend fixes that live inside libweston, which we
consume from the RPM anyway — so the tag matching the RPM version is the
right import base).

## 1. Scope

In scope:

- `frontend/` — the `weston` main binary (`main.c`, `executable.c`,
  `text-backend.c`, `config-helpers.c`, `weston-screenshooter.c`,
  `xwayland.c`, `weston.h`, `weston-private.h`), built as
  `libexec_westonite.so` + `westonite` executable, mirroring upstream.
- `desktop-shell/` — `shell.c`, `input-panel.c`, `shell.h`, built as
  `desktop-shell.so` in our own module dir.
- **Xwayland support** (`frontend/xwayland.c`) — enabled.
- Meson build + **RPM spec** (`westonite.spec`) so the port itself builds as
  an RPM in a UBI10 container.

Out of scope (explicitly decided):

- **No helper clients** — `weston-desktop-shell` (panel/background),
  `weston-keyboard`, and the toytoolkit/cairo stack are not ported. The
  shell runs with no panel (see §6).
- No `screen-share.c` (also avoids its dependency on a libweston *private*
  header — upstream issue #292), no `systemd-notify.c`.
- No changes to the weston repo itself; it is reference-only.

## 2. What the libweston 14 RPM provides vs. what we vendor

Findings from the 14.0 source (verified against `meson.build` files and
`#include` graphs):

**Provided by the RPM (`weston-devel` / libweston 14):**

- `libweston-14.so` + pkg-config `libweston-14.pc`.
- Installed headers used by our sources: `<libweston/libweston.h>`,
  `<libweston/desktop.h>`, `<libweston/shell-utils.h>`,
  `<libweston/weston-log.h>`, `<libweston/windowed-output-api.h>`,
  `<libweston/version.h>`, and — important — `<libweston/config-parser.h>`.
- The `weston_config_*` config-parser implementation: libweston 14 links
  `shared/` with `link_whole` and config-parser carries `WL_EXPORT`, so the
  `.so` exports it. **We do not vendor `config-parser.c`.**
- Backend headers `<libweston/backend-*.h>` and `<libweston/xwayland-api.h>`
  — but **only for features the RPM was built with** (each is installed by
  its feature's meson block). See risk R2/R3.
- Runtime backends/renderers (`/usr/lib64/libweston-14/*.so`) and the
  `xwayland.so` libweston module, loaded by public API — nothing to build.

**Vendored into this repo (verbatim from tag `14.0.2`):**

| Source | Files |
|---|---|
| `frontend/` | `main.c`, `executable.c`, `text-backend.c`, `config-helpers.c`, `weston-screenshooter.c`, `xwayland.c`, `weston.h`, `weston-private.h` |
| `desktop-shell/` | `shell.c`, `shell.h`, `input-panel.c` |
| `shared/` (compiled) | `os-compatibility.c`, `process-util.c`, `option-parser.c` — these have **no** `WL_EXPORT`, so libweston does not export them despite containing the objects; `parse_options()` is declared in the installed `<libweston/config-parser.h>`, only the implementation needs vendoring |
| `shared/` (headers) | `helpers.h`, `string-helpers.h`, `xalloc.h`, `timespec-util.h`, `fd-util.h`, `os-compatibility.h`, `process-util.h` (+ whatever else the compiler demands — let the build drive completion) |
| `protocol/` | `weston-desktop-shell.xml` (weston-private protocol) |
| misc | `weston.ini.in` (→ `westonite.ini.in`), `weston.desktop` (→ `westonite.desktop`), `COPYING` (MIT) |

**Generated at build time** with `wayland-scanner`:
`weston-desktop-shell` (vendored XML), `text-input-unstable-v1` and
`input-method-unstable-v1` (XMLs from the `wayland-protocols-devel` RPM,
with vendored fallback copies if UBI10 lacks the package), plus a
`git-version.h` derived from this repo (upstream generates it from git).

## 3. Target environment: UBI 10

- Build/runtime base: `registry.access.redhat.com/ubi10/ubi`.
- Expected packages: `weston` (runtime libweston + backends + xwayland
  module), `weston-devel`, `meson`, `gcc`, `pkgconf`, `wayland-devel`,
  `wayland-protocols-devel`, `libinput-devel`, `libevdev-devel`,
  `rpm-build`, `rpmdevtools`; `xorg-x11-server-Xwayland` at runtime.
- **Phase 0 validates this** (see §8): UBI repos are a subset of RHEL —
  `weston-devel` may need the CRB repo or an entitled build; CentOS
  Stream 10 repos are the unentitled CI fallback. Record the actual
  `rpm -ql weston-devel` output; it decides R2/R3 below.

## 4. Repository layout

```
example-weston-standalone/
├── meson.build              # root: project(), deps, config.h, version
├── meson_options.txt        # xwayland (default true), xserver path, paths
├── frontend/                # vendored, + meson.build (trimmed from upstream)
├── desktop-shell/           # vendored, + meson.build
├── shared/                  # vendored subset, + meson.build (static lib)
├── protocol/                # weston-desktop-shell.xml (+ fallback copies)
├── data/westonite.desktop
├── westonite.ini.in
├── rpm/westonite.spec
├── containers/Containerfile.build   # UBI10 build+test image
├── VENDOR.md                # upstream tag, import commit, local-patch log
└── PLAN.md
```

## 5. Build design (meson)

- `project('westonite', 'c')` versioned `14.0.2+westonite.N`.
- Resolve `dependency('libweston-14')`, `libinput`, `libevdev`, `dl`,
  `threads`, `wayland-server`, `wayland-scanner`, `wayland-protocols`.
- `config.h` via `configuration_data()`, providing exactly the macros the
  vendored files use: `VERSION`, `PACKAGE_*`, `BINDIR`, `LIBEXECDIR`,
  `MODULEDIR` (→ `$libdir/westonite`), `BUILD_XWAYLAND`, `XSERVER_PATH`,
  `WESTON_SHELL_CLIENT`, `HAVE_MEMFD_CREATE`, `HAVE_MKOSTEMP`,
  `HAVE_POSIX_FALLOCATE`, `HAVE_STRCHRNUL`, `HAVE_XWAYLAND_LISTENFD`
  (feature-probed like upstream), and one `BUILD_*_COMPOSITOR` per backend
  header found (probe: `cc.has_header('libweston/backend-drm.h', dependencies: dep_libweston)`).
- Targets, mirroring upstream structure:
  - `shared/` → static `libshared` (3 C files above).
  - `frontend/` → `libexec_westonite.so` (installed to `$libdir/westonite/`)
    + `westonite` executable (linked via `executable.c`, rpath to module dir).
  - `desktop-shell/` → `desktop-shell.so` in `$libdir/westonite/` —
    same filename as upstream but a different directory, so no conflict.

## 6. Renaming + local patches (kept minimal for 14.0.x rebases)

Rename map: binary `weston`→`westonite`; module dir `$libdir/weston`→
`$libdir/westonite`; config `weston.ini`→`westonite.ini`; session file
`weston.desktop`→`westonite.desktop`. Internal env vars (`WESTON_MODULE_MAP`
etc.) and libweston interfaces stay untouched.

Every source deviation from the verbatim 14.0.2 import is a discrete commit
logged in `VENDOR.md`:

1. **P1 — guard backend includes**: `main.c` includes all seven
   `<libweston/backend-*.h>` unconditionally while the *code* is already
   guarded by `BUILD_*_COMPOSITOR`; wrap each include in its existing guard
   so the port compiles against an RPM built with a backend subset.
2. **P2 — config filename**: `main.c` looks up `weston.ini`; switch the
   string to `westonite.ini` (same XDG search logic, `--config` unaffected).
3. **P3 — no-panel**: `shell.c` unconditionally idle-spawns
   `shell->client` (default `WESTON_SHELL_CLIENT`). Treat an **empty**
   `[shell] client=` value as "do not spawn", and set the built-in default
   to empty. Users can still point `client=` at a binary from the distro
   weston package if they ever want the panel back.
4. **P4 — no on-screen-keyboard client**: `text-backend.c`'s
   `[input-method] path` defaults to `weston-keyboard`, which we don't
   ship; default it to empty (it already handles "no input method" — the
   protocol support in `input-panel.c`/`text-backend.c` stays compiled in).

## 7. RPM spec (`rpm/westonite.spec`)

- `Name: westonite`, `Version: 14.0.2`, `Release: N%%{?dist}`, MIT.
- `BuildRequires`: meson, gcc, `pkgconfig(libweston-14)`,
  `pkgconfig(libinput)`, `pkgconfig(libevdev)`, `pkgconfig(wayland-server)`,
  `pkgconfig(wayland-scanner)`, `pkgconfig(wayland-protocols)`.
- `Requires`: `weston` (supplies libweston runtime, backends, renderers,
  xwayland module); `Recommends: xorg-x11-server-Xwayland`.
- `%%files`: `%%{_bindir}/westonite`, `%%{_libdir}/westonite/*.so`,
  `%%{_datadir}/wayland-sessions/westonite.desktop`, doc/example ini.
  No devel subpackage (we don't install plugin-SDK headers).
- Built with `rpmbuild -ba` in the UBI10 container; source tarball from
  `git archive`.

## 8. Phases

- **Phase 0 — environment truth** *(gates everything)*: UBI10
  `Containerfile.build`; confirm package names/availability of
  `weston-devel` (CRB? entitlement? CentOS Stream 10 fallback); capture
  `rpm -ql` for weston packages → finalize R1–R3.
- **Phase 1 — verbatim import + build**: import files from tag `14.0.2`
  (recorded in `VENDOR.md`), write meson, apply P1 only; compiles and
  `westonite --backend=headless-backend.so` runs in the container with
  desktop-shell loaded (log-verified).
- **Phase 2 — westonite identity + no-panel**: renames, P2–P4,
  `westonite.ini.in`, `.desktop` file; headless smoke test shows no
  attempts to spawn missing clients.
- **Phase 3 — Xwayland**: `BUILD_XWAYLAND` on, `xwayland-api.h` present or
  vendored per Phase 0 findings; smoke test: `xterm`/`xeyes` (or
  `xwininfo`) against westonite headless + Xwayland in the container.
- **Phase 4 — RPM**: spec, `rpmbuild` in container, install the RPM into a
  clean UBI10 image and rerun the Phase 2/3 smoke tests from the installed
  location.
- **Phase 5 — docs/CI**: README (build, run, config), optional GitHub
  Actions job running the container build + smoke tests.

## 9. Risks / open questions

- **R1 — UBI10 package availability**: `weston`/`weston-devel` may not be
  in unentitled UBI repos. Mitigation: CRB repo, entitled builds, or CentOS
  Stream 10 for CI. Phase 0 resolves.
- **R2 — backend header subset**: only headers for compiled-in backends are
  installed. P1 + meson probing makes any subset compile; runtime backend
  choice is validated in Phase 1.
- **R3 — Xwayland availability**: `xwayland-api.h` and the `xwayland.so`
  libweston module only exist if the RPM enabled xwayland (RHEL is expected
  to). If the header alone is missing, vendor it; if the *module* is
  missing, Xwayland cannot work against that RPM — escalate before Phase 3.
- **R4 — `weston-private.h` coupling**: it only needs installed headers
  (verified), but frontend↔libweston version skew is real: pin the port to
  the RPM's exact 14.0.x and rebase when the RPM moves.
- **R5 — screenshooter authorization**: `weston-screenshooter.c` references
  the screenshot client path (`weston-screenshoot` binding); without ported
  clients the binding degrades to a no-op — acceptable; revisit only if
  screenshots are wanted (the output-capture protocol support itself lives
  in libweston).
