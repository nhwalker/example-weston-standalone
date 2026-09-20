# RecreatePlan.md — build `westonite` from the upstream Weston tree

An execution plan for an agent that has **only the upstream Weston
repository** (https://gitlab.freedesktop.org/wayland/weston, tag `14.0.1`)
as reference material. Following it produces `westonite`: a standalone
build of the Weston 14 compositor **frontend** and **desktop-shell**
plugin, linked against the distribution's libweston 14 RPMs, renamed,
trimmed to a pure window manager with a deliberately small frontend
surface, packaged as an RPM, and covered by a black-box end-to-end test
suite that runs in CI, including the DRM backend on a virtual KMS
device.

Every decision in this document is settled. Where the record it was
reconstructed from could not be re-verified, the gap is stated
(Appendix C) rather than papered over.

---

## 0. How to use this document

**Audience.** One agent, working in a fresh git repository, with:

- the upstream Weston repository checked out locally (reference only,
  never modified);
- `docker` (or `podman`) able to pull `quay.io/centos/centos:stream10`
  and reach the CentOS Stream 10 and EPEL 10 repositories;
- push access to a GitHub repository with Actions enabled.

**What you will produce.** A repository that builds `/usr/bin/westonite`,
`/usr/lib64/westonite/libexec_westonite.so` and
`/usr/lib64/westonite/desktop-shell.so`, installs a
`wayland-sessions/westonite.desktop` entry and an example config,
packages all of it as `westonite-14.0.1-1.el10.x86_64.rpm`, and verifies
it with a smoke script, a pytest e2e suite driven over the VNC backend
(85 tests in the build container), and a 10-test DRM module that runs
inside a VM carrying its own kernel.

**Phases.** Each phase ends green before the next starts.

| Phase | Delivers |
|---|---|
| 0 | Environment truth; the build image and the DRM VM image |
| 1 | Import, meson build, the P0 backport, the `westonite` rename, and the two shell trims without which the shell cannot build; first headless run |
| 2 | The test control plane: pytest harness, in-repo RFB client, test clients, the tests that pass on the untrimmed shell, CI skeleton |
| 3 | The remaining shell trims, each landing with the e2e tests that pin it; the window-management and Xwayland suites |
| 4 | The frontend trims (product decisions), each landing with a fail-loud refusal and its test; screenshooter/recorder tests |
| 5 | The DRM backend tested on vkms inside the VM; `drm-vm` CI job |
| 6 | RPM spec, pristine-container install test with the installed e2e subset |
| 7 | Documentation: README, VENDOR.md final, capability inventories, e2e and DRM test docs, the deferred maintenance-layer design |

**Rules that apply throughout.**

1. **Never modify the upstream Weston tree.**
2. **Every deviation from a verbatim upstream file is a discrete commit
   with an entry in `VENDOR.md`**: patches `P0`, `P1`; shell trims
   `T1`…`T8`; frontend trims `F1`…`F7`.
3. **Every phase and every trim ends green**: zero-warning build, the
   smoke script, and the e2e suite (once it exists) all passing in the
   build container. Do not stack unverified steps. The single stated
   exception is inside Phase 1 (§5.4).
4. **Fail loud.** Anything a user can ask for through the CLI or the
   config file that westonite does not do is a startup error naming the
   fact, never a silent no-op. Every refusal has an e2e test, because a
   refusal without a test is how silent no-ops come back.
5. **Scope is our code only.** Bugs inside the EPEL `weston-libs` RPM
   (libweston, backends, neatvnc) are documented and worked around,
   never fixed here and never made the subject of a test.
6. **Tests are black-box.** No test hooks in shipped code; the compositor
   is driven exactly as a user would drive it.
7. **Measure before you change.** The e2e control plane exists before
   any behaviour-changing trim lands, and each trim lands together with
   the test that pins it.
8. Commit messages describe what changed and what verification passed.

**Not in this plan**: porting libweston itself, any helper client, and
any language migration of the frontend or shell.

---

## 1. Fixed decisions

### 1.1 Scope and identity

| ID | Decision |
|---|---|
| D-SCOPE-1 | Port only `frontend/` (the `weston` binary) and `desktop-shell/`. libweston is consumed from the distro RPM and never built. |
| D-SCOPE-2 | Rename to **`westonite`**: binary, module dir `$libdir/westonite`, config `westonite.ini`, session file `westonite.desktop`. Internal env vars (`WESTON_MODULE_MAP`, `WESTON_CONFIG_FILE`, …) and all libweston interfaces stay untouched, so the result installs alongside the stock `weston` package with no file collisions. |
| D-SCOPE-3 | **No helper clients are ported or shipped**: no `weston-desktop-shell`, no `weston-keyboard`, no `clients/`. |
| D-SCOPE-4 | Not imported: `frontend/screen-share.c` (needs a libweston private header), `frontend/systemd-notify.c`, `frontend/text-backend.c`, `desktop-shell/input-panel.c`, `protocol/weston-desktop-shell.xml`. The last three are deleted by the shell trims anyway; importing them would be throwaway work. No `wayland-scanner` code is generated for the compositor at all. |
| D-SCOPE-5 | Xwayland support is enabled (`frontend/xwayland.c`; the RPM's `xwayland.so` loads at runtime). |
| D-SCOPE-6 | Upstream baseline is tag **`14.0.1`**, matching the EPEL 10 RPM. One fix from 14.0.2 is backported (P0). |
| D-SCOPE-7 | Target platform is RHEL 10 / UBI 10 / CentOS Stream 10 **with EPEL 10**. Unentitled CI builds on CentOS Stream 10; the same Containerfile accepts a UBI 10 base on an entitled host. |
| D-SCOPE-8 | The port is packaged as an RPM built inside the container from a `git archive` tarball. |
| D-SCOPE-9 | The production backend is DRM; headless, VNC, wayland, x11 and pipewire backends stay for testing, remote access and development. |

### 1.2 Desktop-shell trims (§7)

| ID | Decision |
|---|---|
| T1 | Remove input-panel / on-screen-keyboard support (the shell's references to the never-imported `input-panel.c` and `text-backend.c`). |
| T2 | Remove the helper-client protocol, lock screen, idle handling (**displays never sleep**), and all screen fades. The background becomes a compositor-side solid curtain per output, `[shell] background-color`, default `0xff002244`. |
| T3 | Remove all animations and their config keys. |
| T4 | Remove every hotkey binding and the `binding-modifier` option; keep only libweston's debug-key chain, hardcoded to Super. Window management is exclusively client-initiated (xdg-shell) plus click/touch-to-activate. |
| T5 | Remove fullscreen and maximize (drop both `weston_desktop_api` callbacks). Windows are free-floating and client-sized only. |
| T6 | Remove tablet-tool (pen) window management. Pens still work inside apps; **touch keeps** tap-to-activate and window dragging. |
| T7 | A left-click on an unresponsive window only activates it (no move grab). |
| T8 | Remove minimize. After this `wm_capabilities` advertises **no** window-state requests. |
| D-MAINT | A "maintenance layer" (operator-toggled hidden layer, minimize-like) is **designed and deferred**: write the design doc (§11.3), do not implement it. |

### 1.3 Frontend trims (§8) — product decisions

| ID | Decision |
|---|---|
| F1 | **No RDP backend.** VNC is the remote-access path. `--backend=rdp` is a startup error. |
| F2 | **No remoting plugin and no pipewire virtual-output plugin** (`[remote-output]`, `[pipewire-output]` sections are startup errors). Note the distinction: the **pipewire backend** (`--backend=pipewire`, screencasting) stays; only the DRM-side virtual-output *plugin* goes. |
| F3 | **No third-party module loading** (`--modules`, `[core] modules`): nothing westonite ships uses `wet_module_init`, and keeping it means a stable plugin ABI. Startup error. |
| F4 | **No touch calibrator** (`[libinput] touchscreen_calibrator`, `calibration_helper`): the interactive tool is a client westonite does not ship. Startup error. |
| F5 | **No Super+S screenshot client spawn** and no screenshot authority hook (the client is not shipped, and on this RPM stack any output capture with a VNC peer connected aborts the compositor). **Super+R wcap recording stays**, with its `--no-outputs` wild-pointer fallback fixed. |
| F6 | **No idle-time plumbing** (`--idle-time`, `[core] idle-time`): inert since T2. Startup error, making "never sleeps" explicit. |
| F7 | Keep everything else in the frontend as upstream has it: all remaining backends and their options, multi-backend `--backends`, clone-of and mirror-of, colour management, `[keyboard]`/`[libinput]` device settings, autolaunch, logging and debug scopes, `--shell`. |

### 1.4 Testing

| ID | Decision |
|---|---|
| D-TEST-1 | The **VNC backend** from EPEL's `weston-libs` is the test control plane: one authenticated RFB connection gives scripted pointer/keyboard injection and framebuffer capture. |
| D-TEST-2 | Pixel tests: yes, but every scene is flat-coloured, so all assertions are computed exact-match checks. **No reference images.** |
| D-TEST-3 | Renderer pinned to **pixman** everywhere in tests. |
| D-TEST-4 | The stock `weston` package is **never installed** in a test container. Window-creating test drivers are two minimal C clients built in this repo (`wtest-client`, `wtest-xclient`), gated behind a meson option, never installed. |
| D-TEST-5 | VNC auth runs through a **real `pam_unix` stack** as a dedicated non-root user, not `pam_permit`. |
| D-TEST-6 | The RFB client is an in-repo, pure-Python implementation of **Apple DH** auth plus QEMU extended key events (`python3-cryptography` from BaseOS; no pip). |
| D-TEST-7 | CI runs on push to `main` and on pull requests only. |
| D-TEST-8 | No sleeps on the happy path: every wait polls a log line, a client stdout line, or a capture predicate with a deadline. Fixed short sleeps appear only in negative tests. |
| D-TEST-9 | Every test asserts exit code 0 on teardown; every test is also a clean-shutdown test. |
| D-TEST-10 | The DRM backend is tested on `vkms` inside a VM that ships its own kernel; the tests are gated on an environment variable, never on `/dev/dri` existing. A one-time real-hardware validation stays an open item. |

---

## 2. Platform facts (what Phase 0 establishes)

1. **RHEL 10 / CentOS Stream 10 / public UBI 10 ship no weston at all.**
2. **weston 14 comes from EPEL 10**: `weston-14.0.1-3.el10_0`, subpackages
   `weston`, `weston-libs`, `weston-devel`, `weston-demo`, `weston-session`.
3. **`weston-devel` installs all seven backend headers**
   (`/usr/include/libweston-14/libweston/backend-{drm,headless,pipewire,rdp,vnc,wayland,x11}.h`)
   plus `xwayland-api.h`, `libweston.h`, `desktop.h`, `shell-utils.h`,
   `config-parser.h`, `weston-log.h`, `windowed-output-api.h`,
   `plugin-registry.h`, `matrix.h`, `zalloc.h`, `version.h`,
   `remoting-plugin.h`, `pipewire-plugin.h`; pkg-config `libweston-14.pc`,
   `weston.pc`, `libweston-14-protocols.pc`.
4. **`weston-libs` installs the full runtime**: `libweston-14.so.0.0.1`
   and, in `/usr/lib64/libweston-14/`: `drm-backend.so`,
   `headless-backend.so`, `wayland-backend.so`, `x11-backend.so`,
   `rdp-backend.so`, `vnc-backend.so`, `pipewire-backend.so`,
   `gl-renderer.so`, `color-lcms.so`, `xwayland.so`,
   `remoting-plugin.so`, `pipewire-plugin.so`.
5. **libweston 14 exports the config parser.** Upstream links `shared/`
   into libweston with `link_whole`, and `config-parser.c` carries
   `WL_EXPORT`, so `weston_config_*` symbols come from
   `libweston-14.so`. **Do not vendor `shared/config-parser.c`.**
   `os-compatibility.c`, `process-util.c` and `option-parser.c` have no
   `WL_EXPORT` and must be vendored; `parse_options()` is declared in
   the installed `<libweston/config-parser.h>`.
6. **Build deps**: `meson` 1.7 and `libinput-devel` live in **CRB** (must
   be enabled). `wayland-devel` 1.25, `wayland-protocols-devel` 1.49,
   `libevdev-devel`, `gcc` 14, `xorg-x11-server-Xwayland` 24.1 are in
   AppStream.
7. **Runtime needs CRB too**: `weston-libs` → `neatvnc` → `libturbojpeg`
   (CRB). A pristine install enables CRB before `dnf install`.
8. Between tags 14.0.1 and 14.0.2, only two files in the vendored set
   changed (verify with
   `git -C <weston> log 14.0.1..14.0.2 -- frontend/ desktop-shell/ shared/`):
   - `frontend/main.c` — upstream commit `51dfd1be` "frontend: Fix crash
     in output resize handler" → backport as **P0**.
   - `shared/config-parser.c` — `ee92a531` "shared: fix binding-modifier
     none". Lives inside the RPM's libweston; moot after T4 removes the
     option.
9. **`/tmp/.X11-unix` must exist before starting with `--xwayland`.**
   Bare containers lack it; without it the RPM's `xwayland.so` fails to
   bind and **segfaults in its error path** (upstream 14.0.1 bug). Every
   script that starts Xwayland does `mkdir -p -m 1777 /tmp/.X11-unix`.
10. **EL10 ships no X.org server**, only Xwayland. There is no Xvfb
    route; anything needing an X server gets it from a westonite running
    `--xwayland`.
11. **`vkms` is in CentOS Stream 10's `kernel-modules-core`**, and
    `qemu-kvm` is in AppStream, which is what makes the DRM VM cheap.
12. **libseat on EL10** has only the `logind` and `seatd` backends; there
    is no `builtin`. Non-logind environments run `seatd` and set
    `LIBSEAT_BACKEND=seatd`.

**Known bugs in the RPM stack** (documented, worked around, never tested
as a subject):

- **Client-initiated VNC resize crashes the compositor**: a
  `SetDesktopSize` request segfaults inside neatvnc 0.9.0's raw-encoder
  worker (`pixel_to_cpixel`), even with `[output] resizeable=false`.
  Operational implication: an authenticated VNC client can kill the
  session.
- **Any output-capture attempt aborts the compositor once a VNC peer is
  connected** (`assert(wl_list_empty(&ci->pending_capture_list))`,
  `output-capture.c`), authorised or denied alike. One reason for F5.
- **`pipewire-plugin.so` segfaults with no PipeWire daemon running.**
  Moot after F2.
- **neatvnc is a single-client server**: a second connection kicks the
  first, and a probe-connect that opens and closes the port is treated
  as a client whose teardown races the real client. Never probe the RFB
  port for readiness.
- **VNC drag deltas reach resize grabs halved** and every other motion;
  move grabs land exactly. Trailing drag motions are occasionally
  dropped (~1 in 10 runs).

Package versions observed in the build image (newer is fine while weston
stays 14.0.x): `weston-devel-14.0.1-3.el10_0`, `weston-libs-14.0.1-3.el10_0`,
`meson-1.7.2-1.el10`, `gcc-14.4.1-1.el10`, `wayland-devel-1.25.0-1.el10`,
`wayland-protocols-devel-1.49-2.el10`, `libinput-devel-1.30.1-2.el10`,
`libevdev-devel-1.13.1-6.el10`, `xorg-x11-server-Xwayland-24.1.9-6.el10`,
guest kernel `6.12.0-251` (CentOS Stream 10 `kernel-core`).

---

## 3. Final repository layout

```
example-weston-standalone/
├── .github/workflows/ci.yml       # §9.3
├── COPYING                        # upstream MIT text (verbatim)
├── LICENSE
├── PLAN.md                        # this document
├── README.md
├── VENDOR.md                      # provenance + P/T/F log
├── containers/
│   ├── Containerfile.build        # §4.2
│   ├── Containerfile.drm-vm       # §4.3
│   └── drm-vm-init.sh             # guest PID 1
├── data/meson.build  westonite.desktop  westonite.ini.example
├── desktop-shell/meson.build  shell.c  shell.h
├── docs/
│   ├── phase0-findings.md
│   ├── desktop-shell-capabilities.md
│   ├── frontend-capabilities.md
│   ├── maintenance-layer-plan.md  # deferred design (D-MAINT)
│   ├── e2e-test-plan.md
│   └── drm-testing.md
├── frontend/
│   ├── meson.build
│   ├── config-helpers.c  executable.c  main.c  weston-screenshooter.c  xwayland.c
│   └── weston.h  weston-private.h
├── git-version.h.meson
├── meson.build
├── meson_options.txt
├── rpm/westonite.spec
├── scripts/
│   ├── smoke-test.sh  e2e-test.sh  drm-vm-test.sh
│   └── rpm-build.sh  rpm-install-test.sh
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
        test_screenshooter.py  test_backends_nested.py  test_refusals.py
        test_backend_drm.py
```

---

## 4. Phase 0 — environment truth and the two images

### 4.1 Steps

1. In a `quay.io/centos/centos:stream10` container, after
   `dnf -y install epel-release && dnf config-manager --set-enabled crb`,
   confirm §2 items 1–7 with `dnf repoquery` / `dnf repoquery -l`.
2. Confirm §2 item 8 in the upstream tree.
3. Write and build `containers/Containerfile.build` (§4.2). Its final
   `RUN` fails the image build if the libweston dev environment is
   unusable.
4. Write and build `containers/Containerfile.drm-vm` (§4.3, contents in
   Appendix A.18). Confirm the guest rootfs has exactly one kernel and
   that `vkms.ko.xz` is present (the Containerfile asserts both).
5. Write `docs/phase0-findings.md`: headline results, RPM contents,
   dependency version table, container validation output, and the two
   image build commands.

### 4.2 `containers/Containerfile.build`

```dockerfile
# Build/test image for westonite.
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
        pipewire \
        pipewire-utils \
    && dnf clean all

# Sanity marker used by CI/smoke scripts: fail the image build early if the
# libweston 14 development environment is not actually usable.
RUN pkg-config --exists 'libweston-14 >= 14.0.1' \
    && test -e /usr/lib64/libweston-14/headless-backend.so \
    && test -e /usr/lib64/libweston-14/xwayland.so \
    && test -e /usr/include/libweston-14/libweston/xwayland-api.h

WORKDIR /src
```

Why each non-obvious package is there: `xorg-x11-server-Xwayland-devel`
provides `xwayland.pc` so meson detects `have_listenfd`
(`HAVE_XWAYLAND_LISTENFD`, avoiding Xwayland's deprecated `-listen` fd
path); `xdpyinfo` is the Xwayland round-trip probe; `wayland-utils`
gives `wayland-info`; `libxcb-devel` builds `wtest-xclient`;
`pipewire`/`pipewire-utils` run the daemon the pipewire-backend test
needs; `python3-cryptography` is the Apple DH primitive.

Build: `docker build -f containers/Containerfile.build -t westonite-build .`

### 4.3 `containers/Containerfile.drm-vm`

Derived `FROM westonite-build` (so the three container-based CI jobs do
not carry a kernel in their layer cache). Installs `qemu-kvm` (EL10
installs it as `/usr/libexec/qemu-kvm`; it is a full
`qemu-system-x86_64` and honours `-accel tcg`) and `e2fsprogs`
(`mkfs.ext4 -d` builds a filesystem image *from a directory* without
mounting; `debugfs` reads one back the same way — together they are why
the harness needs no privileges). Then builds a slim guest rootfs with
`dnf --installroot=/vm/rootfs --releasever=10 --enablerepo=crb
--setopt=install_weak_deps=False` of: `kernel-core`,
`kernel-modules-core` (has `vkms.ko`), `bash`, `coreutils`, `util-linux`,
`procps-ng`, `kmod`, `e2fsprogs`, `weston-libs`, `seatd`, `systemd-udev`
(for input-device properties only, §9), `wayland-utils`,
`python3-pytest`. Lifts the kernel and the generic initramfs to
`/vm/vmlinuz` and `/vm/initramfs.img`, asserts exactly one kernel,
copies the guest init in. Full file: Appendix A.18.

Build: `docker build -f containers/Containerfile.drm-vm -t westonite-drm-vm .`

### 4.4 Gate

Both images build; `docs/phase0-findings.md` committed.

---

## 5. Phase 1 — import, build, P0, rename, first run

**Goal.** `westonite --backend=headless` starts from the vendored sources,
loads the RPM's `headless-backend.so` and our `desktop-shell.so` from
`/usr/lib64/westonite/`, spawns nothing, and exits 0 on SIGTERM.

### 5.1 Import (verbatim, from upstream tag `14.0.1`)

| Destination | Upstream path |
|---|---|
| `frontend/main.c`, `executable.c`, `config-helpers.c`, `weston-screenshooter.c`, `xwayland.c`, `weston.h`, `weston-private.h` | `frontend/` |
| `desktop-shell/shell.c`, `shell.h` | `desktop-shell/` |
| `shared/os-compatibility.c`, `os-compatibility.h`, `process-util.c`, `process-util.h`, `option-parser.c` | `shared/` |
| `shared/helpers.h`, `string-helpers.h`, `xalloc.h`, `timespec-util.h`, `fd-util.h` | `shared/` |
| `git-version.h.meson` | `libweston/git-version.h.meson` |
| `COPYING` | `COPYING` |

Not imported (D-SCOPE-4): `frontend/screen-share.c`, `systemd-notify.c`,
`text-backend.c`; `desktop-shell/input-panel.c`;
`protocol/weston-desktop-shell.xml`; `clients/`;
`shared/config-parser.c`; `libweston/`. If the compiler demands another
`shared/*.h`, import it and add it to the table; the five headers above
were the complete set last time.

Use upstream's `weston.ini.in` and `weston.desktop` only as starting
points for the data files (§5.5).

Sizes at import: `main.c` 4838 lines, `shell.c` 5029, `xwayland.c` 267,
`weston-screenshooter.c` 153, `config-helpers.c` 94, `executable.c` 34,
`shell.h` 201.

### 5.2 Patch P0

Apply upstream commit `51dfd1be` ("frontend: Fix crash in output resize
handler") to `frontend/main.c`; it is the only 14.0.1→14.0.2 change to a
vendored file. The guarded code is the mirror-of resize path:
`simple_heads_output_sharing_resize()` calls
`wet_config_find_head_to_mirror()` and returns early if that yields NULL
before touching `head_to_mirror->output`. Confirm against
`git -C <weston> show 51dfd1be`.

### 5.3 Patch P1 — the config filename

In `frontend/main.c`, change the default config lookup string
(`const char *file = "weston.ini";` in the config-loading function) to
`westonite.ini`, and the two usage-text lines:

```
  -c, --config=FILE	Config file to load, defaults to westonite.ini
  --no-config		Do not read westonite.ini
```

Same XDG search logic; `--config` unaffected; `WESTON_CONFIG_FILE` still
exported to children. Leave other `weston.ini` mentions in libweston-
style log strings alone.

### 5.4 Shell trims T1 and T2 — required before the shell can build

`shell.c` as imported includes the generated `weston-desktop-shell`
protocol header and calls `text_backend_init()`, neither of which
exists here (D-SCOPE-4). So the shell's first successful build is
already post-T2. Do it in three commits — verbatim import, T1, T2 — and
let the compiler drive completeness; the frontend half (`shared/` +
`libexec_westonite.so` + `westonite`) builds independently and is the
first gate (§5.7). This is the one place where a trim cannot end on a
green build of its own.

**T1 — remove input-panel / on-screen-keyboard support.** In
`shell.c`/`shell.h`: the input-panel layer, `text_input` state, their
listeners, and the `text_backend_init`/`text_backend_destroy` calls. In
`frontend/weston.h`: the `struct text_backend` forward declaration and
the two prototypes. (About 16 lines in `shell.c`, 22 in `shell.h`.)

**T2 — remove the helper-client protocol, lock screen, idle handling,
fades; add the compositor-side background.** Delete from `shell.c`
(roughly 988 lines out, 66 in): the `weston_desktop_shell` global,
bind/unbind, client launch/respawn/crash handling
(`launch_desktop_shell_process`, `respawn_desktop_shell_process`,
`desktop_shell_client_destroy`, `check_desktop_shell_crash_too_early`),
every request handler (background, panel, panel position, lock surface,
grab surface, `desktop_ready`), `lock()`/`unlock()`/`resume_desktop()`,
the idle/wake listeners, the fade machinery (curtains, `shell_fade*`,
startup fade), the panel layer with its work-area and move-constraint
logic (`get_output_work_area()` stays and returns the full output),
grab-cursor feedback, the `[shell] client` read and the
`WESTON_SHELL_CLIENT` reference. Do not add a `shell-client-default`
meson option or a `WESTON_SHELL_CLIENT` define; nothing will use them.

Add the background:

```c
/* shell.h */
struct shell_output {
	struct desktop_shell  *shell;
	struct weston_output  *output;
	struct wl_listener    destroy_listener;
	struct wl_list        link;
	struct weston_curtain *background_curtain;
};
/* desktop_shell gains: struct weston_layer background_layer;
 *                      uint32_t background_color; */

/* shell.c, in shell_configuration(): */
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

Call it from `create_shell_output()`, from `handle_output_resized()`
(listener on `output_resized_signal`, looks the `shell_output` up from
the `weston_output`), and destroy the curtain with the output. The
curtain captures input, so clicks on empty desktop go nowhere. Write
against the installed 14 headers: 14's `weston_curtain_params` has a
`get_label` callback; newer upstream has a `char *label`.

### 5.5 Build and data files

All meson files are in Appendix A (A.1–A.6) in their final form; use
them as-is from the start, including the `e2e-test-client` option. Design
points:

- `shared/` → static `libshared` (three `.c` files);
  `frontend/` → `libexec_westonite.so` in `$libdir/westonite/` +
  `westonite` from `executable.c` with an rpath to the module dir;
  `desktop-shell/` → `desktop-shell.so` in `$libdir/westonite/` (same
  filename as upstream, different directory, no conflict) **with an
  explicit `pixman-1` dependency** (the RPM's `libweston-14.pc` keeps
  pixman in `Requires.private`).
- `config.h` provides exactly the macros the vendored files use;
  `LIBWESTON_MODULEDIR` is where `xwayland.so` is loaded from.
- Backend header probe for `drm headless pipewire vnc wayland x11`
  (`rdp` is omitted: F1 removes the include) — a tripwire that fails
  configure loudly if the RPM ever stops shipping a header.

`data/westonite.desktop`:

```ini
[Desktop Entry]
Name=Westonite
Comment=Standalone Weston-based Wayland compositor
Exec=westonite
Type=Application
```

`data/westonite.ini.example`: Appendix A.7.

### 5.6 `VENDOR.md`

Create it now; keep it current. Contents: source URL, tag, commit hash,
why (matches EPEL `weston-14.0.1-3.el10_0`), license; the import table;
the not-imported list with reasons; a note that all `meson.build` files
are written for this repo; `## Local patches` with one bullet per
P/T/F entry as they land; and `## Rebasing to a newer 14.0.x`:

1. `git -C <weston> diff 14.0.1..<new-tag> -- frontend/ desktop-shell/ shared/`
   (hunks touching `protocol/`, `input-panel.c`, `text-backend.c`,
   `screen-share.c` have nothing to apply to);
2. apply the hunks touching imported files (drop P0 if superseded);
3. update the tag/commit, the version in `meson.build` and
   `rpm/westonite.spec`; rebuild; rerun smoke and e2e.

### 5.7 Gates

1. After the import + P0 + P1 + meson commits: `libexec_westonite.so`
   and `westonite` build (temporarily comment out
   `subdir('desktop-shell')` locally to check; do not commit that).
2. After T1 + T2, inside the container:

```sh
meson setup build --prefix=/usr && ninja -C build && ninja -C build install
export XDG_RUNTIME_DIR=/tmp/xdg; mkdir -p -m 0700 $XDG_RUNTIME_DIR
timeout --preserve-status 5 westonite --backend=headless --log=/tmp/w.log; echo $?   # 0
grep "Loading module '/usr/lib64/westonite/desktop-shell.so'" /tmp/w.log
grep -c launching /tmp/w.log                                                     # 0
```

3. `--xwayland` round trip (needs `mkdir -p -m 1777 /tmp/.X11-unix`):

```sh
westonite --backend=headless --xwayland --log=/tmp/w3.log &
sleep 2
DISP=$(grep -oP 'listening on display \K:[0-9]+' /tmp/w3.log)
DISPLAY=$DISP xdpyinfo | grep 'vendor string'
kill %1; wait %1; echo $?                       # 0
grep "launching '/usr/bin/Xwayland'" /tmp/w3.log
```

Zero-warning build throughout.

---

## 6. Phase 2 — the test control plane

**Goal.** The harness, the two test clients, the smoke script, the tests
that already pass on the Phase-1 build, and a CI skeleton — all before
any further behaviour changes. From here on every trim lands with its
test.

### 6.1 The VNC control plane

Standard invocation:

```
westonite --backend=vnc --renderer=pixman --width=W --height=H \
          --port=<per-instance> --disable-transport-layer-security \
          --log=<per-test log> [--no-config | with $XDG_CONFIG_HOME/westonite.ini]
```

- **Authentication is always required**, even with TLS disabled:
  `vnc.c` calls `nvnc_enable_auth(NVNC_AUTH_REQUIRE_AUTH, …)` backed by
  `weston_authenticate_user()` → PAM service **`weston-remote-access`**,
  as the user running westonite. The container gets
  `/etc/pam.d/weston-remote-access` (`auth required pam_unix.so` /
  `account required pam_unix.so`), a non-root user `e2e` with password
  `westonite-e2e`, and the suite runs as that user via `runuser`. The
  setuid `unix_chkpwd` helper is what makes `pam_unix` work for a
  non-root process.
- **Security types offered with TLS off (neatvnc 0.9.0)**: 129
  (RSA-AES-256), 5 (RSA-AES), **30 (Apple DH)**. No classic VNC auth, so
  no off-the-shelf scriptable client works; the in-repo client speaks
  Apple DH.
- The VNC listener starts during backend load, strictly before the
  wayland socket is created, so "wayland socket exists" implies "VNC is
  listening". Readiness = socket exists; **never probe the port** (§2).
- One instance per test, on a free TCP port, with its own
  `XDG_RUNTIME_DIR`, `XDG_CONFIG_HOME`, log file, cwd and socket.
- **Super+X key bindings** are reachable only through QEMU Extended Key
  Events (keycodes): the server's keysym path tracks modifier state only
  for Ctrl/Alt (`vnc_handle_key_event`), while
  `vnc_handle_key_code_event` does. Keycodes are qnum codes mapped by
  neatvnc's `code_map_qnum_to_linux` (qnum `0xdb` → `KEY_LEFTMETA`).
  Pairs used: `SUPER_L = (0xffeb, 0xdb)`, `KEY_S = (0x073, 0x1f)`,
  `KEY_R = (0x072, 0x13)`. The server answers the SetEncodings with a
  one-time payload-less ack pseudo-rect that **consumes** an update
  request, so a pixel-less update must be followed by a fresh request.

### 6.2 Files

- `tests/e2e/support/vncclient.py` — Appendix A.14, verbatim. Apple DH:
  read generator (2 bytes), key length (2), prime, server public key;
  random private key mod prime; shared secret; AES key = MD5 of the
  shared secret (big-endian, key-length bytes); a 128-byte credential
  block (username in bytes 0–63, password in 64–127, NUL-terminated,
  unused bytes random) AES-128-ECB encrypted; send ciphertext + client
  public key. Pixel format 32 bpp little-endian BGRX; encodings Raw,
  DesktopSize, ExtendedDesktopSize, QEMU ext key (−258). `capture()`,
  `pointer/click/drag`, `key/key_tap` (keysym), `key_code(keysym,
  keycode, down)` (message 255/0), `set_desktop_size()` (unusable on
  this RPM, kept for later). Socket timeout 30 s: the Apple-DH + PAM
  handshake can take that long on loaded 2-core runners.
- `tests/e2e/support/compositor.py` — Appendix A.17. `Westonite`
  instance class: per-backend argv (`vnc` → pixman + port + no TLS;
  `wayland`/`x11`/`pipewire` → `--renderer=pixman`, they default to GL
  and the container has no GPU; `drm` → `--continue-without-input`),
  `--width/--height` for vnc/headless/wayland/x11, config written as
  `westonite.ini` in a per-instance `XDG_CONFIG_HOME` (else
  `--no-config`), `cwd` = the instance's work dir (so `capture.wcap`
  lands there), `wait_until()` with `WESTONITE_E2E_TIMEOUT_SCALE`
  multiplying every deadline, `wait_ready()` = process alive and a
  wayland socket exists, `vnc()` with one retry after 2 s on
  `TimeoutError`/`OSError`/`RfbError` (asserting the compositor is still
  alive first), `run_client(argv, check=True)`, `wait_exit()`,
  `terminate()` asserting exit 0 with the log in the message.
- `tests/e2e/conftest.py` — Appendix A.15: the `westonite` factory
  fixture; teardown in **reverse creation order** (a nested compositor
  dies non-zero if its host goes first), SIGTERM, assert 0, kill on
  timeout; the failure-artifact hook copying the test's `tmp_path` tree
  (regular files only) into `$WESTONITE_E2E_ARTIFACTS/<test>/`.
- `tests/e2e/support/client.py` — `WtestClient(compositor, *args,
  binary=None, extra_env=None)`: spawns `$WTEST_CLIENT` (or `binary`)
  with the compositor's client env, reads stdout on a daemon thread;
  `output()`, `count(regex)`, `wait_for_count(regex, n)`,
  `wait_for_line(regex)`, `wait_mapped()` → `(w, h)`, `pause()`
  (SIGUSR1, waits for `paused`), `resume()` (SIGUSR2, `resumed`),
  `terminate()`, context manager.
- `tests/e2e/support/image.py` — Appendix A.16: `pick_pixel`,
  `region_of` (exact-colour bounding box by per-row `bytes.find` of the
  BGR pattern at 4-byte alignment), `wait_for_region`, `solid_color`,
  `wait_for_solid_color` (never asserts a single capture; the first
  frames may predate the shell's first repaint).
- `tests/e2e/clients/` — Appendix A.6 for the build; specs below.
- `tests/e2e/pytest.ini` — the `installed` marker.
- `scripts/smoke-test.sh` (A.9), `scripts/e2e-test.sh` (A.13).

### 6.3 Test clients (built with `-De2e-test-client=true`, never installed)

**`wtest-client.c`** (~550 lines, plain `wayland-client` + `wl_shm`):

- Options: `--size WxH` (200x150), `--color AARRGGBB` (`ffcc0000`),
  `--focus-color AARRGGBB` (0 = none), `--interactive`, `--title T`,
  `--request-fullscreen`, `--request-maximized`.
- Binds `wl_compositor` v4, `wl_shm` v1, `wl_seat` ≤ v5, `xdg_wm_base`
  ≤ v6 (v5+ for `wm_capabilities`). Title + app id
  `org.westonite.wtest-client`; `set_fullscreen`/`set_maximized` before
  the first commit if requested.
- Draws a solid ARGB8888 buffer via `memfd_create` + `wl_shm` on every
  `xdg_surface.configure` (ack first), `focus_color` when
  keyboard-focused and non-zero, else `color`; destroys buffers on
  `release`.
- stdout, one line per event, flushed: `wm-capabilities: [ n … ]`,
  `configure: WxH [ state … ]` (`maximized|fullscreen|resizing|activated|other`;
  0x0 configures printed too), `mapped: WxH` (first configure),
  `focus: enter|leave`, `pointer: enter`, `pointer: button N`, `key: N`
  (press only), `close-requested`, `paused`, `resumed`.
- `--interactive`: `BTN_LEFT` press → `xdg_toplevel_move`; `BTN_RIGHT`
  press → `xdg_toplevel_resize(BOTTOM_RIGHT)`.
- SIGUSR1: print `paused`, stop dispatching entirely (unanswered pings)
  until SIGUSR2 (`resumed`). The loop is `poll`-based
  (`prepare_read`/`read_events`/`dispatch_pending`, 200 ms), not
  `wl_display_dispatch`, which restarts on EINTR.

**`wtest-xclient.c`** (~105 lines, xcb): `--size WxH` (200x150),
`--color RRGGBB` (`cc00cc`); InputOutput window with `back_pixel` =
colour and Exposure|StructureNotify mask, `WM_NAME` `wtest-xclient`;
prints `mapped` on MapNotify and `position: X Y WxH` (root-relative via
`xcb_translate_coordinates`) after map and on every ConfigureNotify.
EL10 has no `xeyes`/`xclock`/`xwininfo`.

### 6.4 Tests that land in this phase

Everything in §12 marked **P2**: lifecycle and signals, CLI and config
discovery, autolaunch, headless/VNC outputs, logging/debug/protocol
dump, Xwayland lazy spawn, nested backends, and the untrimmed-safe
window tests (map, focus on map, click activation, move/resize grabs,
X11 window render/sync/drag, two X clients). The background and
no-helper tests already pass too (T2 is in), so they land here as well.

### 6.5 CI skeleton — `.github/workflows/ci.yml`

Triggers `push: branches: [main]` and `pull_request:` (anything wider
runs every PR commit twice). Job `build-and-test`: checkout → build the
image → `smoke-test.sh` → `e2e-test.sh /results` → upload
`test-results/` as `test-results` with `if: always()`. The RPM steps
(§10) and the `drm-vm` job (§9) are added when those phases land. Final
file: Appendix A.12.

### 6.6 Gate

All P2 tests green in the container; CI green.

---

## 7. Phase 3 — the remaining shell trims, each with its tests

One commit per trim, zero-warning build, smoke + full e2e green, a
`VENDOR.md` entry, and the pinning tests (§12, column "lands") in the
same commit. Line counts are targets from the record, not requirements.

End state to aim at: `shell.c` 2239 lines, `shell.h` 108; `[shell]`
config has exactly one key, `background-color`; `shell_desktop_api` has
exactly `surface_added`, `surface_removed`, `committed`, `move`,
`resize`, `set_parent`, `ping_timeout`, `pong`, `set_xwayland_position`,
`get_position`; `shell_add_bindings()` registers exactly `BTN_LEFT` and
`BTN_RIGHT` → `click_to_activate_binding`, one touch binding →
`touch_to_activate_binding`, and
`weston_install_debug_key_binding(ec, MODIFIER_SUPER)`; the shell creates
zero `wl_global`s; it calls exactly two symbols from
`libexec_westonite.so`: `wet_get_config()` and `screenshooter_create()`.

**T3 — animations.** Remove window open (`zoom`/`fade`) and close
(`fade`) animations including the anim-fade surface refs in
`desktop_surface_removed`/`committed`, the focus dim-layer animation
with its per-workspace focus-surface curtains, `get_animation_type()`,
and the `animation`, `close-animation`, `startup-animation`,
`focus-animation` keys. (~252 lines.)

**T4 — hotkeys.** Remove zap (with `allow-zap`), mod+drag move/resize
triggers, maximize/fullscreen toggles, tiled-snap (with the orientation
state and its save/restore), rotate (grab, matrices, saved-rotation
restore, busy-grab right-click rotate), the mod+Tab switcher,
force-kill, backlight keys, surface opacity, and the `binding-modifier`
option. `shell_add_bindings()` keeps pointer/touch/tablet
click-to-activate and gains
`weston_install_debug_key_binding(ec, MODIFIER_SUPER);` (Super+Shift+Space
then a debug key toggles libweston's runtime debug aids). (~770 lines.)

**T5 — fullscreen and maximize.** Drop `.fullscreen_requested` and
`.maximized_requested` from `shell_desktop_api` (libweston-desktop then
omits both from `xdg_toplevel.wm_capabilities`; its NULL-callback guards
ignore legacy requests). Delete `set_/unset_fullscreen`,
`set_/unset_maximized`, the black letterbox curtains, the fullscreen
layer and `lower_fullscreen_layer()`, the maximize sizing/positioning
helpers, the shared saved-position restore, the `surface_state` struct,
the max/fullscreen guards in the grabs, and the output-resize window
re-fitting. The frontend `--fullscreen` option (nested-backend window
size) is unrelated. (~480 lines.)

**T6 — tablet tool.** Delete the pen tap-to-activate binding, the
tablet-tool move grab and its branch in `desktop_surface_move`, the
`shell_tablet_tool_grab` helpers, and the per-seat tool tracking and
focus-ping listeners. (~285 lines.)

**T7 — busy grab.** In the busy-cursor grab's button handler, drop the
`surface_move()` call: a left click only activates. Ping/pong and the
grab remain. (4 lines.)

**T8 — minimize.** Drop `.minimized_requested`; delete `set_minimized`,
the minimized layer, and the orphaned `surface_keyboard_focus_lost`/
`drop_focus_state` helpers. (~66 lines.)

Final `shell.h` structs after T8:

```c
struct workspace {
	struct weston_layer layer;
	struct wl_list focus_list;
	struct wl_listener seat_destroyed_listener;
};

struct shell_output { /* as in §5.4 */ };

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

Surviving exported helpers: `get_default_output`, `get_default_view`,
`get_shell_surface`, `get_current_workspace`, `get_output_work_area`,
`activate`, `shell_for_each_layer`. `shell.h` keeps
`<libweston/xwayland-api.h>` for the Xwayland position sync.

Gate: the whole §12 shell suite green (P2 + P3 rows), smoke green.

---

## 8. Phase 4 — frontend trims (product decisions), each fail-loud with a test

One commit per trim. Each adds a refusal in `frontend/main.c` that runs
**after the config file is loaded and before any backend loads**, logs
one line containing `is not supported by westonite`, and exits
non-zero; and adds its test to `tests/e2e/test_refusals.py` (§12).
Removed CLI options simply fall out of the option tables, so a leftover
flag hits C's existing `fatal: unhandled option: --x` path, which is
also loud.

A small helper keeps the refusals in one place:

```c
/* main.c: called from wet_main() right after the config is loaded. */
static int
refuse_unsupported(struct weston_config *config, const char *option_modules)
{
	struct weston_config_section *s = NULL;
	const char *name;
	char *str = NULL;
	int ival;
	bool bval;

	while (weston_config_next_section(config, &s, &name)) {
		if (!strcmp(name, "remote-output") ||
		    !strcmp(name, "pipewire-output")) {
			weston_log("fatal: [%s] is not supported by westonite\n", name);
			return -1;
		}
	}
	s = weston_config_get_section(config, "core", NULL, NULL);
	weston_config_section_get_string(s, "modules", &str, "");
	if (str[0] || (option_modules && option_modules[0])) {
		weston_log("fatal: third-party modules are not supported by westonite\n");
		free(str);
		return -1;
	}
	free(str);
	weston_config_section_get_int(s, "idle-time", &ival, -1);
	if (ival >= 0) {
		weston_log("fatal: idle-time is not supported by westonite (displays never sleep)\n");
		return -1;
	}
	s = weston_config_get_section(config, "libinput", NULL, NULL);
	weston_config_section_get_bool(s, "touchscreen_calibrator", &bval, false);
	weston_config_section_get_string(s, "calibration_helper", &str, "");
	if (bval || str[0]) {
		weston_log("fatal: the touch calibrator is not supported by westonite\n");
		free(str);
		return -1;
	}
	free(str);
	return 0;
}
```

**F1 — RDP.** In `load_backend()`, replace the `WESTON_BACKEND_RDP` case
with `weston_log("fatal: the rdp backend is not supported by westonite; use vnc\n"); return -1;`.
Delete `load_rdp_backend()`, `rdp_backend_output_configure()`,
`weston_rdp_backend_config_init()`, the `rdp_options[]` table, the RDP
`--help` block, and `#include <libweston/backend-rdp.h>`. The
`WESTON_BACKEND_RDP` enum references in the remote-output-type checks
(the "remote type of outputs" comparison and the `wet_output_handle_create`
switch) stay — the enum comes from `libweston.h`.

**F2 — remoting and pipewire-output plugins.** Delete `load_remoting()`,
`load_pipewire()`, their forward declarations, `remoted_output_init()`,
`pipewire_output_init()`, `drm_backend_remoted_output_configure()`,
`drm_backend_pipewire_output_configure()`, `load_additional_modules()`
and its call, the `drm_backend_loaded` field, and the
`remoting-plugin.h` / `pipewire-plugin.h` includes. The section refusal
in the helper covers the config side. Keep `load_pipewire_backend()`
and everything for `--backend=pipewire`.

**F3 — third-party modules.** Delete `wet_load_module()` (the
`wet_module_init` loader) and `load_modules()` and their two call sites.
Keep the `--modules` option in the table only so the helper can refuse
it by name; delete its `--help` line. `wet_load_shell()` and the
xwayland module load are untouched.

**F4 — touch calibrator.** Delete `save_touch_device_calibration()` and
the `touchscreen_calibrator` read +
`weston_compositor_enable_touch_calibrator()` call; the helper refuses
both keys.

**F5 — screenshot spawn.** In `frontend/weston-screenshooter.c` delete
`screenshooter_binding()`, `screenshooter_client_destroy()`,
`authorize_screenshooter()`, the `client`, `client_destroy_listener` and
`authorization` fields, the `KEY_S` binding registration and the
`weston_compositor_add_screenshot_authority()` call (and its
`wl_list_remove` in `screenshooter_destroy`). Delete
`wet_get_bindir_path()` from `main.c`/`weston.h` (its only user).
`wet_client_start()` stays (autolaunch uses it). In `recorder_binding()`
guard the fallback: if `ec->output_list` is empty, log `no output to
record` and return instead of `container_of` on an empty list.

**F6 — idle time.** Delete the `--idle-time`/`-i` option and its
`--help` line, the `idle_time` variable and the `[core] idle-time` read;
keep `wet.compositor->idle_time = 300;` (libweston's default; nothing
listens). The helper refuses the config key.

**F7** is the "keep" decision; nothing to do.

After F1–F6, `frontend-capabilities.md` (§11.2) describes the result.

Gate: all §12 P4 tests green, full suite green, smoke green.

---

## 9. Phase 5 — the DRM backend on vkms inside a VM

DRM is the production backend and the one that needs a kernel
mode-setting device, which no container and no GitHub-hosted runner
offers. The VM image from Phase 0 supplies one: it carries its own
kernel, loads `vkms` inside, and runs `test_backend_drm.py` there. The
only thing asked of the runner is `/dev/kvm`, and its absence falls back
to TCG.

Do **not** try to load `vkms` on the CI runner itself (the runner kernel
lacks it; `linux-modules-extra` provides it; the runner already has a
`hyperv_drm` card that is its own console and off limits; vkms registers
on the faux bus so driver-name lookups fail). The shape is wrong: host
kernel modules, device passthrough and a seat daemon, each failure
visible one CI round at a time and none of it reproducible locally.

### 9.1 Pieces (Appendix A.18–A.20)

| Piece | Where | What |
|---|---|---|
| VM image | `containers/Containerfile.drm-vm` | §4.3 |
| guest init | `containers/drm-vm-init.sh` | PID 1 (`init=/init.sh`; no systemd). Sets `PATH` (PID 1 inherits none), mounts proc/sys/devtmpfs/devpts/tmpfs, `modprobe vkms`, waits for `/dev/dri/card0`, starts `systemd-udevd --daemon` + `udevadm trigger` + `settle`, **asserts a non-zero count of `ID_INPUT=1` devices**, starts `seatd -g root` and waits for its socket, exports `LIBSEAT_BACKEND=seatd`, mounts the results disk **by label**, runs `/opt/vm-command.sh`, then writes `<2>DRMVM-EXIT=<n>` to **`/dev/kmsg`** and the console, sleeps 1 s, sysrq power-off |
| harness | `scripts/drm-vm-test.sh` | Builds westonite, `cp -a` the baked rootfs (never hardlinks), `DESTDIR` install into it, injects the working tree's `drm-vm-init.sh`, the e2e tree and the two test clients, writes the payload script, `mkfs.ext4 -d` root (2G) + 64M results image, boots qemu, reads the sentinel, extracts JUnit and failure dirs with `debugfs` |

qemu line: `-accel kvm|tcg -M q35 -cpu max -m 3G -smp 2 -no-reboot
-display none -vga none -serial stdio -kernel … -initrd … -drive
root,if=virtio -drive results,if=virtio -device virtio-keyboard-pci
-device virtio-mouse-pci -append "root=LABEL=drmvm-root rw
console=ttyS0,115200 init=/init.sh selinux=0 panic=10
rd.emergency=poweroff"`, under `timeout 900`.

### 9.2 Load-bearing details

1. **The verdict is the `DRMVM-EXIT=<n>` sentinel, never qemu's exit
   status** — qemu exits 0 for a guest that panicked, hung, or never ran
   the tests. No sentinel is always a failure.
2. **The sentinel goes through `/dev/kmsg`.** A userspace console write
   is tty-buffered and flushed asynchronously; a printk can land in the
   middle of it (`DRMVM: DRMVM-EXI[ 6.082274] sysrq: Power Off` / `T=0`
   is what a KVM-fast run produces). printk emits records whole. The
   host takes the **first** match and requires at least one digit.
3. **`-vga none`, not just `-display none`**: q35 gives every guest a
   Bochs VGA; once udevd coldplugs it gets modprobed as a second DRM
   card, and weston picks it over vkms (connector `Virtual-2`). Remove
   the adapter rather than pass `--drm-device`, so the tests exercise
   weston's own card selection.
4. **udevd is there for input only.** devtmpfs creates `/dev/dri/cardN`;
   libinput's udev backend enumerates with
   `add_match_property("ID_INPUT", "1")`, which only udevd's `input_id`
   builtin sets. Without it the `[libinput]` tests pass vacuously.
5. **`virtio-keyboard-pci` + `virtio-mouse-pci`** (a *relative* pointer;
   `virtio-tablet-pci` is absolute and gets a smaller libinput surface).
   q35 also brings PS/2 emulations and an ACPI button, so every
   assertion is per named device.
6. **No privileges**: `mkfs.ext4 -d` and `debugfs -R dump/rdump` do the
   filesystem work without mounting.
7. `virtio_blk` and `ext4` are modules in the EL10 kernel, so the
   distro-generated generic initramfs is required; `init=` is honoured
   across switch-root; root by `root=LABEL=drmvm-root`.
8. **TCG timing**: boot ~35 s; the *first* compositor start ~22 s (cold
   page cache + llvmpipe EGL init; later starts 1.3–2 s). The harness
   exports `WESTONITE_E2E_TIMEOUT_SCALE=4` under TCG, `1` under KVM.
9. A DRM output advertises **every** connector mode (34 for vkms); the
   active one is the mode whose `flags:` say `current`.
10. Tests are gated on `WESTONITE_DRM_VM=1`, never on `/dev/dri`: a
    developer's workstation has one, and taking DRM master on it would
    black out their display.

### 9.3 Tests and CI

`tests/e2e/test_backend_drm.py` — §12, ten tests. CI job `drm-vm`
(Appendix A.12): build both images, chmod `/dev/kvm` 0666 when present
and pass `--device /dev/kvm`, run `drm-vm-test.sh /results c`, upload
`test-results-drm-vm`. Locally:

```sh
docker build -f containers/Containerfile.build  -t westonite-build .
docker build -f containers/Containerfile.drm-vm -t westonite-drm-vm .
docker run --rm --device /dev/kvm -v "$PWD":/src -v "$PWD/test-results":/results \
    westonite-drm-vm /src/scripts/drm-vm-test.sh /results c
```

Editing `drm-vm-init.sh` or the tests needs no image rebuild (the
harness injects the working tree's copies); only the guest package set
does.

**What this does not prove.** vkms supports atomic modesetting and GBM
modifiers and brings a full output up on llvmpipe, but its modes are a
synthesised list, not EDID; no physical vblank, no hardware timing, no
driver quirks. Passing on vkms means the DRM path is wired correctly and
does not crash. Record the one-time real-hardware validation as an open
item in `docs/drm-testing.md`.

Gate: sentinel `DRMVM-EXIT=0`, 10 passed, CI job green.

---

## 10. Phase 6 — RPM and the pristine install test

The spec is Appendix A.8. Design: `Name: westonite`, `Version: 14.0.1`,
`Release: 1%{?dist}`, MIT; `BuildRequires` meson/gcc and the pkg-config
set (`libweston-14 >= 14.0.1`, `wayland-server`, `wayland-scanner`,
`wayland-protocols >= 1.33`, `libinput`, `libevdev`, `pixman-1`,
`xwayland`); `Requires: weston-libs%{?_isa} >= 14.0.1` (**not** the full
`weston`); `Recommends: xorg-x11-server-Xwayland`; stock `%meson` macros;
`%files` = binary, `%dir %{_libdir}/westonite`, the two `.so`s, the
session file, the example ini, `%license COPYING`, `%doc VENDOR.md`. No
devel subpackage.

`scripts/rpm-build.sh` (A.10): `git archive --prefix=westonite-14.0.1/`
→ `rpmbuild --define '_topdir /tmp/rpm' -ba`; needs
`git config --global --add safe.directory /src` because the bind mount
is owned by another user.

`scripts/rpm-install-test.sh` (A.11), in a bare
`quay.io/centos/centos:stream10` container: `epel-release`, **enable
CRB**, install the RPM + `xorg-x11-server-Xwayland xdpyinfo
python3-pytest python3-cryptography desktop-file-utils util-linux`;
assert `rpm -q weston` fails; `desktop-file-validate` the session file
and resolve its `Exec=` in `PATH`; the headless + `--xwayland` +
`xdpyinfo` check from §5.7; then the `@pytest.mark.installed` subset
(§12, eight tests) as the `e2e` user against the installed files.

CI: add the "Build the RPM", "Install-test the RPM in a pristine
container" and "Upload RPMs" steps to `build-and-test` (A.12).

Gate: `westonite-14.0.1-1.el10.x86_64.rpm` (+debuginfo, debugsource)
built; pristine install test passes; CI green.

---

## 11. Phase 7 — documentation

Write these once, describing the final state.

### 11.1 `README.md`

What westonite is (frontend + desktop-shell, built against libweston 14
RPMs, renamed; upstream `14.0.1` matching EPEL's `weston-14.0.1-3.el10_0`;
target platform; links to `VENDOR.md`, `docs/phase0-findings.md`,
`PLAN.md`); what you get (the three installed artifacts, session file,
example config; backends/renderers/Xwayland module come from
`weston-libs` at `/usr/lib64/libweston-14/`); no helper clients, no
panel, no on-screen keyboard, and none can be re-enabled; what is
deliberately not supported (F1–F6, one line each); **Required
repositories** table (BaseOS/AppStream, CRB, EPEL 10; build vs runtime;
the CRB-at-runtime `libturbojpeg` note); **Building**; **Building the
RPM**; **Running** (`westonite`, `--backend=headless`, `--xwayland`;
config search; the `/tmp/.X11-unix` note); **Testing** (VNC control
plane, in-repo RFB client, PAM stack, the two test clients, flat-colour
pixel assertions, no sleeps, the DRM VM, CI artifacts); **License**.

### 11.2 Capability inventories

`docs/desktop-shell-capabilities.md`: what the shell is (a pure
window-manager plugin), what upstream's has that this one does not
(T1–T8 in prose), then numbered groups with `file:line` anchors:
built-in background; core window management (the `weston_desktop_api`
entries: surface lifecycle, client-initiated move/resize via the pointer
and touch grabs — resize is pointer-only, upstream never implemented
touch resize; transient/parent surfaces and child layer syncing;
Xwayland integration via `set_xwayland_position` and
`transform_handler`; unresponsive-client handling with the busy-cursor
grab and no special sprite; activation and focus; cursor/workspace/
background layers); input bindings; multi-output/hotplug/session
(`desktop_shell_notify_session` re-syncs activation on VT switch); seat
management. Close with observations for future trimming (busy-cursor
grab and touch move grab are cheap and self-contained; focus-state
tracking and Xwayland positioning are entangled; session-notify matters
only on DRM).

`docs/frontend-capabilities.md`: the libweston/frontend split reminder;
process and launch plumbing (binary split, CLI/config, socket naming,
**autolaunch** — `westonite [--] /path/app` or `[autolaunch] path=` +
`watch=`, the kiosk primitive; child-process machinery); backend
selection and the per-backend config table (drm, headless, x11,
wayland, vnc, pipewire); output management (`[output]` sections,
clone-of, mirror-of, colour management/HDR); input configuration
(`[keyboard]`, `[libinput]` device settings — DRM only, via
`configure_device`); Xwayland glue; recording and debugging (Super+R
wcap recorder; weston-log wiring: `--log`, `--debug`, `--logger-scopes`,
`--flight-rec-scopes`; the debug-key chain); core misc (`--shell`,
primary-client tracking, `config-helpers.c`); `shared/`; and a final
section listing F1–F6 as the removed surface with their refusal
messages.

### 11.3 `docs/maintenance-layer-plan.md` (design only)

Status: designed, deferred. Concept: a "maintenance" `weston_layer`
above the workspace, toggled by an operator hotkey; windows there are
fully interactive; while active, everything below is dimmed by one
translucent curtain per output (`capture_input = true`) and blocked
from input; while off, the layer is unpositioned so its windows are
unmapped (minimize-like, no client involvement). Stack when active:
cursor → maintenance → dim curtain → workspace → background. Controls:
Super+M toggles the focused window between workspace and maintenance;
Super+Shift+M toggles the mode. Hotkeys are the only way in or out
(`minimized_requested` stays absent). Details: a per-surface
`in_maintenance` flag respected by `shell_surface_calculate_layer_link`;
focus hygiene on enter/leave; transient children follow their parent via
the existing child-layer syncing; `[shell] maintenance-dim=0xAARRGGBB`
default `0x99000000`; reuse `WESTON_LAYER_POSITION_UI`. Caveats: Xwayland
iconify behaviour (check `xwayland/window-manager.c`), non-transient new
toplevels from a maintenance app map to the workspace, multi-seat mode
state (proposed: global). Estimate ~200–250 lines in `shell.c`. Log as
feature F-M1 in `VENDOR.md` if ever implemented.

### 11.4 `docs/e2e-test-plan.md` and `docs/drm-testing.md`

The e2e plan: decisions (D-TEST-*), the control plane facts (§6.1), the
client stack, the runner design, the inventory (§12), pixel
determinism, CI wiring, the RPM-stack findings (§2) and the coverage
gaps (VNC resize and the P0 mirror-resize path have no reachable
black-box trigger on this RPM stack; transient popup stacking needs
deterministic placement the client machinery does not have; touch and
tablet paths have no input hardware in CI; DRM on real hardware is
manual).

`docs/drm-testing.md`: why a VM, the pieces, why udev is there, how to
run it, how to read the result, speed, and "what this does not prove".

### 11.5 `VENDOR.md` final and `PLAN.md`

`VENDOR.md` lists P0, P1, T1–T8, F1–F6 with one bullet each. Commit
this document as `PLAN.md`.

---

## 12. Test inventory

Column **lands**: the phase whose commit adds the test (P2 = §6, P3 =
§7 with the trim named, P4 = §8 with the trim named, P5 = §9).
**[I]** = `@pytest.mark.installed` (the eight tests the pristine
container re-runs against the RPM). Totals: 85 tests collected in the
container, 84 pass, 1 skip; 10 in the VM.

**`test_lifecycle.py`**

| Test | Asserts | lands |
|---|---|---|
| `test_clean_shutdown_sigterm` [I] | headless; `terminate(SIGTERM)` exits 0 | P2 |
| `test_clean_shutdown_sigint` | same with SIGINT | P2 |
| `test_clean_shutdown_sigusr2` | same with SIGUSR2 (a first-class termination signal in C, sharing `on_term_signal` with SIGTERM) | P2 |
| `test_sigterm_logs_caught_signal` | after SIGTERM the log contains `caught signal 15` | P2 |
| `test_sigint_reroutes_through_sigusr2` | after SIGINT the log contains `caught signal 12` and not `caught signal 2` (SIGINT is caught by plain `sigaction`, gdb-friendly, and re-raised as SIGUSR2) | P2 |
| `test_clean_shutdown_vnc_backend` | vnc instance; SIGTERM exits 0 | P2 |

**`test_shell_background.py`**

| Test | Asserts | lands |
|---|---|---|
| `test_background_default` [I] | vnc 640x480; RFB geometry (640, 480); whole frame becomes solid `(0x00,0x22,0x44)` | P2 |
| `test_background_from_config` [I] | `[shell] background-color=0xff336699` → solid `(0x33,0x66,0x99)`; log mentions `westonite.ini` | P2 |

**`test_cli.py`** — immediate-exit cases use `subprocess.run` directly
with `--log=<file> --no-config` and their own `XDG_RUNTIME_DIR`.

| Test | Asserts | lands |
|---|---|---|
| `test_version` | `--version` exits 0; first stdout token `westonite` | P2 |
| `test_help` | `--help` exits 0; contains `--backend` | P2 |
| `test_unknown_backend` | `--backend=bogus` → non-zero; log `unknown backend "bogus"` | P2 |
| `test_unhandled_option_is_fatal` | `--bogus-option` → non-zero; log `unhandled option: --bogus-option` | P2 |
| `test_option_for_an_unloaded_backend_is_fatal` | `--backend=headless --seat=seat1` → non-zero; log `unhandled option: --seat` | P2 |
| `test_missing_xdg_runtime_dir_refused` | without `XDG_RUNTIME_DIR`: non-zero; stderr mentions it | P2 |
| `test_socket_name` | `--socket=wibble-0` → that socket is the display | P2 |
| `test_two_instances_share_runtime_dir` | two instances in one runtime dir get two distinct `wayland-*` sockets | P2 |
| `test_config_found_in_xdg_config_home` [I] | `[core]` ini in `$XDG_CONFIG_HOME` → log `Using config file '…/westonite.ini'` | P2 |
| `test_config_explicit_path` | `--config=<custom-name.cfg>` used | P2 |
| `test_stock_weston_ini_is_ignored` [I] | a `weston.ini` in `$XDG_CONFIG_HOME` is never mentioned (P1 negative) | P2 |
| `test_home_config_fallback` | `XDG_CONFIG_HOME` unset, `HOME=<tmp>` → `~/.config/westonite.ini` found | P2 |
| `test_no_config_flag_ignores_existing_config` | `--no-config` with an ini present → no `Using config file` | P2 |
| `test_log_lines_are_timestamped` | after `Command line:`, lines match `^\[\d\d:\d\d:\d\d\.\d\d\d\] ` and continuation lines start with a space | P2 |
| `test_flight_recorder_is_on_by_default` | log `Flight recorder: enabled` | P2 |
| `test_empty_flight_rec_scopes_disables_the_recorder` | `--flight-rec-scopes=` → `Flight recorder: disabled` (absent ≠ empty) | P2 |
| `test_logger_scopes_redirect_the_log_file` | `--logger-scopes=drm-backend` → starts (fixture waits on the socket) and `Command line:` is absent | P2 |
| `test_logger_scopes_log_keeps_the_default` | `--logger-scopes=log` → `Command line:` present | P2 |
| `test_wait_for_debugger_stops_the_process` | `--wait-for-debugger`, `wait=False` → log `waiting for debugger, send SIGCONT to continue`; `/proc/<pid>/stat` state `T`; no socket yet; SIGCONT then `wait_ready()` | P2 |
| `test_protocol_dump_scope` | `--logger-scopes=proto --socket=proto-probe` + `wayland-info` → log matches `rq wl_display@1\.get_registry\(new id wl_registry@2\)` and `rq wl_registry@2\.bind\(\d+, "wl_\w+", \d+, new id \[unknown\]@\d+\)` | P2 |
| `test_protocol_dump_is_silent_without_a_subscriber` | default + `wayland-info` → `wl_display@1.get_registry` absent | P2 |
| `test_debug_protocol_advertises_the_global` | `--debug` → `weston_debug_v1` in `wayland-info` | P2 |
| `test_debug_protocol_is_off_by_default` | default → absent | P2 |
| `test_output_decorations_reach_the_backend` | `[core] output-decorations=true` on headless/pixman → exit non-zero, log mentions `decorations` | P2 |
| `test_use_pixman_config_key_conflicts_with_renderer` | `[core] use-pixman=true` + `renderer=gl` → non-zero; `Conflicting renderer specifications` | P2 |

**`test_children.py`** — stub clients are `#!/bin/sh` scripts that
`env > <marker>` then run a body.

| Test | Asserts | lands |
|---|---|---|
| `test_no_helper_clients_by_default` [I] | after `desktop-shell.so` loads, 1 s grace, no `launching` | P2 |
| `test_shell_client_setting_is_ignored` | `[shell] client=<stub>` spawns nothing | P2 |
| `test_autolaunch_config_spawns` | `[autolaunch] path=<stub>` → stub runs with `WAYLAND_DISPLAY` == the socket and `WESTON_CONFIG_FILE` == the ini path | P2 |
| `test_autolaunch_watch_exits_with_client` [I] | `watch=true`, stub `sleep 1` → compositor exits 0 by itself | P2 |
| `test_autolaunch_no_watch_survives_client_exit` | `watch=false`, stub `exit 0` → still running after 1 s | P2 |
| `test_autolaunch_child_crash_tolerated` | stub `kill -SEGV $$` → still running | P2 |
| `test_autolaunch_nonexecutable_is_fatal` | `path=/nonexistent-client` → log `autolaunch path (/nonexistent-client) is not executable`, non-zero | P2 |
| `test_positional_command_runs_and_watch_applies` | `westonite -- <stub> arg1` → stub sees `arg1`; compositor exits 0 when it exits | P2 |
| `test_clean_shutdown_with_live_child` | SIGTERM with a running autolaunched client exits 0 | P2 |

**`test_outputs.py`** — geometry via `wayland-info`.

| Test | Asserts | lands |
|---|---|---|
| `test_headless_geometry_from_cli` | `--width=800 --height=500` → `width: 800 px, height: 500 px` | P2 |
| `test_output_scale_and_transform_from_config` | `[output] name=headless mode=640x480 scale=2 transform=rotate-90` → `scale: 2`, `transform: 90` | P2 |
| `test_output_transform_from_cli` | `--transform=rotate-270` → `transform: 270` | P2 |
| `test_invalid_transform_is_fatal` | `--transform=bogus` → non-zero; `Invalid transform "bogus"` | P2 |
| `test_invalid_mode_falls_back_to_defaults` | `[output] name=headless mode=1024xNOPE` → `Invalid mode for output headless. Using defaults.`; output is 1024x640 | P2 |
| `test_no_outputs_advertises_no_wl_output` | `--no-outputs` → no `interface: 'wl_output'` | P2 |
| `test_vnc_output_mode_from_config` | `[output] name=vnc mode=800x500` → RFB geometry (800, 500) | P2 |
| `test_vnc_client_resize_repaints_background` | **skip-marked** (RPM-side `SetDesktopSize` crash) | P2 |
| `test_multi_backend_headless_plus_vnc` | `--backend=vnc --backends=headless,vnc` → both outputs advertised (poll), VNC capture works | P2 |

**`test_shell_windows.py`** — vnc + `wtest-client`. Colours RED
`ffcc0000`/BRIGHT_RED `ffff4444`, BLUE `ff0000cc`/BRIGHT_BLUE `ff4444ff`,
GREEN `ff00cc00`. `grab_drag()` presses, **waits until the client
reports the button**, then moves in steps and releases;
`move_window_to()` re-drags until the origin lands (each iteration is a
real grab; retries only absorb dropped motions).

| Test | Asserts | lands |
|---|---|---|
| `test_window_maps_inside_output` | `mapped: 200x150`; a 200x150 RED region fully inside 640x480 | P2 |
| `test_new_window_gets_keyboard_focus` | `focus: enter` and BRIGHT_RED region | P2 |
| `test_click_moves_activation_between_windows` | two windows; find the one drawn unfocused, click a pixel of it; `focus: enter`/`leave` swap; framebuffer swaps; a typed `a` (keysym 0x61) lands in it | P2 |
| `test_pointer_move_grab` | `--interactive`; `move_window_to(60,60)` with size unchanged | P2 |
| `test_pointer_resize_grab` | park at (40,40); right-drag from 5 px inside the bottom-right corner by (+80,+60), ≤ 3 attempts, until a `configure: WxH [ resizing` with W,H larger; then region grew and the last sized configure equals the on-screen size (no exact deltas — §2) | P2 |
| `test_focus_moves_to_survivor_when_focused_window_closes` | two focus-coloured windows; terminate the focused one; the survivor gets one more `focus: enter` and repaints focused | P2 |
| `test_background_clicks_are_swallowed` | click bare background (x=5 or 630, y=470); no `focus: leave` after 0.5 s; still drawn focused | P2 |
| `test_wm_capabilities_empty` | `wm-capabilities: [ ]` empty | P3, T8 |
| `test_fullscreen_request_ignored` | `--request-fullscreen` → maps 200x150, region 200x150, no configure contains `fullscreen` | P3, T5 |
| `test_maximize_request_ignored` | same for maximize | P3, T5 |
| `test_unresponsive_client_handled` | `pause()`; three clicks on it; origin unchanged; `resume()`; a click yields `pointer: button`; alive | P3, T7 |

**`test_xwayland.py`** — `--xwayland`; `wtest-xclient` with
`DISPLAY=<x_display>`. `titlebar_drag()` presses 10 px above the
content, holds 0.5 s (xwm handles frame clicks asynchronously), moves in
12 steps 30 ms apart, holds 0.3 s, releases.

| Test | Asserts | lands |
|---|---|---|
| `test_lazy_spawn_and_roundtrip` [I] | display advertised before `launching '/usr/bin/Xwayland'`; `xdpyinfo -display :N` prints `vendor string`; then the launching line | P2 |
| `test_x11_window_renders_and_position_syncs` | MAGENTA (`cc00cc`) region 200x150; last `position:` report converges on the region origin (recapture inside the poll) | P2 |
| `test_x11_window_drag_updates_x_position` | titlebar drag by (80,50); region moved and the last X position equals the new origin | P2 |
| `test_two_x11_clients` | magenta and orange (`cc6600`) both render | P2 |

**`test_backends_nested.py`** — `outputs_of(w)` = `name:` values from
`wayland-info` run as a client of that instance.

| Test | Asserts | lands |
|---|---|---|
| `test_wayland_backend_nests_in_another_westonite` | host headless `--socket=nest-host`; nested `backend="wayland"`, `--socket=nest-child`, same runtime dir, env `WAYLAND_DISPLAY=nest-host` → `Output 'wayland0' enabled`; `wayland0` advertised; a client of the nested one sees `wl_output` | P2 |
| `test_wayland_backend_output_count_and_size` | `--output-count=2 --width=800 --height=500` → `wayland0` and `wayland1`; `width: 800 px, height: 500 px` | P2 |
| `test_wayland_default_heads_number_from_zero_beside_named` | `[output] name=WL-1` + `--output-count=2` → `WL-1` and `wayland0`, no `wayland1` | P2 |
| `test_x11_default_heads_continue_after_named` | host `--xwayland`; nested x11 with `[output] name=X-1` + `--output-count=2` → `X-1` and `screen1`, no `screen0` | P2 |
| `test_x11_backend_under_our_own_xwayland` | nested x11 → `Output 'screen0' enabled` | P2 |
| `test_x11_backend_default_size_is_1024x600` | nested x11, no size flags → `width: 1024 px, height: 600 px` | P2 |
| `test_pipewire_backend_publishes_an_output` | start a `pipewire` daemon in its own runtime dir, wait for `pipewire-0`; `backend="pipewire"` → `Output 'pipewire' enabled`; daemon stopped in `finally` | P2 |
| `test_pipewire_backend_is_still_supported` | `backend="pipewire"`, `wait=False`, no daemon → log matches `Loading module|initializing pipewire backend|pipewire` and never `is not supported by westonite` | P4, F2 |

**`test_screenshooter.py`** — Super taps via `key_code`.

| Test | Asserts | lands |
|---|---|---|
| `test_super_r_toggles_wcap_recorder` | vnc; capture; Super+R → `<workdir>/capture.wcap` appears; move the pointer across 10 positions 50 ms apart; Super+R; poll until the file's `<IIII` header has magic `0x57434150`, width/height = RFB geometry, and more than 16 bytes | P2 |
| `test_super_s_is_inert` | vnc; Super+S; after 1 s no `launching` in the log and the compositor is alive | P4, F5 |
| `test_foreign_capture_client_is_denied` | headless; `run_client(["/usr/bin/weston-screenshooter"], check=False)` exits non-zero and the compositor survives (no authority listener exists, so every capture is denied) | P4, F5 |

The F5 recorder guard for `--no-outputs` has no black-box trigger
(Super+R needs a VNC peer; `--no-outputs` is a headless option); it is
covered by review only.

**`test_refusals.py`** — every case: start with the config or flag,
`wait=False`, `wait_for_log(pattern)`, `wait_exit() != 0`.

| Test | Asserts | lands |
|---|---|---|
| `test_rdp_backend_is_refused` | `--backend=rdp` → `rdp backend is not supported by westonite` | P4, F1 |
| `test_remote_output_section_is_refused` | `[remote-output] name=r mode=640x480@30 host=127.0.0.1 port=5000` → `\[remote-output\] is not supported by westonite` | P4, F2 |
| `test_pipewire_output_section_is_refused` | `[pipewire-output] name=p mode=640x480@30` → `\[pipewire-output\] is not supported by westonite` | P4, F2 |
| `test_modules_key_is_refused` | `[core] modules=something.so` → `not supported by westonite` | P4, F3 |
| `test_modules_cli_flag_is_refused` | `--modules=something.so` → `not supported by westonite` | P4, F3 |
| `test_touch_calibrator_is_refused` | `[libinput] touchscreen_calibrator=true` → `not supported by westonite` | P4, F4 |
| `test_idle_time_key_is_refused` | `[core] idle-time=60` → `not supported by westonite` | P4, F6 |
| `test_idle_time_flag_is_fatal` | `--idle-time=60` → `unhandled option: --idle-time` (direct subprocess) | P4, F6 |

**`test_backend_drm.py`** — `pytestmark = skipif WESTONITE_DRM_VM != "1"`.
vkms: one connected connector `Virtual-1`, preferred 1024x768@60.
Helpers: `output_block(w, name)` splits `wayland-info` on `^interface:`
and picks the `wl_output` stanza with that `name:`; `mode_of(w)` = the
single mode whose `flags:` contain `current`; `configured_devices(w)` =
all `libinput: configuring device "<name>"`; `libinput_block(w, device)`
= the setting lines C logs under that device, indented by exactly ten
spaces after the optional timestamp (weston's own continuations are
indented fifteen), in log order.

| Test | Asserts | lands |
|---|---|---|
| `test_drm_backend_enables_the_connected_head` | log `DRM: head 'Virtual-1' found, connector \d+ is connected` and `Output 'Virtual-1' enabled with head(s) Virtual-1`; `wayland-info` mentions it | P5 |
| `test_drm_output_defaults_to_the_preferred_mode` | current mode (1024, 768) | P5 |
| `test_drm_output_mode_from_config_is_a_modeline` | `[output] name=Virtual-1 mode=1280x720` → (1280, 720) | P5 |
| `test_drm_output_scale_applies` | `scale=2` → `scale: 2` | P5 |
| `test_drm_output_off_leaves_no_outputs_and_that_is_fatal` | `mode=off` on the only head → exit non-zero; log never contains `was supposed to be pruned` | P5 |
| `test_drm_device_can_be_selected` | `--drm-device=card0` → `using /dev/dri/card0` and the output enabled | P5 |
| `test_unknown_drm_device_is_a_startup_error` | `--drm-device=card99` → non-zero | P5 |
| `test_libinput_hook_runs_for_every_device` | no `[libinput]` → `QEMU Virtio Keyboard` and `QEMU Virtio Mouse` both configured, both blocks empty | P5 |
| `test_libinput_pointer_settings_apply` | `[libinput] left-handed=true middle-button-emulation=true accel-profile=flat accel-speed=0.5 natural-scroll=true scroll-method=button scroll-button=BTN_RIGHT` → mouse block exactly `middle-button-emulation=true`, `left-handed=true`, `accel-profile=flat`, `accel-speed=0.500`, `natural-scroll=true`, `scroll-method=button`, `scroll-button=BTN_RIGHT` | P5 |
| `test_libinput_unsupported_keys_are_skipped_per_device` | `[libinput] enable-tap=true left-handed=true` → mouse block `["left-handed=true"]`, keyboard block empty, `enable-tap` nowhere in the log | P5 |

---

## 13. Final acceptance checklist

- [ ] Both images build.
- [ ] `scripts/smoke-test.sh` passes in the build image.
- [ ] `scripts/e2e-test.sh /results`: 84 passed, 1 skipped (`test_backend_drm.py` collected as 10 skips if you include it; either way).
- [ ] `scripts/drm-vm-test.sh /results c`: sentinel `DRMVM-EXIT=0`, 10 passed.
- [ ] `scripts/rpm-build.sh /out` produces `westonite-14.0.1-1.el10.x86_64.rpm` (+debuginfo, debugsource).
- [ ] `scripts/rpm-install-test.sh /rpms /src /results` in a pristine `centos:stream10` container: `weston` not pulled in, session file valid, legacy smoke passes, `-m installed` 8 passed.
- [ ] GitHub Actions green on `main`, both jobs.
- [ ] `VENDOR.md` lists P0, P1, T1–T8, F1–F6.
- [ ] `docs/` contains `phase0-findings.md`, `desktop-shell-capabilities.md`, `frontend-capabilities.md`, `maintenance-layer-plan.md`, `e2e-test-plan.md`, `drm-testing.md`; `PLAN.md` is this document.
- [ ] `desktop-shell/shell.c` has the 10-entry `shell_desktop_api`, the 4-registration `shell_add_bindings`, reads only `background-color`; `grep -r wl_global_create desktop-shell/` finds nothing.
- [ ] `frontend/main.c` has no `load_rdp_backend`, `load_remoting`, `load_pipewire`, `load_modules`, `save_touch_device_calibration`, `idle-time` option; `weston-screenshooter.c` registers only `KEY_R`.

---

## Appendix A — exact file contents

Reproduced verbatim so they can be copied rather than re-derived.

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
# Phase 0); probe each one we include anyway so a repackaged libweston
# fails loudly at configure time instead of at compile time.  rdp is
# not probed: F1 removes its include.
foreach backend : [ 'drm', 'headless', 'pipewire', 'vnc', 'wayland', 'x11' ]
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

### A.7 `data/westonite.ini.example`

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
helper clients (no panel, no on-screen keyboard, no lock screen), no
animations, and a deliberately small frontend surface (see README.md).

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

### A.11 `scripts/rpm-install-test.sh`

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

### A.12 `.github/workflows/ci.yml`

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

  # The DRM backend needs a KMS device, which no container and no
  # GitHub-hosted runner offers.  This job boots a VM that carries its
  # own kernel (ours, from the same EL10 content set as everything else)
  # and loads vkms inside it -- so the only thing asked of the runner is
  # /dev/kvm, and even that is optional.  docs/drm-testing.md explains
  # the route and, importantly, what vkms does *not* prove.
  drm-vm:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build the CentOS Stream 10 + EPEL build image
        run: docker build -f containers/Containerfile.build -t westonite-build .

      - name: Build the DRM VM image
        run: docker build -f containers/Containerfile.drm-vm -t westonite-drm-vm .

      - name: DRM e2e inside the VM
        run: |
          mkdir -p test-results
          # The runner has /dev/kvm but it is group-owned; the container
          # runs as root and still needs write access to it.  Falling
          # back to TCG costs ~30s of boot, so this is best-effort.
          KVM=
          if [ -c /dev/kvm ]; then
            sudo chmod 0666 /dev/kvm || true
            KVM="--device /dev/kvm"
          fi
          docker run --rm $KVM -v "$PWD":/src -v "$PWD/test-results":/results \
            westonite-drm-vm /src/scripts/drm-vm-test.sh /results c

      - name: Upload DRM VM results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: test-results-drm-vm
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
# QEMU Extended Key Event: keycode-carrying key messages. Needed for
# modifier-key input -- the server's keysym path deliberately skips
# xkb modifier tracking for everything but Ctrl/Alt (RFC6143 7.5.4
# shift-state rules), so Super+X bindings only work via keycodes.
QEMU_EXT_KEY = -258


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
        encodings = [RAW_ENCODING, DESKTOP_SIZE, EXTENDED_DESKTOP_SIZE,
                     QEMU_EXT_KEY]
        self._send(struct.pack(">BxH", 2, len(encodings))
                   + b"".join(struct.pack(">i", e) for e in encodings))
        # screen layout, learned from ExtendedDesktopSize rects
        self.screens = [(0, 0, 0, self.width, self.height, 0)]
        # set by the server's one-time QEMU_EXT_KEY ack pseudo-rect
        self.qemu_keys = False

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
            update_without_pixels = False
            while not got_pixels and not resized and not update_without_pixels:
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
                        elif enc == QEMU_EXT_KEY:
                            # payload-less ack pseudo-rect: the server
                            # accepts keycode key events
                            self.qemu_keys = True
                        else:
                            raise RfbError(f"unexpected encoding {enc}")
                    if not got_pixels and not resized:
                        # a pseudo-rect-only update (e.g. the one-time
                        # QEMU ext-key ack) CONSUMES our update request
                        # server-side (neatvnc send_ext_support_frame)
                        # -- go around and request again
                        update_without_pixels = True
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

    def key_code(self, keysym, keycode, down):
        """QEMU Extended Key Event (message 255/0): key by keycode, so
        the server tracks modifier state (see QEMU_EXT_KEY above)."""
        self._send(struct.pack(">BBHII", 255, 0, 1 if down else 0,
                               keysym, keycode))

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
    # Reverse creation order: nested instances (a wayland backend inside
    # another westonite, an x11 backend on our own Xwayland) depend on
    # the one launched before them.  Tearing the host down first pulls
    # the child's display out from under it and it exits non-zero --
    # a teardown artefact, not a test failure.
    for w in reversed(instances):
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

### A.17 `tests/e2e/support/compositor.py`

```python
"""Manage westonite compositor instances for e2e tests.

Each instance gets its own XDG_RUNTIME_DIR, wayland socket name, config
file, and log file. Readiness and state checks poll the log with a
deadline -- never bare sleeps.
"""

import os
import pwd
import re
import signal
import socket
import subprocess
import time


# Multiplier applied to every deadline in this module.  The DRM VM
# (docs/drm-testing.md) falls back to TCG emulation on a host without
# /dev/kvm, where the FIRST compositor start takes ~22s -- cold page
# cache plus llvmpipe EGL init; later starts in the same run are 1.3-2s
# -- which blows this module's 10s deadlines outright.  Scaling them
# where the environment is known to be slow is the honest fix;
# loosening them for everyone would hide a real startup regression on
# the fast path.
TIMEOUT_SCALE = float(os.environ.get("WESTONITE_E2E_TIMEOUT_SCALE", "1"))


class Timeout(AssertionError):
    pass


def wait_until(predicate, deadline=10.0, interval=0.1, message="condition"):
    """Poll predicate() until truthy; raise Timeout after deadline seconds
    (times TIMEOUT_SCALE)."""
    deadline *= TIMEOUT_SCALE
    end = time.monotonic() + deadline
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(interval)
    raise Timeout(f"timed out after {deadline}s waiting for {message}")


def free_tcp_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Westonite:
    """One westonite process under test."""

    def __init__(self, tmp_path, extra_args=(), config=None, backend="headless",
                 width=640, height=480, env=None, socket_name=None,
                 runtime_dir=None, no_config_flag=True):
        self.workdir = tmp_path
        self.workdir.mkdir(parents=True, exist_ok=True)
        self.log_path = tmp_path / "westonite.log"
        self.runtime_dir = runtime_dir or tmp_path / "xdg"
        self.runtime_dir.mkdir(mode=0o700, exist_ok=True)
        self.config_home = tmp_path / "config"
        self.config_home.mkdir(exist_ok=True)
        self.backend = backend
        self.vnc_port = None
        self.socket_name = socket_name

        argv = [os.environ.get("WESTONITE_BIN", "westonite"),
                f"--backend={backend}", f"--log={self.log_path}"]
        if backend == "vnc":
            self.vnc_port = free_tcp_port()
            argv += ["--renderer=pixman",
                     f"--port={self.vnc_port}",
                     "--disable-transport-layer-security"]
        elif backend in ("wayland", "x11", "pipewire"):
            # nested/streaming backends default to the GL renderer and
            # there is no GPU in the test container; pixman is what the
            # vnc leg uses for the same reason
            argv += ["--renderer=pixman"]
        elif backend == "drm":
            # The DRM leg runs in a VM (docs/drm-testing.md) whose seat
            # may have no usable input at the moment weston starts, and
            # weston refuses to start without one unless told to.  A
            # property of the environment, not of any one test.
            argv += ["--continue-without-input"]
        if backend in ("vnc", "headless", "wayland", "x11") and width is not None:
            argv += [f"--width={width}", f"--height={height}"]
        if socket_name:
            argv += [f"--socket={socket_name}"]

        if config is None:
            # keep tests hermetic against stray ini files unless a test
            # explicitly wants the config-search behavior itself
            if no_config_flag:
                argv += ["--no-config"]
        else:
            config_file = self.config_home / "westonite.ini"
            config_file.write_text(config)
        argv += list(extra_args)

        self.env = dict(os.environ)
        self.env.update({
            "XDG_RUNTIME_DIR": str(self.runtime_dir),
            "XDG_CONFIG_HOME": str(self.config_home),
        })
        self.env.pop("WAYLAND_DISPLAY", None)
        for key, value in (env or {}).items():
            if value is None:
                self.env.pop(key, None)
            else:
                self.env[key] = value

        # cwd: files the compositor (or a child it spawns) creates with
        # relative paths -- capture.wcap, screenshots -- land in the
        # test's own tmp dir instead of wherever pytest was started.
        self.proc = subprocess.Popen(argv, env=self.env,
                                     cwd=str(self.workdir),
                                     stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL)

    # -- log helpers ----------------------------------------------------

    def log(self):
        try:
            return self.log_path.read_text(errors="replace")
        except FileNotFoundError:
            return ""

    def wait_for_log(self, pattern, deadline=10.0):
        regex = re.compile(pattern)
        return wait_until(lambda: regex.search(self.log()),
                          deadline=deadline,
                          message=f"log line matching {pattern!r}")

    def wait_ready(self):
        """Wait until the compositor is serving clients: the wayland
        socket exists (created after all backends load, so the VNC
        listener is already up too). Deliberately NO probe-connect to
        the VNC port: neatvnc is a single-client server and treats a
        probe as a client -- its teardown can race the real client's
        connect and kill it (seen as instant connection-closed on slow
        CI runners)."""
        wait_until(self._alive_with_socket, message="wayland socket to exist")
        return self

    def _alive_with_socket(self):
        assert self.proc.poll() is None, (
            f"westonite exited {self.proc.poll()} during startup\n"
            f"--- log ---\n{self.log()}")
        return self._sockets() != []

    def _sockets(self):
        if self.socket_name:
            path = self.runtime_dir / self.socket_name
            return [self.socket_name] if path.is_socket() else []
        return sorted(p.name for p in self.runtime_dir.iterdir()
                      if p.name.startswith("wayland-") and p.is_socket())

    @property
    def wayland_display(self):
        socks = self._sockets()
        assert socks, "no wayland socket"
        return socks[0]

    @property
    def x_display(self):
        """The :N display of this instance's Xwayland (requires
        --xwayland in extra_args); waits for it to be advertised."""
        match = self.wait_for_log(r"listening on display (:\d+)")
        return match.group(1)

    def vnc(self):
        """Connect the test's RFB client, authenticating as the user
        running the suite (see scripts/e2e-test.sh for the PAM setup).
        One reconnect on a stalled or server-closed handshake: busy CI
        runners can stall it, and the compositor may still be draining
        an earlier client's teardown."""
        from .vncclient import RfbError, VncClient
        assert self.vnc_port, "instance was not started with backend='vnc'"
        user = os.environ.get("WESTONITE_VNC_USER",
                              pwd.getpwuid(os.getuid()).pw_name)
        password = os.environ["WESTONITE_VNC_PASSWORD"]
        try:
            return VncClient("127.0.0.1", self.vnc_port, user, password)
        except (TimeoutError, OSError, RfbError):
            assert self.proc.poll() is None, (
                f"westonite died during VNC connect\n--- log ---\n{self.log()}")
            time.sleep(2.0)
            return VncClient("127.0.0.1", self.vnc_port, user, password)

    # -- client helpers -------------------------------------------------

    def client_env(self):
        env = dict(self.env)
        env["WAYLAND_DISPLAY"] = self.wayland_display
        return env

    def spawn(self, argv, **popen_kw):
        return subprocess.Popen(argv, env=self.client_env(), **popen_kw)

    def run_client(self, argv, timeout=15, check=True):
        """Run a Wayland client to completion; return CompletedProcess
        with captured text output. Asserts exit code 0 unless
        check=False (for clients that are EXPECTED to fail)."""
        timeout *= TIMEOUT_SCALE
        result = subprocess.run(argv, env=self.client_env(), timeout=timeout,
                                capture_output=True, text=True)
        assert not check or result.returncode == 0, (
            f"{argv} exited {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}")
        return result

    # -- shutdown -------------------------------------------------------

    def wait_exit(self, deadline=15.0):
        """Wait for the compositor to exit on its own; return exit code."""
        deadline *= TIMEOUT_SCALE
        try:
            return self.proc.wait(deadline)
        except subprocess.TimeoutExpired:
            raise Timeout(
                f"westonite still running after {deadline}s\n"
                f"--- log ---\n{self.log()}")

    def terminate(self, sig=signal.SIGTERM, deadline=10.0):
        """Signal the compositor and assert it exits cleanly (code 0)."""
        deadline *= TIMEOUT_SCALE
        if self.proc.poll() is None:
            self.proc.send_signal(sig)
        try:
            code = self.proc.wait(deadline)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            raise AssertionError(
                f"westonite did not exit within {deadline}s of {sig!r}\n"
                f"--- log ---\n{self.log()}")
        assert code == 0, (
            f"westonite exited {code}\n--- log ---\n{self.log()}")
        return code

    def kill(self):
        if self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait(5)
```

### A.18 `containers/Containerfile.drm-vm`

```dockerfile
# DRM test VM image (docs/drm-testing.md).
#
# Derived from the build image rather than folded into it: three CI jobs
# use westonite-build, and none of them wants an extra kernel, an
# initramfs and a second root filesystem in their layer cache.
#
#   docker build -f containers/Containerfile.build   -t westonite-build .
#   docker build -f containers/Containerfile.drm-vm  -t westonite-drm-vm .
#   docker run --rm --device /dev/kvm \
#       -v "$PWD":/src -v "$PWD/test-results":/results \
#       westonite-drm-vm /src/scripts/drm-vm-test.sh /results c
#
# --device /dev/kvm is an optimization, not a requirement: without it
# qemu falls back to TCG emulation, which boots this VM in about 35s.
#
# The guest kernel ships INSIDE this image on purpose. CentOS Stream
# 10's kernel-modules-core has vkms, so the only thing the harness needs
# from whatever machine runs it is (optionally) /dev/kvm -- no module
# loading on the host, no host kernel version dependency, nothing to
# install on the runner. See docs/drm-testing.md §1 for the on-the-runner
# approach this replaced.

ARG BUILD_IMAGE=westonite-build
FROM ${BUILD_IMAGE}

# qemu-kvm: the VMM (EL10 installs it as /usr/libexec/qemu-kvm; it is a
#   full qemu-system-x86_64 and honours -accel tcg).
# e2fsprogs: mkfs.ext4 -d builds the root image FROM A DIRECTORY without
#   mounting it, and debugfs reads the results disk back out the same
#   way -- together they are why this needs no privileges at all.
RUN dnf -y install qemu-kvm e2fsprogs \
    && dnf clean all

# The guest root filesystem, built with the same repos as this image, so
# the libweston the VM tests is the libweston everything else tests.
#   kernel-core        -- /usr/lib/modules/$kver/vmlinuz
#   kernel-modules-core-- vkms.ko  (the entire point)
#   weston-libs        -- libweston 14 + drm-backend.so + gl-renderer.so
#   seatd              -- EL10 libseat has only logind and seatd backends,
#                         and logind needs a session bus we have not got
#   systemd-udev       -- udevd + the input rules.  NOT for device nodes
#                         (devtmpfs makes those): libinput's udev backend
#                         enumerates with add_match_property(ID_INPUT,1),
#                         a property only udevd's input_id builtin sets,
#                         so without a udevd run libinput finds no devices
#                         at all and the [libinput] hook never fires.
#                         Pulls systemd in, but nothing makes it PID 1 --
#                         the guest still boots init=/init.sh.
#   python3-pytest     -- the e2e suite runs INSIDE the guest
#   wayland-utils      -- wayland-info, the suite's do-you-really-work probe
# install_weak_deps=False keeps this near 900 MB instead of several GB.
RUN mkdir -p /vm/rootfs \
    && dnf -y --installroot=/vm/rootfs --releasever=10 --enablerepo=crb \
        --setopt=install_weak_deps=False --setopt=tsflags=nodocs \
        install \
            kernel-core \
            kernel-modules-core \
            bash \
            coreutils \
            util-linux \
            procps-ng \
            kmod \
            e2fsprogs \
            weston-libs \
            seatd \
            systemd-udev \
            wayland-utils \
            python3-pytest \
    && rm -rf /vm/rootfs/var/cache/dnf/* /vm/rootfs/var/lib/dnf/history* \
    && dnf clean all

# Lift the kernel and the (generic, non-hostonly) initramfs that
# kernel-install generated in the installroot out to a fixed path, so
# the harness never has to know the kernel version.  Assert exactly one
# kernel: a silently-two-kernels installroot would pick one at random.
RUN set -eu; \
    kver=$(ls /vm/rootfs/lib/modules); \
    test "$(echo "$kver" | wc -l)" -eq 1; \
    cp "/vm/rootfs/lib/modules/$kver/vmlinuz" /vm/vmlinuz; \
    cp "/vm/rootfs/boot/initramfs-$kver.img" /vm/initramfs.img; \
    test -e "/vm/rootfs/lib/modules/$kver/kernel/drivers/gpu/drm/vkms/vkms.ko.xz"; \
    echo "$kver" > /vm/kver

COPY containers/drm-vm-init.sh /vm/rootfs/init.sh
RUN chmod +x /vm/rootfs/init.sh && mkdir -p /vm/rootfs/opt /vm/rootfs/results

WORKDIR /src
```

### A.19 `containers/drm-vm-init.sh`

```bash
#!/bin/bash
# Guest init for the DRM test VM (docs/drm-testing.md).
#
# This runs as PID 1 -- the kernel is booted with init=/init.sh, so
# there is no systemd, no udev and no login.  Everything the tests need
# is set up here by hand:
#
#   pseudo filesystems -> vkms -> udevd -> seatd -> pytest -> poweroff
#
# devtmpfs is what creates /dev/dri/cardN when vkms registers, so nothing
# in the DRM path itself needs udev.  udevd runs anyway, for exactly one
# reason: libinput's udev backend enumerates input devices with
# `udev_enumerate_add_match_property(e, "ID_INPUT", "1")`, and that
# property is set by udevd's input_id builtin.  With no udevd run, the
# virtio keyboard and tablet the harness attaches exist as
# /dev/input/eventN but are invisible to libinput -- and the
# `[libinput]` configure_device hook never fires.
#
# The last line the host looks for is the DRMVM-EXIT= sentinel: a
# kernel panic, an early exit or a hang all produce *no* sentinel, which
# the host harness treats as failure.  Never make it unconditional.

set -u

# PID 1 inherits no environment at all -- not even PATH -- so bash falls
# back to its compiled-in default, which does not include /usr/sbin.
# modprobe and seatd both live there.
export PATH=/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin

log() { echo "DRMVM: $*" > /dev/console; }

mount -t proc     proc     /proc
mount -t sysfs    sysfs    /sys
mount -t devtmpfs devtmpfs /dev
mkdir -p /dev/pts /dev/shm /run /tmp
mount -t devpts devpts /dev/pts
mount -t tmpfs  tmpfs  /dev/shm
mount -t tmpfs  tmpfs  /run
mount -t tmpfs  tmpfs  /tmp

exec > /dev/console 2>&1

log "guest up: $(uname -r)"

finish() {
    code=$1
    sync
    umount /results 2>/dev/null || true

    # The sentinel goes out through /dev/kmsg, not just /dev/console.
    # A userspace write to the console is buffered by the tty layer and
    # flushed asynchronously, so a kernel printk can be emitted into
    # the MIDDLE of it -- which is exactly what the sysrq power-off two
    # lines down did the first time this ran on a KVM-accelerated
    # runner, fast enough for the race to land:
    #
    #   DRMVM: DRMVM-EXI[    6.082274] sysrq: Power Off
    #   T=0
    #
    # and the host rightly reported no sentinel for a run whose seven
    # tests had all passed.  printk emits records whole, so a kmsg
    # write cannot be split that way; <2> (KERN_CRIT) makes sure it
    # reaches the console whatever the console loglevel is.  The
    # console copy stays as the human-readable one -- if it survives
    # intact the host just reads the same value twice.
    echo "<2>DRMVM-EXIT=$code" > /dev/kmsg 2>/dev/null || true
    log "DRMVM-EXIT=$code"

    # Let the tty drain before anything else prints, so the console log
    # stays readable for whoever has to debug a failure.
    sleep 1

    # No systemd, so no `poweroff`: ask the kernel directly.  `o` is
    # power-off; if the emulated machine ignores it the host's timeout
    # is the backstop.
    echo 1 > /proc/sys/kernel/sysrq 2>/dev/null || true
    echo o > /proc/sysrq-trigger 2>/dev/null || true
    sleep 15
    exit "$code"
}

# -- vkms ---------------------------------------------------------------
# The whole point of the VM: a KMS device we fully control.  Loading it
# here rather than baking it into the initramfs keeps the failure
# visible on the console instead of in a boot loop.
if ! modprobe vkms; then
    log "FATAL: modprobe vkms failed"
    finish 90
fi

for _ in $(seq 1 50); do
    [ -e /dev/dri/card0 ] && break
    sleep 0.1
done
if [ ! -e /dev/dri/card0 ]; then
    log "FATAL: vkms loaded but no /dev/dri/card0"
    ls -l /dev/dri 2>&1 || true
    finish 91
fi

log "drm devices: $(ls /dev/dri | tr '\n' ' ')"
for con in /sys/class/drm/card*-*; do
    [ -e "$con/status" ] || continue
    log "connector $(basename "$con") = $(cat "$con/status")"
done

# -- udev ---------------------------------------------------------------
# Only for the input-device properties (see the header).  --daemon
# forks and returns; `udevadm trigger` then replays an "add" for
# everything already in /sys, because the devices were created before
# udevd started and the kernel does not resend those uevents.
if ! /usr/lib/systemd/systemd-udevd --daemon > /run/udevd.log 2>&1; then
    log "FATAL: systemd-udevd failed to start"
    cat /run/udevd.log 2>&1 || true
    finish 94
fi
udevadm trigger --type=devices --action=add > /dev/null 2>&1 || true
# A bounded settle: a timeout here is not fatal on its own -- the check
# below is what decides, and it says something far more useful.
udevadm settle --timeout=30 > /dev/null 2>&1 || true

# Assert the one thing udevd was installed for.  Without it the libinput
# tests would not fail, they would pass vacuously (no devices, no hook
# calls, no log lines to contradict).
tagged=$(udevadm trigger --type=devices --subsystem-match=input --dry-run \
         --property-match=ID_INPUT=1 --verbose 2>/dev/null | wc -l)
log "udev: $tagged input devices tagged ID_INPUT"
if [ "$tagged" -eq 0 ]; then
    log "FATAL: no ID_INPUT devices -- libinput would see nothing"
    ls -l /dev/input 2>&1 || true
    finish 95
fi

# -- seat ---------------------------------------------------------------
# libseat on EL10 is built with the logind and seatd backends only.
# logind wants a session bus and a real login session; seatd is a
# 200-line daemon that just hands out device fds, which is exactly what
# a single-purpose VM wants.
seatd -g root > /run/seatd.log 2>&1 &
for _ in $(seq 1 50); do
    [ -S /run/seatd.sock ] && break
    sleep 0.1
done
if [ ! -S /run/seatd.sock ]; then
    log "FATAL: seatd did not create its socket"
    cat /run/seatd.log 2>&1 || true
    finish 92
fi
export LIBSEAT_BACKEND=seatd

# -- results disk -------------------------------------------------------
# /dev/vdb is a small ext4 image the host made and reads back afterwards
# with `debugfs -R dump` -- so the JUnit XML gets out without the host
# ever mounting anything (no loop device, no CAP_SYS_ADMIN).
# By label, not by /dev/vdN: there is no udev here to make
# /dev/disk/by-label, but libblkid scans devtmpfs directly, and a label
# does not care what order the host passed the -drive arguments in.
mkdir -p /results
if ! mount -L drmvm-results /results; then
    log "WARNING: could not mount the results disk -- console only"
fi

# -- environment --------------------------------------------------------
mkdir -p -m 0700 /run/xdg
export XDG_RUNTIME_DIR=/run/xdg
export HOME=/root
export LD_LIBRARY_PATH=/usr/local/lib64

# The host injects the payload here: binaries under /usr/local, the e2e
# tree under /opt/e2e, and the command to run in /opt/vm-command.sh.
if [ ! -x /opt/vm-command.sh ]; then
    log "FATAL: no /opt/vm-command.sh -- host injection failed"
    finish 93
fi

log "running payload"
/opt/vm-command.sh
finish $?
```

### A.20 `scripts/drm-vm-test.sh`

```bash
#!/bin/bash
# Run the DRM leg of the e2e suite inside a throwaway VM that has a real
# KMS device (vkms).  See docs/drm-testing.md for why this exists and
# what it does not prove.
#
# Runs inside the containers/Containerfile.drm-vm image, as root.
# Usage: drm-vm-test.sh [results-dir] [frontend]
#   frontend: "c" (the only value; kept so the results files carry a
#   leg name)
#
# The container needs no privileges.  --device /dev/kvm makes the VM
# roughly 10x faster but its absence only costs wall-clock: qemu falls
# back to TCG.
set -euo pipefail

RESULTS="${1:-/tmp}"
FRONTEND="${2:-c}"
mkdir -p "$RESULTS"

# Per-frontend, because CI runs both legs into the same results dir
# and a shared name would leave only the last one's console behind --
# exactly the artifact you need when the *first* leg is the one that
# failed.
CONSOLE="$RESULTS/drm-vm-console-$FRONTEND.log"
JUNIT="$RESULTS/e2e-drm-$FRONTEND.xml"

test -d /vm/rootfs || { echo "not the drm-vm image (no /vm/rootfs)" >&2; exit 2; }

# Decided up front because the guest payload needs it too: under TCG the
# compositor takes some 7s to start (llvmpipe EGL init dominates), which
# puts the suite's 10s deadlines right on the edge, so the guest scales
# every deadline rather than the suite loosening them for everyone.
# `if`, not `[ ... ] && ACCEL=kvm`: a bare test that comes out false is a
# failing command, and under `set -e` the absence of /dev/kvm would end
# the script instead of selecting TCG.
ACCEL=tcg
SCALE=4
if [ -c /dev/kvm ] && [ -w /dev/kvm ]; then
	ACCEL=kvm
	SCALE=1
fi

# -- build ---------------------------------------------------------------
cd /src
if [ -f build/build.ninja ]; then
	meson configure build -De2e-test-client=true >/dev/null
else
	meson setup build --prefix=/usr -De2e-test-client=true >/dev/null
fi
ninja -C build >/dev/null

# -- stage the guest root ------------------------------------------------
# A real copy, not hardlinks: the injections below would otherwise write
# through into the baked rootfs and poison every later run.
rm -rf /vm/run /vm/run.img /vm/results.img
cp -a /vm/rootfs /vm/run

# The C frontend and its shell plugin, installed exactly as the RPM
# would (prefix=/usr), so the guest exercises the shipped layout.
DESTDIR=/vm/run ninja -C build install >/dev/null

# The image bakes an init.sh, but take the working tree's copy: editing
# the guest init should not cost a 6-minute image rebuild, and in CI the
# two are the same file from the same commit anyway.
cp /src/containers/drm-vm-init.sh /vm/run/init.sh
chmod +x /vm/run/init.sh

mkdir -p /vm/run/opt/e2e
cp -a /src/tests/e2e/. /vm/run/opt/e2e/
mkdir -p /vm/run/usr/local/bin
cp build/tests/e2e/clients/wtest-client /vm/run/usr/local/bin/ 2>/dev/null || true
cp build/tests/e2e/clients/wtest-xclient /vm/run/usr/local/bin/ 2>/dev/null || true

BIN=/usr/bin/westonite

# The payload runs as root inside a single-purpose VM.  WESTONITE_DRM_VM
# is what unskips the DRM tests: they must NEVER run on a developer's
# machine just because it happens to have /dev/dri -- taking DRM master
# there would black out their display.
cat > /vm/run/opt/vm-command.sh <<EOF
#!/bin/bash
cd /opt/e2e
export WESTONITE_DRM_VM=1
export WESTONITE_E2E_TIMEOUT_SCALE=$SCALE
export WESTONITE_BIN=$BIN
export WTEST_CLIENT=/usr/local/bin/wtest-client
export WTEST_XCLIENT=/usr/local/bin/wtest-xclient
export WESTONITE_E2E_ARTIFACTS=/results/failures-drm-$FRONTEND
mkdir -p "\$WESTONITE_E2E_ARTIFACTS"
python3 -m pytest /opt/e2e/test_backend_drm.py -v -p no:cacheprovider \\
	--junit-xml=/results/junit.xml
EOF
chmod +x /vm/run/opt/vm-command.sh

# 2G: the staged guest root is ~1 GB (860 MB baked + the injected
# binaries and e2e tree).  Sparse, but CI runners are not generous with
# disk and the image is built alongside a 4 GB build image.
mkfs.ext4 -q -F -L drmvm-root -d /vm/run /vm/run.img 2G
mkfs.ext4 -q -F -L drmvm-results /vm/results.img 64M

# -- boot ----------------------------------------------------------------
echo "drm-vm: booting with -accel $ACCEL, timeout scale $SCALE (frontend: $FRONTEND)"

# -no-reboot so a panic ends the process instead of looping; the guest
# powers itself off with sysrq when it is done.  console=ttyS0 + -serial
# stdio is the only channel out.
#
# -vga none, not just -display none: q35 gives every guest an emulated
# Bochs VGA whether or not anything displays it, and once udevd runs its
# coldplug that modalias gets modprobed -- a SECOND DRM card, which
# weston then picked over vkms (its connector is called Virtual-2, which
# is how this was spotted: every mode assertion here is anchored to
# vkms).  Removing the adapter is better than teaching the harness to
# pass --drm-device: the tests should exercise weston's own card
# selection, and there is exactly one right answer when there is
# exactly one card.
#
# The two virtio input devices are what give the `[libinput]` tests
# something to configure: virtio-keyboard-pci and virtio-mouse-pci
# register real evdev nodes, which udevd tags and libinput then picks
# up.  A mouse as well as a keyboard because most of the section's keys
# are pointer-side (accel, scroll, left-handed) and a keyboard
# advertises none of those capabilities.  A *relative* pointer
# specifically -- virtio-tablet-pci is absolute, and libinput offers a
# different, smaller config surface for those.
set +e
timeout 900 /usr/libexec/qemu-kvm \
	-accel "$ACCEL" -M q35 -cpu max -m 3G -smp 2 -no-reboot \
	-display none -vga none -serial stdio \
	-kernel /vm/vmlinuz -initrd /vm/initramfs.img \
	-drive file=/vm/run.img,if=virtio,format=raw \
	-drive file=/vm/results.img,if=virtio,format=raw \
	-device virtio-keyboard-pci -device virtio-mouse-pci \
	-append "root=LABEL=drmvm-root rw console=ttyS0,115200 init=/init.sh selinux=0 panic=10 rd.emergency=poweroff" \
	> "$CONSOLE" 2>&1
QEMU_RC=$?
set -e

# -- results -------------------------------------------------------------
# debugfs reads the results filesystem without mounting it, so this stays
# unprivileged.  `|| true` because a guest that died early leaves nothing
# to dump and the console is then the whole story.
debugfs -R "dump /junit.xml $JUNIT" /vm/results.img >/dev/null 2>&1 || true
[ -s "$JUNIT" ] || rm -f "$JUNIT"
debugfs -R "rdump /failures-drm-$FRONTEND $RESULTS" /vm/results.img \
	>/dev/null 2>&1 || true

# The sentinel, not qemu's exit status, is the verdict: qemu exits 0 for
# a guest that panicked, hung until the timeout, or never ran the tests.
# No sentinel is a failure, always.
#
# The guest prints it twice -- once through /dev/kmsg, once to the
# console -- so take the FIRST match, which is the kmsg one: printk
# emits records whole, while the console copy can be spliced by a
# concurrent printk (see containers/drm-vm-init.sh).  And require at
# least one digit, so a console copy cut off right after the `=` cannot
# match and yield an empty verdict.
#
# `|| true`: with pipefail set, a grep that matches nothing -- or a
# SIGPIPE from head -- would take the whole script down here, exactly in
# the case whose error message below is the most useful thing this
# script can produce.
SENTINEL=$(grep -ao 'DRMVM-EXIT=[0-9][0-9]*' "$CONSOLE" | head -1 | cut -d= -f2 || true)
if [ -z "$SENTINEL" ]; then
	echo "drm-vm: FAILED -- no DRMVM-EXIT sentinel (qemu rc=$QEMU_RC)" >&2
	tail -60 "$CONSOLE" >&2
	exit 1
fi
if [ "$SENTINEL" != 0 ]; then
	echo "drm-vm: FAILED -- guest reported exit $SENTINEL" >&2
	tail -80 "$CONSOLE" >&2
fi
echo "drm-vm: guest exit $SENTINEL (console: $CONSOLE)"
exit "$SENTINEL"
```

---

## Appendix B — pitfalls index

| # | Where | Pitfall |
|---|---|---|
| 1 | Phase 0 | `meson` and `libinput-devel` are in CRB, not AppStream. Enable CRB or the image build fails. |
| 2 | Phase 0 | Public UBI 10 repos contain no weston and no `-devel` packages; unentitled builds must use CentOS Stream 10 + EPEL. |
| 3 | Phase 1 | Do not vendor `shared/config-parser.c`; its symbols come from `libweston-14.so`. |
| 4 | Phase 1 | `desktop-shell.so` needs an explicit `pixman-1` dependency; the RPM's `.pc` keeps pixman private. |
| 5 | Phase 1 | `shell.c` cannot build until T1/T2 remove its references to the never-imported protocol and text backend; gate the frontend half first. |
| 6 | Phase 1 | `weston_curtain_params` in 14.0.1 has a `get_label` callback and `surface_committed`; newer upstream differs. Write against the installed 14 headers. |
| 7 | Phase 1 | `/tmp/.X11-unix` missing → the RPM's `xwayland.so` segfaults in its bind-error path. Always `mkdir -p -m 1777`. |
| 8 | Phase 1 | Without `xorg-x11-server-Xwayland-devel` there is no `xwayland.pc`, so `HAVE_XWAYLAND_LISTENFD` stays off and the deprecated `-listen` path is used. |
| 9 | Phase 2 | neatvnc with TLS off offers only RSA-AES-256 / RSA-AES / Apple DH. No off-the-shelf Python client works; implement Apple DH. |
| 10 | Phase 2 | VNC always requires PAM auth via service `weston-remote-access` as the compositor's own user; `pam_unix` needs a real password and a non-root user. |
| 11 | Phase 2 | neatvnc is single-client: never probe-connect the port; a second connection kicks the first; serialise connections per instance. Apple-DH + PAM can take > 10 s on loaded runners: 30 s socket timeout and one retry. |
| 12 | Phase 2 | VNC `SetDesktopSize` segfaults the RPM stack. Skip-mark resize tests; do not work around it. |
| 13 | Phase 2 | Grabs: hold the button until the client reports it, or the shell never sees the move/resize request while the button is down. |
| 14 | Phase 2 | Resize grabs over VNC receive half-delta, every-other motions; trailing drag motions are occasionally dropped. Assert growth and consistency, converge by re-dragging. |
| 15 | Phase 2 | Spawn order ≠ map order for two clients; discover which one is unfocused from the framebuffer. |
| 16 | Phase 2 | `wtest-client` must use a `poll`-based loop, not `wl_display_dispatch`, or SIGUSR1 pausing is delayed by EINTR restarts. |
| 17 | Phase 2 | EL10 ships no `xeyes`/`xclock`/`xwininfo` and no X.org server: carry an xcb client; the x11 backend runs as a client of the Xwayland another westonite spawns; all nested backends need `--renderer=pixman`. xwm handles titlebar clicks asynchronously: press-and-hold before moving. |
| 18 | Phase 2 | Tear nested instances down in reverse creation order; killing the host first makes the child exit non-zero. |
| 19 | Phase 2 | Super+X bindings over VNC work only via QEMU extended key events (keycodes). The server's one-time ack pseudo-rect consumes an update request, so re-request after a pixel-less update. |
| 20 | Phase 2 | `push: branches: ['**']` plus `pull_request` runs every PR commit twice; restrict push to `main`. |
| 21 | Phase 4 | Any output-capture attempt with a VNC peer connected aborts the compositor (RPM-side assert). Test denial on headless only. |
| 22 | Phase 5 | The verdict is the `DRMVM-EXIT` sentinel written to `/dev/kmsg`; qemu's exit code means nothing, and a console-only sentinel can be spliced by a printk. |
| 23 | Phase 5 | `-vga none`: otherwise udevd's coldplug modprobes the Bochs VGA and weston picks that card over vkms. |
| 24 | Phase 5 | Without a udevd run libinput sees no devices and the `[libinput]` tests pass vacuously; assert the `ID_INPUT` count in the guest init. |
| 25 | Phase 5 | vkms registers on the faux bus since ~6.14: a driver-name lookup finds nothing. In the VM there is exactly one card. |
| 26 | Phase 5 | PID 1 inherits no `PATH`; set it before the first `modprobe`. Mount the results disk by label: no udev means no `/dev/disk/by-label`. |
| 27 | Phase 5 | Under TCG the first compositor start takes ~22 s; scale deadlines with `WESTONITE_E2E_TIMEOUT_SCALE=4` instead of loosening them globally. |
| 28 | Phase 5 | A DRM output advertises all 34 vkms modes; the active one is the mode whose `flags:` say `current`. |
| 29 | Phase 6 | Runtime install of `weston-libs` needs CRB enabled (`neatvnc` → `libturbojpeg`). `git archive` in the container needs `safe.directory`. The pristine container also needs `desktop-file-utils` and `util-linux` (`runuser`). |
| 30 | Always | Never let the `weston` package into a test container: it brings a second `desktop-shell.so` and the helper clients. |

## Appendix C — what could not be re-verified from the record

1. The upstream commit ids `51dfd1be` (P0) and `ee92a531` and the claim
   that these are the *only* 14.0.1→14.0.2 changes to the vendored set
   come from the record, not from a fresh diff. Verify with the `git
   log` in §2 item 8 before applying P0.
2. The exact hunks of P0 were not reconstructed; only the resulting
   guard in `simple_heads_output_sharing_resize()` was observed. Apply
   the upstream commit rather than hand-writing it.
3. Package versions in §2 are from July 2026 builds; newer content is
   expected and fine while weston stays 14.0.x. If EPEL rebases weston,
   follow the rebase procedure in `VENDOR.md`.
4. `test_foreign_capture_client_is_denied` runs
   `/usr/bin/weston-screenshooter`, which was present in the build image
   without `weston` or `weston-demo` installed. The record does not say
   which subpackage provides it; check with
   `rpm -qf /usr/bin/weston-screenshooter` in the image, and if it comes
   from a package this plan forbids, drop that one test rather than
   install the package.
5. The refusal helper in §8 and the tests in `test_refusals.py` are
   specified here, not copied from a working tree; their log wordings
   are the contract, the code shape is a sketch.
6. Line-count targets for the trims come from commit messages and may
   be off by a few lines.
7. `test_two_instances_share_runtime_dir` and
   `test_multi_backend_headless_plus_vnc` rely on timing tuned against
   2-core GitHub runners; on much slower hosts raise the deadlines
   uniformly via `WESTONITE_E2E_TIMEOUT_SCALE` rather than per test.
