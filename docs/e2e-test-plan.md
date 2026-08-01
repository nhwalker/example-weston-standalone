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
| log-timestamps *(R2f)* | every line carries `[hh:mm:ss.mmm]`, continuation lines deliberately do not — the parity check that the Rust frontend goes through libweston's `"log"` scope rather than writing the file itself |
| flight-recorder-default *(R2f)* | absent `--flight-rec-scopes` starts the recorder from C's default list; an explicitly **empty** one disables it (absent ≠ empty) |
| logger-scopes *(R2f)* | `--logger-scopes=drm-backend` *replaces* the logger's `"log"` subscription, so ordinary output leaves the log file while the compositor still starts; `--logger-scopes=log` is the default spelled out and keeps it |
| wait-for-debugger *(R2f)* | `--wait-for-debugger` logs the pid and SIGSTOPs — asserted as process state `T` with no socket yet created, then resumed with SIGCONT |
| protocol-dump *(R2g)* | `--logger-scopes=proto` dumps every request and event, with arguments decoded (a string, two ints and a generic new-id in one `wl_registry.bind` line); **negative**: no subscriber, no dump, so one user's log never fills with another client's traffic |
| debug-protocol *(R2g)* | `--debug` makes `weston_debug_v1` appear in the registry; **negative**: absent by default, since the flag also lets every client screenshot every output |
| output-decorations *(R2g)* | `[core] output-decorations` reaches the headless backend — asserted through its failure mode (decorations need GL/Vulkan; the backend refuses them on noop/pixman), because a key that never arrived would let startup succeed |
| use-pixman-config-key *(R2g)* | `[core] use-pixman` + `renderer=gl` is "Conflicting renderer specifications" — the cheapest proof the *file* spelling is read at all, where before only the CLI flags were |

The R2f and R2g rows are **shared with the C oracle**, not `toml_only`:
this is libweston's own log machinery and both frontends wire it the
same way, so the oracle passing them is the parity proof.  Note
`logger-scopes` is testable only because the fixture waits on the
*wayland socket* rather than on a log line — a readiness check that
depends on logging could not observe logging being redirected.

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
| mirror-of [vnc] | toml-only (the C oracle aborts on a headless source, main.c:2543): `[output] mirror-of=` on the vnc head → the mirror comes up at the *source's* mode with client resize disabled and no spurious monitor-change line; passes in both `--backends` orders (the vnc-first one pins the deferral); fatal with no remote backend loaded |
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

### 2.7 Screenshooter & wcap recorder [vnc] *(added at R2e)*

Keys go in as QEMU extended key events — `vncclient.py` gained the
`-258` pseudo-encoding + `key_code()` (qnum keycodes) because the
server's keysym path never tracks the Super modifier
(`vnc_handle_key_event`: only Ctrl/Alt get STATE_UPDATE_AUTOMATIC).
Capture *completion* is blocked by the RPM-side abort documented in
§6.

| Test | Asserts |
|---|---|
| super-s-spawn | Super+S resolves the client via `WESTON_MODULE_MAP` (both frontends use `weston_module_path_from_env`) and logs the spawn; the in-flight slot frees after the attempt dies and a second Super+S spawns again |
| foreign-capture-denied | a `weston-screenshooter` the compositor did not spawn gets no capture (headless — see §6) and the compositor survives |
| super-r-wcap [pix] | Super+R starts the recorder (`capture.wcap` appears in the compositor cwd), damage produces frames, second Super+R stops it; header magic/format/geometry match the output |

### 2.8 Nested & streaming backends *(added at R2c-nested)*

No VM and no GPU: each backend runs against something the suite already
knows how to start.  All three take `--renderer=pixman` (they default
to GL; there is no GPU in the container).

| Test | Asserts |
|---|---|
| wayland-nested | a second westonite with `--backend=wayland` inside a headless one advertises `wayland0` and serves its own clients |
| wayland-count-size | `--output-count=2` creates `wayland0`/`wayland1`; `--width/--height` size them |
| x11-nested | `--backend=x11` as a client of the Xwayland *we* spawn advertises `screen0` (EL10 ships **no X.org server** — no Xvfb — so the compositor under test provides the X server) |
| x11-default-size | x11's own configure defaults, 1024x600, distinct from every other backend's |
| pipewire-output | with a pipewire daemon running, `--backend=pipewire` publishes the `pipewire` output (skipped where the daemon is absent) |
| rdp-refused [toml] | `--backend=rdp` is a startup error naming the product decision (C still builds RDP, so Rust-only) |
| remote-output-refused [toml] | `[[remote-output]]` is a startup error naming the product decision — the remoting plugin is dropped like RDP |
| pipewire-output-refused [toml] | `[[pipewire-output]]` — the pipewire virtual-output *plugin* — is a startup error naming the product decision |
| pipewire-backend-still-works | the other side of that line: `--backend=pipewire` (the *backend*, ported at R2c-nested) is **not** refused, so the drop cannot quietly widen |

Those last two guard a regression that actually happened: both sections
parsed into the config model and **nothing read them**, so asking for a
remote output got silence and no output — precisely the failure mode
the fail-loud rule exists to prevent.  A refusal without a test is how
that recurs.

Nested instances also fixed a harness assumption: the `westonite`
fixture now tears instances down in **reverse** creation order, since a
nested child dies non-zero if its host (and with it the parent display
or X server) is killed first.

### 2.9b Colour management *(added at R2c-color)*

`tests/e2e/test_color_management.py`, nine cases, all in the container
— no VM.  Mostly *validation*, because validation is what a port gets
wrong quietly.

| Test | Asserts |
|---|---|
| unknown-eotf / unknown-colorimetry | an unrecognised mode name is a startup error listing the valid ones |
| eotf-on-non-drm | eotf/colorimetry/characteristics on a non-DRM output are refused, not silently inert |
| icc-without-cm | an `icc-profile` without `[core] color-management` is refused — **C ignores it silently**, so this is a deliberate divergence |
| half-a-group | three of the six primaries is a config error (C's entirely-or-not-at-all rule) |
| out-of-range | a chromaticity outside 0..=1 is refused |
| missing-section | `color-characteristics = X` with no matching block is refused |
| allow-hdcp | accepted on any backend — C reads it everywhere, and it was missing from our model until R2c-color |
| vrr-mode/max-cll | both are now unknown keys: neither is read anywhere in 14.0.1 |

Ordering note worth keeping, found by a red test: the "these keys are
drm-only" refusal runs *before* the name and range validation, so on a
headless output it masks the error under test.  Those cases request
`--backend=drm` purely to get past that gate — which needs no VM, since
config validation precedes backend load.

### 2.9 DRM backend, in a VM *(added at R2c-drm-harness)*

The one backend that needs a kernel mode-setting device.  It gets one
from `scripts/drm-vm-test.sh`, which boots a throwaway VM carrying its
own kernel and loads `vkms` inside it — see **docs/drm-testing.md** for
the route, the on-the-runner approach it replaced, and what vkms does
not prove.

Since R2c-drm the harness runs them against **both** frontends —
`drm-vm-test.sh /results rust` then `... c` — and the CI job is a gate.
The Rust leg is self-proving about which binary ran: the harness feeds
it TOML, which the C frontend cannot parse, so the modeline test
landing on 1280x720 instead of the preferred 1024x768 could only have
come from the Rust port.

These tests are gated on `WESTONITE_DRM_VM=1`, deliberately **not** on
the presence of `/dev/dri`: a developer's machine has one, and taking
DRM master on it would black out their display.  So they are skipped
everywhere except inside the harness — the one place in this suite
where "skipped" is the normal outcome.

vkms presents a single connected connector, `Virtual-1`, preferred mode
1024x768@60.  Every expectation is anchored to that.

| Test | Asserts |
|---|---|
| head-enabled | `drm_heads_changed` enables the connected head and names the output after the connector |
| preferred-mode | the default `mode=preferred` picks 1024x768 |
| modeline | `[output] mode=1280x720` is passed through as a modeline and wins over the preferred mode |
| scale | `[output] scale=2` reaches the output through the DRM configure path |
| mode-off | `mode=off` prunes the head — it never appears to clients, and the "supposed to be pruned" assert is never reached |
| drm-device | `--drm-device=card0` selects the card |
| bad-drm-device | a device that does not exist is a startup error, not a silent fallback to the first card |
| libinput-hook-runs | the `configure_device` hook runs for every device even with no `[libinput]` section (C installs it unconditionally) — and with no section it touches nothing |
| libinput-pointer-settings | every pointer-side key the mouse supports lands, in C's order, checked as an exact ordered list |
| libinput-capability-gating | `enable-tap` on devices with no tap fingers, and `left-handed` on a keyboard, are skipped **silently and per device** |

The verdict comes from a `DRMVM-EXIT=<n>` sentinel on the guest
console, never from qemu's exit status: qemu exits 0 for a guest that
panicked, hung to the timeout, or never ran the tests at all.  JUnit XML
and the failure artifacts come back out of a second disk via `debugfs
dump`/`rdump`, so nothing on the host side needs to mount anything.

Input, added at R2c-input: the `[libinput]` section reaches libinput
only through this backend's `configure_device` hook, so this is the
only place it can be tested at all.  The harness therefore attaches a
`virtio-keyboard-pci` and a `virtio-mouse-pci` (a *relative* pointer —
`virtio-tablet-pci` is absolute and libinput offers it a smaller config
surface), and the guest runs **udevd**: libinput's udev backend
enumerates with `udev_enumerate_add_match_property(e, "ID_INPUT", "1")`
and nothing but udevd's `input_id` builtin sets that property, so
without it libinput sees no devices and the tests would pass vacuously.
The guest init asserts a non-zero tagged-device count rather than let
that happen quietly.  q35 also brings PS/2 emulations and an ACPI power
button along, so every assertion is scoped to a *named* device.

Second-order consequence, worth recording because it cost a full run:
once udevd coldplugs, it modprobes q35's emulated Bochs VGA, which
registers a second DRM card — and weston picked *that* one over vkms.
Every mode assertion failed against a connector called `Virtual-2`.
The fix is `-vga none` (remove the adapter) rather than
`--drm-device=card0`, so the tests keep exercising weston's own card
selection.

Harness note, found by a red test rather than by reading: a DRM output
advertises **every** mode the connector supports over `wl_output` (34
of them for vkms), where headless and vnc advertise one.  So "the first
`width:` line wayland-info prints" is the *preferred* mode, not the
active one — reading a DRM output's mode means finding the one whose
`flags:` say `current`.

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

Plus a separate `drm-vm` job (added at R2c-drm-harness): builds
`containers/Containerfile.drm-vm` on top of the build image and runs
`scripts/drm-vm-test.sh`, which boots the VM (KVM-accelerated where
`/dev/kvm` is available, TCG otherwise) and runs §2.9 inside it.  It is
`continue-on-error` while it exercises only the C oracle, and becomes a
gate in the PR that adds the Rust DRM leg.

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
- **Found during the R2c-virtualout probe — `pipewire-plugin.so`
  segfaults when no PipeWire daemon is running.** A `[pipewire-output]`
  section with the DRM backend loaded takes the compositor down with
  SIGSEGV inside the plugin (`segfault at 10 ... in
  pipewire-plugin.so`) rather than reporting a connection failure.
  Reproduced with the **C** frontend, so it is an RPM-side bug, not a
  port defect. Consequence for the suite: `[pipewire-output]` cannot be
  exercised until the guest rootfs ships the `pipewire` daemon, and
  even then the no-daemon path is a crash rather than an error to
  assert.
- **Found by reading during the same probe — upstream NULL-deref in
  `remoted_output_init` (main.c:3120).** When a `[remote-output]`
  section has no `mode=` (or `mode=off`), C logs
  `"Would not create a remoted output \"%s\""` passing `output->name`
  while `output` is still NULL. Any config that hits that branch
  crashes weston. The Rust port must use the section's own name there
  — a divergence from C that is a bug fix, and one to record as such.
- **Found during R2e — any weston-output-capture attempt aborts the
  compositor once a VNC peer is connected.** The vnc backend's
  repaint (`vnc_output_repaint`) reaches the renderer only indirectly
  through neatvnc's aml dispatch, so a capture task queued on the
  output can survive the repaint cycle and trip
  `assert(wl_list_empty(&ci->pending_capture_list))`
  (output-capture.c:193, weston-libs 14.0.1). Reproduced with the C
  frontend both for a Super+S-authorized screenshot and for a
  *denied* foreign client; without an RFB peer the foreign client
  hangs instead (VNC output powered off), and on headless the deny is
  clean server-side while the stock `weston-screenshooter` client
  aborts on its own `screenshot.c:101` zero-width assert. Entirely
  inside EPEL's weston-libs (our-code-only decision). Consequences
  for the suite (§2 test_screenshooter.py): Super+S is exercised with
  the client binary diverted via `WESTON_MODULE_MAP` (binding, gate,
  spawn, slot recycling — no capture committed), the authority-denial
  test runs on headless, and the wcap recorder (a renderer-hook
  mechanism, not output-capture) is verified end-to-end. Capture
  *completion* re-enables when EPEL ships a fixed backend.
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
