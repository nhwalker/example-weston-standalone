# E2E test plan — westonite

Goal: thorough end-to-end coverage of what this repo actually ships —
the `westonite` frontend, the trimmed `desktop-shell.so`, and the local
patches (P0, P2, P3) — tested strictly **black-box**: no test hooks
compiled into shipped code, no vendored test harness. The compositor is
driven exactly as a user would drive it.

Decisions already made (2026-07-22):

- **Black-box scripts only**; the upstream weston test harness is *not*
  ported. The **VNC backend** (from EPEL's `weston-libs`) is the test
  control plane: one connection gives scripted input injection
  (pointer, keyboard) *and* framebuffer capture.
- **Scope: our code only.** The RPM's libweston (renderers, backends,
  protocol implementation, xwayland module) is treated as trusted
  infrastructure — we exercise it incidentally but do not write tests
  whose subject is RPM behavior.
- **Pixel tests: yes** — VNC framebuffer captures compared against
  reference images (pixman renderer for determinism).
- **CI: push only** — no scheduled/nightly jobs.

Capability inventory the tests are written against:
[`frontend-capabilities.md`](frontend-capabilities.md) and
[`desktop-shell-capabilities.md`](desktop-shell-capabilities.md). The
shell trims T1–T9 (`VENDOR.md`) are part of the contract: tests assert
the removals *stay* removed (no panel spawn, no fullscreen/maximize/
minimize capabilities, etc.).

---

## 1. Test control plane: the VNC backend

```
westonite --backend=vnc --renderer=pixman \
          --width=800 --height=600 --port=<per-instance> \
          --disable-transport-layer-security \
          --log=<per-test log>
```

Facts that shape the design (verified against the 14.0.1 sources):

- **Authentication is always required.** Even with TLS disabled,
  `vnc.c` calls `nvnc_enable_auth(NVNC_AUTH_REQUIRE_AUTH, …)` backed by
  `weston_authenticate_user()` → PAM service **`weston-remote-access`**,
  and the client must authenticate *as the user running westonite*.
  Test container setup therefore needs: a
  `/etc/pam.d/weston-remote-access` file (permissive `pam_permit.so` is
  acceptable in the ephemeral test container; `pam_unix.so` + a real
  password is the fallback if we want to exercise a realistic stack), a
  dedicated non-root test user, and the compositor run as that user.
- **One client per instance** — a second VNC connection kicks the
  first. Tests serialize connections per instance; parallelism comes
  from running multiple instances on distinct ports with distinct
  `XDG_RUNTIME_DIR` / `WAYLAND_DISPLAY`.
- **Security-type compatibility is the top risk** (spike S1, §6):
  neatvnc without TLS offers RSA-AES-style username/password security
  types; with TLS it offers VeNCrypt. Not every scriptable client
  speaks these.

### Client stack (spike S1 decides)

Candidates, in preference order:

1. **`vncdotool`** (Python, pip): mature scripting API — `move`,
   `click`, `key`, `type`, `captureScreen`, `expectScreen`. Must verify
   it negotiates a security type neatvnc offers.
2. **`asyncvnc`** or another modern Python client with RSA-AES /
   VeNCrypt support.
3. **Small custom `wnc-ctl` C tool on `libvncclient`**
   (`libvncserver-devel` is in EPEL; gnutls build speaks VeNCrypt).
   ~200 lines: connect, auth, inject events from a command list, dump
   framebuffer as PNG. Fully under our control — the guaranteed
   fallback.

S1 also decides whether the standard test config is no-TLS or
self-signed-TLS (generated at container build), driven purely by which
mode the chosen client supports.

### Test runner

`tests/e2e/` in this repo, **pytest**-based:

- `conftest.py` fixtures own instance lifecycle: launch with per-test
  config/log/runtime dirs, wait on log lines (not `sleep`) for
  readiness ("output enabled", VNC listening), teardown by SIGTERM,
  **assert exit code 0 on every teardown** (every test doubles as a
  clean-shutdown test), attach the log on failure.
- Helpers: `wait_for_log(pattern, deadline)`, `vnc` session wrapper,
  `screenshot()` → PNG, `assert_region_color()`,
  `assert_matches_reference(name, tolerance)`, `spawn_client()` for
  in-session Wayland/X clients.
- Output: JUnit XML for CI; failure screenshots + diffs + compositor
  logs saved as artifacts.
- The existing `scripts/smoke-test.sh` stays as the fast sanity gate;
  a new `scripts/e2e-test.sh` builds (reusing the build steps) and runs
  the pytest suite in the container.

Bash is deliberately dropped for the new suite: image comparison,
per-test fixtures, and VNC scripting are all painful in shell. Python 3
is already in the CentOS Stream 10 base image.

### In-session client drivers (test-only container deps)

- EPEL **`weston`** package demo clients (`weston-terminal`,
  `weston-simple-shm`, `weston-flower`, …): real xdg-shell clients with
  toytoolkit decorations — titlebar/edge drags exercise the shell's
  move/resize grabs. Installing the full `weston` package in the *test*
  container is fine (westonite is rename-isolated by design).
- **`wayland-info`** (`wayland-utils`): asserts advertised globals,
  output geometry/scale/transform.
- `WAYLAND_DEBUG=1` on a demo client, parsed by the runner: protocol-
  level assertions (e.g. `xdg_toplevel.wm_capabilities` is empty).
- X clients for Xwayland (`xdpyinfo`, `xeyes`/`xclock`, `xwininfo` from
  AppStream x11 utils).

---

## 2. Test inventory

Grouped by owner of the code under test. Tags: [pix] = pixel assertion,
[vnc] = needs VNC control plane, [x] = needs Xwayland.

### 2.1 Frontend: lifecycle, CLI, config (headless, no VNC)

| Test | Asserts |
|---|---|
| version/help | `--version`/`--help` exit 0, name is westonite |
| bad-cli | unknown option / `--backend=bogus` → non-zero exit, diagnostic in log |
| xdg-runtime-dir | missing/world-writable `XDG_RUNTIME_DIR` → refused with the upstream error text |
| socket-naming | `--socket=NAME` honored; two instances coexist via `add_socket_auto`; `WAYLAND_DISPLAY` seen by spawned client |
| config-discovery (P2) | `westonite.ini` found in `$XDG_CONFIG_HOME`, then `~/.config`; **negative**: a `weston.ini` alongside is ignored; `-c PATH` and `--no-config` behave |
| config-env | `WESTON_CONFIG_FILE` exported to child clients (observed via autolaunch env dump) |
| clean-shutdown | SIGTERM and SIGINT → exit 0, sockets removed (implicitly re-checked by every teardown) |
| log-file | `--log` creates/appends, timestamps present |

### 2.2 Frontend: child processes & autolaunch

| Test | Asserts |
|---|---|
| no-helpers (P3) | default config spawns **no** client processes (existing smoke check, ported) |
| shell-client-opt-in (P3) | `[shell] client=/path/stub` → spawned with wayland env; empty value → not spawned |
| autolaunch-positional | `westonite -- /path/stub` spawns the client |
| autolaunch-config | `[autolaunch] path=` spawns; `watch=true` → compositor **exits when the client exits** (the kiosk primitive); `watch=false` → client exit tolerated, compositor keeps running |
| sigchld | crashing child is reaped; compositor unaffected |

### 2.3 Frontend: backends & outputs

| Test | Asserts |
|---|---|
| headless-geometry | `--width/--height` reflected in `wayland-info` output mode |
| output-sections | `[output]` scale/transform for the headless/VNC output reflected in `wayland-info` |
| vnc-output-mode [vnc] | `[output] mode=WxH` for the vnc output → framebuffer received over VNC has exactly WxH |
| vnc-resize [vnc][pix] | `resizeable=true` + client-side resize → new framebuffer size, background repainted full-frame (exercises `handle_output_resized` → `shell_output_recreate_background`) |
| mirror-resize (P0) [vnc] | S2 spike: `mirror-of=` a resizeable VNC output, resize it — the P0 backport ("Fix crash in output resize handler") must hold: no crash, mirror keeps tracking |
| multi-backend | `--backends=headless,vnc` → both outputs advertised, both usable |

### 2.4 desktop-shell behavior [vnc]

| Test | Asserts |
|---|---|
| background-default [pix] | untouched config → solid `0xff002244` full-frame |
| background-config [pix] | `[shell] background-color=0xff336699` → that color, full-frame |
| background-resize [pix] | after VNC resize, new area fully covered (no stale/black bands) |
| window-map [pix] | `weston-simple-shm` → non-background region appears, fully inside output bounds (initial placement logic) |
| click-activate | two terminals; VNC click on each → typed keys land in the clicked one (visible echo in screenshot region) |
| move-grab [pix] | titlebar drag via VNC pointer → window's bounding region moves by the drag delta |
| resize-grab [pix] | edge drag → window region dimensions change |
| wm-capabilities (T6/T9) | `WAYLAND_DEBUG` on a demo client: `wm_capabilities` advertises **no** window-state requests |
| fullscreen-ignored (T6) [pix] | client requesting fullscreen (`weston-simple-egl -f` or toytoolkit fullscreen key) → geometry unchanged, request silently ignored |
| desktop-clicks | clicks on empty background are swallowed (focused window keeps focus, nothing crashes) |
| transient | demo client with popup/transient (e.g. toytoolkit menu) → child stacks above parent |
| unresponsive (T8) | SIGSTOP a focused client, click it → no move-grab starts, compositor stays healthy; SIGCONT recovers (best-effort: observable surface is "nothing bad happens") |

Out of scope: touch and tablet paths (VNC injects only pointer/keyboard;
no input hardware in CI), lock/idle (removed), animations (removed),
workspaces (single, upstream).

### 2.5 Xwayland [x]

| Test | Asserts |
|---|---|
| lazy-spawn | existing smoke: display advertised, Xwayland spawned on first connection, `xdpyinfo` round-trip — ported into pytest |
| x11-window [vnc][pix] | `xeyes`/`xclock` maps and renders over the background |
| x11-position-sync | drag the X window via VNC; `xwininfo -geometry` matches the new position (exercises `desktop_surface_set_xwayland_position` / `transform_handler`) |
| two-x-clients | second X client connects to the same lazily-started server |

### 2.6 Packaging & install e2e

| Test | Asserts |
|---|---|
| rpm-install (existing) | pristine Stream 10 container: RPM pulls `weston-libs` only; kept as-is |
| installed-suite | a tagged subset of the pytest suite (background pixel test, config discovery, autolaunch-watch, Xwayland round-trip) re-run against the **installed RPM** in the pristine container — same tests, different binaries |
| session-file | `wayland-sessions/westonite.desktop` passes `desktop-file-validate`; `Exec=` target exists |

---

## 3. Reference images & determinism

- Renderer pinned to **pixman**; output size pinned per test.
- Full-frame golden images only for client-free scenes (backgrounds) —
  those are exact solid colors, tolerance ~0.
- Anything containing client-rendered text (terminals) uses **geometric
  assertions** instead: bounding box of the non-background region,
  region deltas before/after an action, solid-color patch samples.
  This sidesteps font-stack nondeterminism entirely.
- References live in `tests/e2e/reference/`; regenerated only inside
  the canonical build container via `scripts/regen-references.sh`
  (documented in the README so a font/pixman bump in the base image is
  a conscious re-baseline, not silent drift).
- No sleeps: every wait is a log-line / screenshot-predicate poll with
  a deadline.

## 4. CI wiring (push only)

Extend `.github/workflows/ci.yml`:

1. build image (existing)
2. build + smoke (existing, fast gate)
3. **e2e suite**: `docker run … /src/scripts/e2e-test.sh` — JUnit
   output; on failure upload screenshots/diffs/compositor logs as
   artifacts
4. rpmbuild (existing)
5. pristine install test (existing) + **installed-suite** subset
6. RPM artifacts (existing)

Runtime budget: e2e stage ≤ ~5 minutes (dozens of tests, each a short-
lived compositor instance; instances are cheap headless/VNC processes).

## 5. Phases

- **E1 — control plane** — spike S1 (client/security-type compat, PAM
  setup, TLS-or-not decision), pytest skeleton, instance fixture, and
  the first two tests: `background-default`, `clean-shutdown`.
  *Exit criteria: a pixel-verified screenshot of the default background
  captured over authenticated VNC in the CI container.*
- **E2 — frontend suite** — §2.1 + §2.2 + §2.3 (except mirror-resize);
  port the three smoke assertions into pytest (keep the bash smoke as
  the quick gate).
- **E3 — shell suite** — §2.4, plus spike S2 (P0 mirror-resize
  reproduction recipe) and the mirror-resize test.
- **E4 — Xwayland + install** — §2.5, §2.6 installed-suite plumbing in
  `rpm-install-test.sh`.
- **E5 — CI + docs** — workflow wiring, artifact upload, README
  section, reference-image regeneration doc.

Each phase lands as an independently green PR; the suite is additive.

## 6. Risks & open questions

- **S1 (blocking E1): VNC security-type mismatch.** neatvnc's no-TLS
  auth types (RSA-AES family) are not universally supported by
  scriptable clients. Mitigations, in order: self-signed TLS +
  VeNCrypt-capable client; `libvncclient`-based custom tool (known to
  speak VeNCrypt when built with gnutls). If *all* of that fails —
  considered unlikely — the fallback control plane is the RDP backend
  (also in `weston-libs`, FreeRDP clients are scriptable), with the
  same test inventory.
- **S2: P0 reproduction recipe** — the exact mirror/resize sequence
  that hit the upstream crash needs to be reconstructed from upstream
  commit `51dfd1be`; if it needs the DRM backend it degrades to a
  "resize with mirror active doesn't crash" smoke on VNC.
- **Font/AA drift** in client screenshots — avoided structurally (§3).
- **PAM in container**: `pam_permit` for the test service keeps the
  suite independent of shadow-file handling in rootless containers;
  the installed-suite run can use `pam_unix` + a real password once,
  to prove the realistic stack.
- **Flakiness budget**: VNC framebuffer updates are asynchronous;
  every pixel assertion polls until match-or-deadline rather than
  asserting a single capture.
- Open: is the `[autolaunch] watch=` kiosk flow the primary production
  use-case? If so E2 should be promoted ahead of parts of E3, and the
  installed-suite subset should center on it.
- Open: preferred VNC test client language constraint, if any (pure
  Python keeps the container slim; the C fallback adds a build step)?
