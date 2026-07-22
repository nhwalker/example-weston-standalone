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
- **Everything lives in this repo and targets this repo.** All test
  code, references, and CI changes land in
  `example-weston-standalone`; the `weston` tree stays reference-only.
  The subjects under test are exclusively the artifacts this repo
  builds (`westonite`, `libexec_westonite.so`, our `desktop-shell.so`)
  and the RPM built from them — never the distro `weston` package. In
  particular the stock `weston` package is **not** installed in the
  test container, so there is no risk of accidentally exercising its
  binaries or its `desktop-shell.so` instead of ours; window-creating
  test drivers are a minimal client built in this repo (§1.3).

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
  `/etc/pam.d/weston-remote-access` file, a dedicated non-root test
  user, and the compositor run as that user. E1 verified that the
  realistic stack — `pam_unix.so` with a real password, checked via
  the setuid `unix_chkpwd` helper — works for a non-root compositor in
  the container, so the suite uses that rather than `pam_permit`
  (wrong-password rejection stays testable). `scripts/e2e-test.sh`
  owns this setup.
- **One client per instance** — a second VNC connection kicks the
  first. Tests serialize connections per instance; parallelism comes
  from running multiple instances on distinct ports with distinct
  `XDG_RUNTIME_DIR` / `WAYLAND_DISPLAY`.
- **Security-type compatibility is the top risk** (spike S1, §6):
  neatvnc without TLS offers RSA-AES-style username/password security
  types; with TLS it offers VeNCrypt. Not every scriptable client
  speaks these.

### Client stack (decided by spike S1 — see §6)

**`tests/e2e/support/vncclient.py`** — an in-repo pure-Python RFB
client. Off-the-shelf scriptable clients (`vncdotool`, `asyncvnc`)
cannot negotiate any security type neatvnc offers in no-TLS mode
(RSA-AES-256 / RSA-AES / Apple DH), so the suite carries its own
~200-line client implementing Apple DH auth on top of
`python3-cryptography` (BaseOS RPM — no pip anywhere in the test
stack), Raw-encoding full-frame capture, and pointer/keyboard event
injection. Standard test config is no-TLS
(`--disable-transport-layer-security`); auth still runs through the
full PAM + `pam_unix` stack as a dedicated non-root `e2e` user.

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

### In-session client drivers

- **`wtest-client` — a minimal xdg-shell test client built in this
  repo** (`tests/e2e/clients/`, plain `wayland-client` + `wl_shm`, no
  toolkit; test-only, never installed by the RPM). One small C program
  (~400 lines) that is the window-side counterpart of every shell
  test:
  - draws a solid-color window of a requested size (exact-color pixel
    assertions — no fonts, no AA, golden images stay byte-stable);
  - on pointer button press sends `xdg_toplevel.move` (or `.resize`
    with a chosen edge), so VNC click-and-drag exercises the shell's
    move/resize grabs without needing toolkit decorations;
  - repaints in a distinct color on keyboard focus enter/leave
    (focus/activation visible in the framebuffer);
  - prints received protocol state to stdout — `wm_capabilities`,
    configure sizes, `xdg_popup` results, ping/pong — so the runner
    asserts protocol facts by parsing client output rather than
    `WAYLAND_DEBUG` traces;
  - can request fullscreen/maximize on command, to prove the trimmed
    shell ignores them;
  - can go intentionally unresponsive (stop dispatching) on signal,
    for the T8 busy-grab test.
  This deliberately does *not* rely on the distro `weston` package's
  demo clients: installing that package would put a second
  `desktop-shell.so` and helper binaries in the container, exactly the
  ambiguity this repo exists to avoid.
- **`wayland-info`** (`wayland-utils`, a small standalone package):
  asserts advertised globals, output geometry/scale/transform.
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
| no-helpers (T3) | default config spawns **no** client processes (existing smoke check, ported) |
| shell-client-ignored (T3) | `[shell] client=/path/stub` is dead config: nothing spawned, startup undisturbed (T3 removed the helper-client machinery outright, superseding the P3 opt-in — discovered during E2; the plan's earlier opt-in row was stale) |
| autolaunch-positional | `westonite -- /path/stub` spawns the client |
| autolaunch-config | `[autolaunch] path=` spawns; `watch=true` → compositor **exits when the client exits** (the kiosk primitive); `watch=false` → client exit tolerated, compositor keeps running |
| sigchld | crashing child is reaped; compositor unaffected |

### 2.3 Frontend: backends & outputs

| Test | Asserts |
|---|---|
| headless-geometry | `--width/--height` reflected in `wayland-info` output mode |
| output-sections | `[output]` scale/transform for the headless/VNC output reflected in `wayland-info` |
| vnc-output-mode [vnc] | `[output] mode=WxH` for the vnc output → framebuffer received over VNC has exactly WxH |
| vnc-resize [vnc][pix] | **skip-marked** — client-initiated resize segfaults EPEL's neatvnc/weston-libs (see §6); test exists and re-enables when the RPM stack is fixed |
| mirror-resize (P0) [vnc] | S2 spike: `mirror-of=` a VNC output — trigger needs rethinking, VNC client-resize is unusable (§6) |
| multi-backend | `--backends=headless,vnc` → both outputs advertised, both usable |

### 2.4 desktop-shell behavior [vnc]

| Test | Asserts |
|---|---|
| background-default [pix] | untouched config → solid `0xff002244` full-frame |
| background-config [pix] | `[shell] background-color=0xff336699` → that color, full-frame |
| background-resize [pix] | blocked on the same RPM-side resize crash as vnc-resize (§6) |
| window-map [pix] | `wtest-client` (solid red, 200×150) → exactly that region appears, fully inside output bounds (initial placement logic) |
| click-activate [pix] | two `wtest-client` windows in distinct colors; VNC click on each → only the clicked one shows its focused color (activation + keyboard focus) |
| move-grab [pix] | click-hold on a `wtest-client` (which requests `xdg_toplevel.move` on button press) and drag via VNC → window region moves by the drag delta |
| resize-grab [pix] | same via `xdg_toplevel.resize` with a chosen edge → window region dimensions change; client's reported configure size matches |
| wm-capabilities (T6/T9) | `wtest-client` prints the `wm_capabilities` event: advertises **no** window-state requests |
| fullscreen-ignored (T6) [pix] | `wtest-client --request-fullscreen` (and `--request-maximize`) → geometry unchanged, no fullscreen/maximized state in configure events |
| desktop-clicks | clicks on empty background are swallowed (focused window keeps focus, nothing crashes) |
| transient [pix] | `wtest-client` opens an `xdg_popup` / sets a parent → child renders above parent within expected bounds |
| unresponsive (T8) | signal `wtest-client` to stop dispatching, click it → no move-grab starts, compositor stays healthy; resume recovers (observable surface is "nothing bad happens") |

Out of scope: touch and tablet paths (VNC injects only pointer/keyboard;
no input hardware in CI), lock/idle (removed), animations (removed),
workspaces (single, upstream).

### 2.5 Xwayland [x]

| Test | Asserts |
|---|---|
| lazy-spawn | display advertised before spawn, Xwayland spawned on first connection, `xdpyinfo` round-trip |
| x11-window [vnc][pix] | `wtest-xclient` maps and renders at exact size (EL10 ships no X demo apps; the in-repo xcb client replaces them *and* `xwininfo`) |
| x11-position-sync [vnc][pix] | X-reported root position converges on the on-screen content; titlebar drag moves the window with the X position following (exercises `desktop_surface_set_xwayland_position` / `transform_handler`) |
| two-x-clients [vnc][pix] | second X client connects to the same lazily-started server; both render |

### 2.6 Packaging & install e2e

| Test | Asserts |
|---|---|
| rpm-install (existing) | pristine Stream 10 container: RPM pulls `weston-libs` only; kept as-is |
| installed-suite | the `@pytest.mark.installed` subset (clean shutdown, both background pixel tests, P2 config discovery incl. the weston.ini negative, no-helpers, autolaunch-watch, Xwayland round-trip) re-run against the **installed RPM** in the pristine container — same tests, different binaries; excludes anything needing the build tree (wtest-client/xclient, wayland-info) |
| session-file | `wayland-sessions/westonite.desktop` passes `desktop-file-validate`; `Exec=` resolves in PATH |

---

## 3. Pixel determinism *(as implemented)*

- Renderer pinned to **pixman**; output size pinned per test.
- Every scene is composed of solid colors: the shell's background plus
  the flat-color windows of `wtest-client` / `wtest-xclient`. All
  pixel assertions are therefore *computed*, exact-match checks
  (`solid_color`, `region_of` bounding boxes) — no golden image files
  exist and none are needed, so there is no re-baselining workflow.
  No fonts, no antialiasing, no toolkit theming anywhere in the
  asserted pipeline (the xwm titlebar is the one themed element on
  screen, and nothing asserts its pixels).
- No sleeps on the happy path: every wait polls a log line, client
  stdout line, or capture predicate with a deadline. Fixed short
  sleeps appear only in negative tests ("nothing happens within 1 s").

## 4. CI wiring (push only) *(as implemented)*

`.github/workflows/ci.yml`, every push:

1. build image
2. build + smoke (fast gate)
3. **e2e suite**: `e2e-test.sh /results` — JUnit XML plus, per failed
   test, its whole working dir (compositor log, config, stub output)
   collected by a conftest hook
4. rpmbuild
5. pristine install test incl. the **installed subset**
   (`rpm-install-test.sh /rpms /src /results`)
6. artifacts: RPMs + `test-results` (uploaded `if: always()`)

Runtime budget: e2e stage ≤ ~5 minutes (dozens of tests, each a short-
lived compositor instance; instances are cheap headless/VNC processes).

## 5. Phases

- **E1 — control plane** ✅ *(done)* — spike S1 resolved (security
  types probed, in-repo Apple-DH client written and proven, `pam_unix`
  auth verified, no-TLS mode chosen); pytest skeleton
  (`tests/e2e/`, instance fixture with log/socket polling and
  exit-0-asserting teardown), `scripts/e2e-test.sh`, container test
  deps (`python3-pytest`, `python3-cryptography`). Tests green in the
  build container: `clean_shutdown_sigterm`, `clean_shutdown_sigint`,
  `clean_shutdown_vnc_backend`, `background_default` (pixel-exact
  `0xff002244` full-frame), `background_from_config` (pixel-exact
  `0xff336699` from `westonite.ini`). Exit criteria met.
- **E2 — frontend suite** ✅ *(done)* — §2.1 + §2.2 + §2.3 in
  `test_cli.py` / `test_children.py` / `test_outputs.py` (26 new
  tests): version/help/bad-CLI/XDG-runtime-dir, socket naming +
  two-instance coexistence, the full P2 config-discovery matrix
  (incl. the negative `weston.ini` test and `$HOME` fallback),
  helper-client policy (updated for T3: `[shell] client=` is dead
  config — the plan's opt-in row was stale), the autolaunch/kiosk
  matrix (spawn env incl. `WESTON_CONFIG_FILE`, watch=true session
  exit, crash tolerance, non-executable fatal, positional `--`
  command), headless/VNC output geometry & scale/transform via
  `wayland-info`, and multi-backend headless+vnc. VNC client-resize
  tests were written but are skip-marked: the resize path segfaults
  the RPM stack (see §6). `wayland-utils` added to the build image.
- **E3 — shell suite** ✅ *(done)* — `wtest-client` built
  (`tests/e2e/clients/`, behind `-De2e-test-client=true`, never
  installed; solid-color xdg-shell client with left-click→move /
  right-click→resize, focus-color switching, protocol-event stdout,
  SIGUSR1 unresponsive mode) and `test_shell_windows.py` (10 tests):
  window-map bounds, new-window focus, click-activation between two
  windows (pixel + protocol), pointer move grab (pixel-exact landing),
  pointer resize grab (growth + pixel/configure/commit consistency —
  exact deltas unassertable, see §6), empty `wm_capabilities` (T6/T9),
  fullscreen and maximize requests ignored (T6), background clicks
  swallowed, unresponsive-client handling (T8). Two findings recorded
  in §6 (VNC resize-grab half-delta quirk; grabs need the button held
  until the client's request reaches the shell — encoded in
  `grab_drag`). Deferred from §2.4: `transient` (initial placement is
  randomized, so parent/child overlap can't be arranged
  deterministically without more client machinery) and the
  blocked `background-resize`; S2 (P0) still open.
- **E4 — Xwayland + install** ✅ *(done)* — `wtest-xclient` added
  (xcb, solid-color, prints root-relative position on every
  ConfigureNotify — EL10 ships no xeyes/xclock/xwininfo, so the suite
  carries its own X client). `test_xwayland.py` (4 tests): lazy
  Xwayland spawn + `xdpyinfo` round-trip, X window renders at exact
  size with X-reported position converging on the on-screen content
  (xwm position sync), titlebar drag moves the window pixel-exactly
  with the X position following, and two simultaneous X clients.
  §2.6: `rpm-install-test.sh` now also validates the wayland-session
  file (`desktop-file-validate` + Exec lookup) and runs the
  `@pytest.mark.installed` subset (8 dependency-light tests incl.
  pixel-verified VNC backgrounds) against the installed RPM in the
  pristine container — verified end-to-end locally. Note for titlebar
  interactions: xwm handles frame clicks asynchronously, so drags must
  press-and-hold before moving (`titlebar_drag` helper).
- **E5 — CI + docs** ✅ *(done)* — results plumbed out of both
  containers (`scripts/e2e-test.sh` and `rpm-install-test.sh` take a
  results dir; a conftest hook copies failed tests' working dirs —
  compositor logs, configs, stub output — into it), JUnit XML + failure
  artifacts uploaded from CI as `test-results`, README gains a Testing
  section. No reference-image machinery was ever needed: the
  flat-color scene design made every pixel assertion computable
  (§3), so there is nothing to re-baseline.

Each phase lands as an independently green PR; the suite is additive.

## 6. Risks & open questions

- **S1 — RESOLVED (E1, 2026-07-22).** Empirically, EPEL 10's neatvnc
  0.9.0 with TLS disabled offers security types **129 (RSA-AES-256),
  5 (RSA-AES), 30 (Apple DH)** — no classic VNC auth, so `vncdotool`
  and `asyncvnc` both fail the handshake. Resolution: an in-repo
  ~200-line pure-Python RFB client (`tests/e2e/support/vncclient.py`)
  implementing **Apple DH** auth (needs only `python3-cryptography`
  from BaseOS — no pip), Raw-encoding capture, and pointer/keyboard
  injection. Verified against the live backend: authenticates through
  a real `pam_unix` stack as a non-root user (wrong password
  rejected), captures pixel-exact frames, injects input. TLS mode is
  unnecessary for the suite.
- **Found during E2 — client-initiated VNC resize crashes the RPM
  stack.** A VNC client sending `SetDesktopSize` segfaults the
  compositor (crash in neatvnc 0.9.0's raw-encoder worker,
  `pixel_to_cpixel`, via weston-libs 14.0.1's vnc backend) — even with
  `[output] resizeable=false`, and the server never advertises an
  ExtendedDesktopSize layout first. This is entirely inside EPEL's
  `weston-libs`/`neatvnc` (out of scope per the our-code-only
  decision), but note the operational implication: an authenticated
  VNC client can kill the session. Consequences for the suite: the
  `vnc-resize` (§2.3) and `background-resize` (§2.4) tests are marked
  skip with this reason (the RFB client retains its
  `set_desktop_size()` support for when EPEL ships a fix), and S2
  cannot use VNC resize as its trigger.
- **Found during E3 — VNC drag deltas reach resize grabs halved.**
  During an interactive resize drive over VNC, only every second
  pointer motion reaches the shell's resize grab and at half its
  delta (empirically: an 8-step +70,+50 drag yields sized configures
  matching motions 2/4/6/8 at half-delta, ending +35,+25), while the
  same event stream lands *move* grabs pixel-exactly. The shell's
  resize grab tracks whatever positions it is handed, so this lives in
  the RPM stack's input translation (vnc backend/neatvnc), not our
  code. Consequence: `resize-grab` asserts growth + pixel/configure/
  commit consistency instead of exact deltas. E5 stress runs showed
  the same event-dropping occasionally clips *move* drags short too
  (~1 in 10 runs), so drag-driven tests converge by re-dragging toward
  the target (`move_window_to`, resize retry loop) — every iteration
  is still a real grab; retries only absorb dropped events.
- **S2 — CLOSED as a documented gap (E5).** The P0 backport ("frontend:
  Fix crash in output resize handler", upstream `51dfd1be`) guards the
  mirror-of output-resize path. Black-box, the only way to resize an
  output at runtime is a VNC client `SetDesktopSize` — which crashes
  the RPM stack before our handler ever runs (see the E2 finding
  above). So the P0 path has **no reachable e2e trigger** on this RPM
  stack; its coverage remains the verbatim upstream backport itself
  (logged in `VENDOR.md`). Revisit when EPEL ships a neatvnc/weston
  with working client resize: un-skip `vnc-resize`, then add
  `mirror-of=` + resize as the P0 regression test.
- **Font/AA drift** in client screenshots — avoided structurally (§3).
- **PAM in container**: resolved with S1 — `pam_unix` + a real
  password works for a non-root compositor in the container, so the
  realistic stack is used everywhere.
- **Flakiness budget**: VNC framebuffer updates are asynchronous;
  every pixel assertion polls until match-or-deadline rather than
  asserting a single capture.
- ~~Open: autolaunch priority / client language~~ — both mooted by
  execution: the full autolaunch/kiosk matrix landed in E2 (with
  `watch=true` in the installed subset), and the client stack settled
  as pure Python for VNC plus two small in-repo C test clients.

## 7. Remaining coverage gaps (post-E5)

Everything in §2 is implemented except the following, each blocked or
deferred with its reason recorded above:

- `vnc-resize` / `background-resize` — skip-marked; RPM stack
  segfaults on client resize (E2 finding).
- P0 mirror-resize regression — no reachable black-box trigger (S2).
- `transient` popup/child stacking — randomized initial placement
  prevents deterministic overlap; needs extra client machinery if
  ever wanted.
- Touch/tablet input paths — VNC injects only pointer/keyboard; no
  input hardware in CI (out of scope by design).
- DRM backend on real hardware — out of scope by design.
