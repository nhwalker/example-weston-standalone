# Frontend (non-shell) capability inventory

Companion to `desktop-shell-capabilities.md`, covering everything else in
the repo: the `westonite` binary (`frontend/`) and the vendored `shared/`
support code. Written to assess what can be dropped next. Line numbers
refer to the state at trim T9.

Sizes: `frontend/main.c` 4838 lines, `xwayland.c` 267,
`weston-screenshooter.c` 153, `config-helpers.c` 94, `executable.c` 34;
`shared/` compiled subset 923 lines (`os-compatibility.c`,
`process-util.c`, `option-parser.c`).

Reminder of the split: **libweston (RPM) owns** rendering, input
delivery, protocol implementation, backends' internals, and the xwayland
module. **The frontend owns** picking and configuring all of that:
option/config parsing, backend/output/input configuration, process
management, and logging. Dropping frontend code removes the ability to
*configure or launch* a thing; the thing itself stays in the RPM.

---

## 1. Process & launch plumbing

- **Binary split**: `executable.c` (34 lines) is a stub calling
  `wet_main` in `libexec_westonite.so` — upstream structure kept so
  tests/tools could link the lib; we ship no such tools.
- **CLI/config basics**: option parsing (`shared/option-parser.c`),
  `westonite.ini` discovery + `WESTON_CONFIG_FILE` export (P2),
  `--config`/`--no-config`, `--version`, `--help` (`usage` 673),
  `--wait-for-debugger`, `--log`, XDG_RUNTIME_DIR sanity check
  (`verify_xdg_runtime_dir` 648).
- **Wayland socket**: `--socket`/`WAYLAND_DISPLAY` naming, plus
  `wl_display_add_socket_auto` fallback.
- **Autolaunch** (`execute_autolaunch` 4314): `westonite [--] /path/app`
  positional client, or `[autolaunch] path=` + `watch=` in the config —
  spawns a client at startup and (with watch) **exits the compositor
  when that client exits**. The kiosk-style "the app *is* the session"
  primitive.
- **Child-process machinery**: `wet_client_start`/`wet_process` +
  sigchld handling (`shared/process-util.c`, main.c). Users today:
  xwayland spawn, autolaunch, and the (dead, see §6) screenshooter
  launch.

## 2. Backend selection & configuration

`--backend`, `[core] backend=`, default `drm` (`WESTON_NATIVE_BACKEND`);
`weston_choose_default_backend` (1223) picks wayland/x11 automatically
when nested. **Multi-backend**: `--backends` loads several
simultaneously (weston 14 feature). Renderer choice: `--renderer`
(gl/pixman/noop), legacy `--use-gl`/`--use-pixman` per backend,
`[core] gbm-format=`.

Per-backend config blocks (each pairs CLI options + ini sections; the
`.so` itself comes from the RPM):

| Backend | Loader | Config surface |
|---|---|---|
| DRM (real hardware) | `load_drm_backend` (3375) | `--seat`, `--drm-device`, `--additional-devices`, `--current-mode`, `[core] require-input`, per-output DRM settings |
| headless | `load_headless_backend` (3472) | `--width/height`, `--no-outputs`, `--refresh-rate` |
| x11 (nested) | `load_x11_backend` (3924) | `--width/height/scale`, `--fullscreen`, `--output-count`, `--no-input` |
| wayland (nested) | `load_wayland_backend` (4044) | `--width/height/scale`, `--fullscreen`, `--sprawl`, `--display` |
| RDP (remote desktop server) | `load_rdp_backend` (3732) | `--port`, TLS cert/key, RemoteFX toggles, `[rdp]` section |
| VNC (remote desktop server) | `load_vnc_backend` (3857) | TLS cert/key, `[vnc]` section |
| PipeWire (virtual outputs) | `load_pipewire_backend` (3611) | `--width/height`, `[pipewire]` section |

## 3. Output management (the largest block, roughly main.c 1300–3350)

- **[output] sections**: mode (modeline/preferred/current/off), scale,
  transform (rotation/flips), position (explicit layout),
  per-output enable/disable; head↔output matching and hotplug
  (`wet_head_tracker`, `simple_head_enable` 2007,
  `drm_head_prepare_enable` 2764).
- **Clone mode**: `clone-of=` — same-CRTC clones on DRM
  (independent-CRTC clone explicitly unimplemented upstream, 2930).
- **Mirroring**: `mirror-of=` (`wet_config_find_head_to_mirror` 1759) —
  mirror one output onto another windowed/simple head, with
  resize tracking (this is what patch P0 fixed).
- **Color management / HDR**: `[core] color-management=` (loads the
  RPM's `color-lcms.so`), ICC profiles per output, EOTF + colorimetry
  modes, `[color_characteristics]` sections
  (`wet_output_set_color_characteristics` 1644).
- **Streaming/virtual outputs** (plugin loaders, plugins from the RPM):
  `load_remoting` (166) → `[remote-output]` gstreamer streams;
  `load_pipewire` (169) → `[pipewire-output]` streams.

## 4. Input configuration

- **[keyboard]**: XKB layout/model/variant/options/rules, repeat
  rate/delay, `numlock-on`, `vt-switching` toggle.
- **[libinput]**: tap, tap-and-drag(-lock), natural scrolling,
  left-handed, middle-emulation, rotation, accel profile/speed,
  scroll method/button, touchscreen `calibration_matrix`.
- **Touch calibrator**: `[libinput] touchscreen_calibrator=` +
  `save_touch_device_calibration` (1075) — enables libweston's
  calibration protocol; the interactive tool is upstream's
  `weston-touch-calibrator` client, **which we do not ship**.
- `--continue-without-input`, `[core] require-input`.

## 5. Xwayland glue (`frontend/xwayland.c`, `wet_load_xwayland` 4271)

Loads the RPM's `xwayland.so` API module, then lazily spawns
`/usr/bin/Xwayland` on first X client connection (listenfd or
display-socket handshake, SIGUSR1 ready signal, respawn on exit).
Config: `[core] xwayland=true` or `--xwayland`; `[xwayland] path=`.

## 6. Screenshot / recording / debugging

- **`weston-screenshooter.c`**: registers **Super+S** → spawns the
  upstream `weston-screenshooter` *client binary we do not ship* (dead
  end at runtime today), plus the screenshot **authorization hook** that
  would bless that client; and **Super+R** → libweston's built-in wcap
  screen **recorder** (writes `.wcap` files; decoder tool not shipped).
- **weston-log framework wiring** (main.c): `--log` file, `--debug`
  (enables the `weston-debug` protocol — needed by `weston-debug` CLI
  from the stock weston package), `--logger-scopes`,
  `--flight-rec-scopes` (in-memory flight recorder, on by default).
- The Super+Shift+Space **debug-key chain** is registered by the shell
  but implemented in libweston (documented in the shell inventory).

## 7. Core misc

- **Idle timeout**: `--idle-time` / `[core] idle-time` →
  `weston_compositor_set_idle_time`. **Inert since T3**: the idle/wake
  signals fire but nothing in westonite listens anymore (displays never
  sleep by decision). Config surface is dead weight.
- `[core] modules=` / `--modules` (`load_modules` 1032) — loads
  arbitrary `wet_module_init` plugins from `$libdir/westonite`; we ship
  none besides the shell.
- `--shell` / `[core] shell=` — which shell plugin to load (default
  `desktop-shell.so`; the RPM's stock shells are *not* in our module
  dir, so alternatives require installing them there).
- Primary-client tracking (`handle_primary_client_destroyed` 926) —
  used with `--socket`-less nested runs.
- `config-helpers.c` (94 lines) — typed config lookup helpers used by
  the output code.

## 8. `shared/` support code

`os-compatibility.c` (memfd/mkostemp fallbacks, cloexec helpers),
`process-util.c` (custom_env/fdstr for spawning), `option-parser.c`
(CLI). Pure support — shrinks only if its callers shrink.

---

## Drop assessment

**Dead or semi-dead today (cheapest wins):**

- **Screenshooter Super+S path** — spawns a binary we don't ship;
  the binding logs a failure and nothing happens. Drop the spawn+
  authorization (and decide on Super+R wcap recording — works, but
  produces files only upstream's unshipped `wcap-decode` reads).
- **Idle-time plumbing** (§7) — inert since T3; removing the option
  and `set_idle_time` call makes the "never sleeps" decision explicit.
- **Touch calibrator enablement** (§4) — protocol side only, tool not
  shipped. Dead unless the deployment sideloads the upstream tool.

**Deployment-dependent (need your answers):**

- **Remote-access backends** (RDP, VNC) and **PipeWire backend** —
  if never used, `load_rdp_backend`/`load_vnc_backend`/
  `load_pipewire_backend` + their option tables go (~400 lines), and
  the meson probe list shrinks. Runtime `.so`s stay in the RPM either
  way. Same question for **remoting/pipewire virtual-output plugin
  loaders** (§3).
- **Nested backends** (x11, wayland) — useful for development
  (running westonite in a window); dropping them saves ~250 lines but
  makes desktop-side testing harder. Recommend keeping.
- **Multi-backend `--backends`** — niche; simplifies `wet_load_backend`
  if dropped.
- **Clone/mirror multi-head logic** (§3) — several hundred lines; only
  needed for multi-monitor clone/mirror setups.
- **Color management / HDR config** (§3) — only if color-critical or
  HDR displays are in scope.
- **Autolaunch** (§1) — likely *the* mechanism this deployment wants
  (app-as-session); keep and possibly document prominently.
- **`--shell` / `modules=` genericity** (§7) — could hardcode
  desktop-shell and drop the plugin-loading indirection, at the cost of
  flexibility.

**Keep (load-bearing):** DRM + headless backends (production + CI),
Xwayland glue, output mode/scale/transform basics, [keyboard]/[libinput]
config, logging + debug scopes, `shared/` support, autolaunch.
