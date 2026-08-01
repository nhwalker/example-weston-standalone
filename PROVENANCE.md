# Rust migration provenance (plan §8)

Maps every Rust module that translates C behavior to its source C file
at the upstream tag current at translation time (`14.0.1` + P0).  This
file replaces VENDOR.md's verbatim-import model for translated code;
VENDOR.md continues to govern the C sources until R3 removes them.
Rebase procedure on an EPEL weston bump: plan §8.

| Rust module | Source C @ tag | Notes |
|---|---|---|
| `crates/weston/src/compositor.rs` (bring-up, heads-changed, teardown) | `frontend/main.c` @ 14.0.1: `load_headless_backend` (3472), `simple_heads_changed` (2099), `simple_head_enable` (2007), signal/teardown paths (`main`, `out:`) | R0 **canned** mirror for the smoke exit criterion only; the R2a/R2b frontend port supersedes it and this row then points at the real modules |
| `crates/weston/src/listener.rs` | idiom port: `wl_listener`/`container_of` pattern (34 sites) | design, not line translation — see docs/callback-inventory.md |
| `crates/weston-sys/shim/shim.c` | static inlines from wayland-util/server headers + weston-log va_list pair | §3k |
| `crates/weston/src/{desktop,grab,curtain,layer,input_bindings,shell_init}.rs` | `desktop-shell/shell.c` @ 14.0.1+T9: desktop api vtable (1607-1619), grabs (117-925), curtains (1925-1988), layers, bindings (2117-2130), wet_shell_init (2171-2239) | wrapper half of the R1 shell port |
| `crates/westonite-shell/src/lib.rs` | `desktop-shell/shell.c` @ 14.0.1+T9: policy half (focus/activation 343-458 + 1622-1727, children 1457-1476, commit/map 1296-1397, output policy 1822-2045, teardown 2047-2115) | safe half; §10.3 bool** bug fixed by design |
| `crates/westonite-shell-plugin/src/lib.rs` | `wet_shell_init` entry contract | R1-only cdylib, deleted at R3 (D2) |
| `crates/weston/src/xwayland.rs` | `frontend/xwayland.c` @ 14.0.1 (whole file: `wet_load_xwayland` 234, `spawn_xserver` 105, `handle_display_fd` 58, `xserver_cleanup` 92, `wet_xwayland_destroy` 221) + `wet_client_launch`/`sigchld_handler` slices of `frontend/main.c` (437/376) | R2d; process side in `westonite-spawn` |
| `crates/weston/src/compositor.rs` (x11/wayland/pipewire loaders) | `frontend/main.c` @ 14.0.1: `load_x11_backend` (3924), `load_wayland_backend` (4044), `load_pipewire_backend` (3611) + their `*_output_configure` (3910/4030/3558) | R2c-nested |
| `crates/weston/src/screenshooter.rs` | `frontend/weston-screenshooter.c` @ 14.0.1 (whole file) + `wet_client_start` (main.c:512) and `wet_get_bindir_path` (main.c:1025) | R2e; created by the frontend, not the shell (§4 ownership fix, pulled forward from R3) |

## Migration log

- **R2c-color (2026-07-31)** — colour management: `[core]
  color-management` and the `[[output]]` colour keys, deferred since
  R2b because the DRM output path did not exist yet.
  - Ported: `wet_output_set_color_profile` / `_set_eotf_mode` /
    `_set_colorimetry_mode` / `_set_color_characteristics` /
    `allow_content_protection` (main.c:1365-1700), as one
    `apply_color` in the fence because the only thing that differs
    between backends is *which* of them get called.  Every symbol was
    already bound, so the fence's external surface did not grow.
  - **`allow_hdcp` was missing from the config model entirely.** C
    reads it on every backend (main.c:1873), so it was unreachable.
  - **Two keys in the model that C never reads**: `vrr-mode` (no
    reader anywhere in 14.0.1) and `max-cll` (no such field in
    `weston_color_characteristics`).  Removed, so `deny_unknown_fields`
    reports them with a span instead of accepting something libweston
    cannot apply.
  - C's grouping rule is the subtle part and is reproduced exactly: the
    eleven characteristic keys form five groups (primaries, white, maxL,
    minL, maxFALL) and each must be given **entirely or not at all**.
    Half a group is a config error, not a partial application.
  - One deliberate divergence: C returns from
    `wet_output_set_color_profile` *before reading* `icc_profile` when
    the manager is absent, so an icc-profile without
    `color-management=true` is silently ignored there.  We refuse it.
  - Test-harness finding: `ini_to_config` typed bools and ints but not
    floats, so the first float-valued keys in the model arrived quoted
    and failed deserialization.  Fixed in `support/compositor.py`.
  - Ordering finding, from a red test: the "these keys are drm-only"
    refusal runs before the name/range validation, so on a headless
    output it masks the error under test.  The validation cases request
    `--backend=drm` purely to get past that gate — no VM needed, since
    config validation precedes backend load.
- **Pipewire virtual-output plugin dropped (2026-07-31)** —
  `[[pipewire-output]]`, decided right after remoting and on the same
  grounds.  The distinction that makes this safe to drop is worth
  stating, because the names invite confusion: weston has **two
  independent** build options here, and they are different features —
  `backend-pipewire` ("PipeWire backend: screencasting via PipeWire"),
  which westonite **ports** and ships as `--backend=pipewire`, versus
  `pipewire` ("Virtual remote output with Pipewire on DRM backend"),
  the plugin, which it does not.  Neither is deprecated upstream (both
  default-on in 15.0.90), so this is a product decision like RDP and
  remoting.  `test_pipewire_backend_is_still_supported` asserts the
  other side of the line, so the drop cannot quietly widen into the
  backend.
- **Remoting dropped (2026-07-31)** — `[[remote-output]]`, the remoting
  plugin, is a product decision the same way RDP was: westonite does
  not ship GStreamer/RTP output streaming, and VNC is its remote path.
  - It is **not** deprecated upstream, and the question was worth
    asking: weston 15.0.90 still builds `remoting` by default and the
    man page still documents `remoting=`.  What *is* deprecated there
    is `screen-share` ("pending removal"), which westonite never
    shipped (`VENDOR.md`; it needed a libweston *private* API) — and
    whose upstream replacement is the mirroring functionality already
    ported at R2c-mirror.
  - The probe that preceded this (see the plan §7 note) had shown
    remoting *working* in the VM harness, so this is a decision not to
    ship a working feature, not an inability to port one.
  - **It was a silent no-op until now.** `[[remote-output]]` and
    `[[pipewire-output]]` both parsed into the config model and nothing
    ever read them: a user asking for a remote output got silence and
    no output.  That is the exact failure the fail-loud rule exists to
    prevent, so both are now startup errors, each with an e2e test,
    because a refusal without a test is how this recurs.  (Both name a
    product decision; pipewire-output briefly said "not yet ported"
    before it was dropped an hour later.)
- **R2c-drm (2026-07-31)** — the DRM backend, branch
  `claude/rust-migration-j84b2p`.  The last backend, and the only one
  that needed an environment built for it first (R2c-drm-harness,
  below).
  - **The layoutput machinery is what makes DRM different.** Every
    other backend takes the simple path: one head, one output,
    configure, enable.  DRM cannot, because several connectors may be
    driven as clones of one logical output, and because
    `weston_output_enable` failing is *normal* on real hardware (not
    enough CRTCs, incompatible modes).  So C gives it its own
    heads-changed handler plus `wet_layoutput` / `wet_output` /
    `wet_head_array` / `drm_try_attach` / `drm_try_enable` /
    `drm_process_layoutput(s)`; `crates/weston/src/layoutput.rs` is
    that pile.  Heads are *staged* on a group rather than enabled on
    sight, then attached and enabled together, detaching one head at a
    time and retrying until it comes up — losers pushed to the next
    round rather than dropped.
  - Two details kept exactly because they are load-bearing and look
    like slips: `wet_head_array` keeps its holes rather than
    compacting (the undo walk indexes from the end and relies on
    them), and C's cap is `n + 1 >= ARRAY_LENGTH`, one below the array
    size.
  - `require-outputs` (any/all/none) decides how hard a failed group
    is.  Note the consequence C leaves implicit: with the default
    `any`, an *empty* layoutput list also fails (`0 == 0`) — which is
    exactly how `mode=off` on the only head becomes a startup failure
    rather than a compositor running blind.  The e2e test asserts that.
  - `drm_backend_output_configure`: mode (preferred/current/modeline),
    max-bpc, scale, transform, gbm-format, content-type, seat — with
    C's two non-obvious defaults preserved: `mode=current` with no
    explicit `max-bpc` passes 0 rather than 16 (so reusing the current
    mode does not force a full modeset), and a single-head output
    inherits the *head's* transform rather than NORMAL (a panel
    mounted rotated reports it).
  - `clone-of` section resolution landed with it, retiring the
    fail-loud refusal carried since R2b.
  - Verified live: `tests/e2e/test_backend_drm.py`, **7 passed**
    against `westonite-rs` on vkms in the VM.  The modeline test is
    self-proving about *which* binary ran — the harness feeds the Rust
    leg TOML, which the C frontend cannot parse, so 1280x720 instead
    of the preferred 1024x768 could only have come from our port.  The
    `drm-vm` CI job is a gate from here on and runs the Rust leg
    first.
  - **Deliberately not ported, refused fail-loud**:
    `configure_input_device`.  C wires it into
    `weston_drm_backend_config.configure_device`, and it is the only
    consumer of `[libinput]` in the whole frontend — but weston-sys has
    no libinput bindings at all, so binding libinput's device-config
    API is a slice of its own.  `[libinput]` with DRM loaded is a
    startup error meanwhile.  The refusal is conditional on DRM
    because without it the section does nothing in C either.
- **R2c-drm-harness (2026-07-31)** — a test environment for the DRM
  backend, before any DRM porting.  Branch
  `claude/rust-migration-j84b2p`.  No Rust changed in this slice; it is
  entirely test infrastructure, and it follows the same discipline as
  R2c-nested: *prove the environment with the C oracle first, port
  second.*
  - **What was tried and dropped.** Four throwaway CI probes asked
    whether the GitHub runner could host a `vkms` device directly.  It
    can — `linux-modules-extra-$(uname -r)` provides the module — but
    the shape was wrong: `apt` on the runner, a module load into the
    *host* kernel, a device passed into a container and a seat daemon,
    each observable only one CI round at a time and none of it
    reproducible locally.  Dropped on user direction after round 4.
    The facts the probes bought are kept in `docs/drm-testing.md` §1 so
    nobody re-derives them; the probe step is deleted from CI.
  - Two of those facts were mistakes worth recording.  Round 3
    concluded "no vkms card" while its own log showed vkms loaded:
    since ~6.14 vkms registers on the **faux bus**, so
    `/sys/class/drm/cardN/device/driver` reads `faux_driver` and a
    driver-name match finds nothing.  Round 4 died on
    `LIBSEAT_BACKEND=builtin`, which is not a value EL10's libseat
    knows — it is compiled with the `logind` and `seatd` backends only.
  - **The route taken**: a VM carrying its own kernel inside our image
    (`containers/Containerfile.drm-vm`), so the only runner dependency
    is `/dev/kvm` — and its absence merely falls back to TCG (~35 s to
    boot, measured).  CentOS Stream 10's `kernel-modules-core` ships
    `vkms.ko` and `qemu-kvm` is in AppStream, so the guest is built
    from the same repos as everything else and the libweston under test
    in the VM is the libweston the rest of the suite tests.
  - No privileges anywhere: `mkfs.ext4 -d` builds the guest root *from
    a directory* without mounting it, and `debugfs -R dump` reads the
    JUnit XML back out of the results disk the same way.  No loop
    device, no `CAP_SYS_ADMIN`, no privileged container.
  - `containers/drm-vm-init.sh` is PID 1 — no systemd, no udev
    (devtmpfs is what creates `/dev/dri/cardN` when vkms registers).
    It loads vkms, starts `seatd`, runs the payload and prints a
    `DRMVM-EXIT=<n>` sentinel.  `scripts/drm-vm-test.sh` treats *the
    sentinel*, not qemu's exit status, as the verdict: qemu exits 0 for
    a guest that panicked, hung to the timeout, or never ran the tests,
    so a missing sentinel is always a failure.
  - The sentinel goes out through `/dev/kmsg`, and that is load-bearing.
    The first KVM-accelerated CI run reported "no sentinel" for a run
    whose seven tests had all passed: a userspace write to
    `/dev/console` is buffered by the tty layer and flushed
    asynchronously, so the sysrq power-off printk was emitted into the
    middle of it — `DRMVM: DRMVM-EXI[ 6.082274] sysrq: Power Off` /
    `T=0`.  Only KVM was fast enough for the race to land; every TCG run
    had been clean.  printk emits records whole, so a kmsg write cannot
    be split that way.
  - **The checkpoint that justified the whole route**: the C frontend
    on vkms reaches `Output 'Virtual-1' enabled with head(s) Virtual-1`
    — atomic modesetting, GBM modifiers, a 34-entry mode list
    (1024x768 preferred), the GL renderer via llvmpipe, and
    `desktop-shell.so` loaded.  Richer than expected; pixman is not
    needed here.
  - Bring-up findings: PID 1 inherits *no* environment, so `PATH` must
    be set before the first `modprobe`; `virtio_blk` and `ext4` are
    modules in the EL10 kernel, so the distro initramfs is required
    (the generic one `kernel-install` generates in the installroot
    works, with `init=` honoured across switch-root); and the results
    disk is mounted by **label**, since with no udev there is no
    `/dev/disk/by-label`.
  - Two findings came from red tests rather than from reading.  A DRM
    output advertises **every** connector mode over `wl_output` (34 for
    vkms) where headless and vnc advertise one, so a test reading "the
    first `width:` line" reads the *preferred* mode, not the active one
    — the mode in use is the one whose `flags:` say `current`.  And
    under TCG the *first* compositor start in the VM takes ~22 s (cold
    page cache plus llvmpipe EGL init; later starts in the same run are
    1.3–2 s), which blows the suite's 10 s deadline; the fix is
    `WESTONITE_E2E_TIMEOUT_SCALE`, set to 4 by the harness when it falls
    back to TCG, rather than loosening the deadlines for everyone and
    hiding real startup regressions on the fast path.
  - `tests/e2e/test_backend_drm.py` (7 tests) is gated on
    `WESTONITE_DRM_VM=1` and **not** on the presence of `/dev/dri`: a
    developer's workstation has one, and taking DRM master on it would
    black out their display.
  - Explicitly *not* proven: vkms is a virtual device with no real
    modes, timings, driver quirks or physical vblank.  A one-time
    real-hardware validation remains an R3 gate (`docs/drm-testing.md`
    §3).
- **R2c-nested (2026-07-29)** — the x11, wayland and pipewire backends,
  branch `claude/rust-migration-j84b2p`.
  - **The premise changed before the code did.** These three were
    slated as "inspection-verified, zero container reachability".  That
    was wrong: probing the environment first (with the *C* frontend, so
    the environment was proven before anything was ported) showed all
    three run in the existing container with no VM and no GPU —
    wayland nests inside a headless westonite, x11 runs as a client of
    the Xwayland *we* spawn, pipewire needs only its daemon.  So they
    are ported the same way every other slice was: with e2e tests that
    actually run.
  - Two environment findings worth keeping: EL10 ships **no X.org
    server at all** (only Xwayland — so there is no `Xvfb` route and
    the compositor under test supplies the X server for the compositor
    under test), and all three backends default to the GL renderer, so
    the harness passes `--renderer=pixman` exactly as the VNC leg does.
  - Fence: `load_x11` / `load_wayland` / `load_pipewire` mirroring the
    C loaders, plus `create_windowed_heads` for the shared
    section-name-then-default create_head loop ('X'→`screenN`,
    'WL'→`waylandN`).  `HeadSetup::Windowed` now carries *which*
    windowed plugin API to drive — it previously hardcoded the
    headless one, which would have silently misconfigured x11/wayland
    outputs.  New `HeadSetup::Pipewire` for the set_gbm_format +
    output_set_size configure.  `OutputPolicy::decide_sized` supplies
    the per-backend defaults that C keeps in each configure's
    `wet_output_config defaults` (x11 1024x600 is the one that differs
    visibly, and has its own test).
  - Frontend: the per-flag fail-loud table was restructured from
    "headless-only / vnc-only / nobody" into one row per flag with the
    set of loaded backends that consumes it, transcribed from the C
    `weston_option` tables — `--scale` and `--use-pixman` are shared by
    three backends now, so the old shape would have rejected valid
    invocations.
  - **RDP dropped as a product decision** (owner's call, 2026-07-29):
    not a porting gap.  `--backend=rdp` fails with a message that says
    westonite does not ship it and points at VNC; its CLI flags stay in
    the unconsumed table.  The R2c-mirror decision to defer RDP turned
    out to be moot in the best way.
  - Harness: the `westonite` fixture tears instances down in **reverse**
    creation order — a nested child exits non-zero when its host (and
    with it the parent display or X server) is killed first, which is a
    teardown artefact, not a failure.  `[shell] cursor-theme` /
    `cursor-size` added (C reads them for the nested wayland cursor).
    `pipewire`/`pipewire-utils` added to the build image.
  - CI: a throwaway `drm-vkms-probe` job (`continue-on-error`) reports
    whether a GitHub-hosted runner can `modprobe vkms`, whether
    `/dev/dri` appears and whether `/dev/kvm` exists — the cheap
    experiment that picks the DRM route (vkms on the runner vs. a
    VM/virtme).  Delete it once DRM lands.
  - Validation: fmt/clippy/unit/fence green; all 8 smoke legs; full
    e2e **Rust 69 passed / 1 skipped**, **C oracle 58 passed / 12
    skipped**; every new test green against BOTH frontends (the
    rdp-refusal one is Rust-only policy, so it is `toml_only`).

- **Deferred-drain fix (2026-07-28)** — the dispatch-core defect found
  at the R2d review round and measured at R2e, branch
  `claude/rust-migration-j84b2p`.  Not a port slice: it fixes our own
  R0-era wrapper bug, taken before R3 while the C frontend is still
  around to diff against.
  - Root cause: `Compositor::run` wrapped `wl_display_run` in
    `with_depth`, holding the dispatch counter at ≥1 for the whole
    session, so A4's drain-at-the-edge could only fire after the loop
    exited.  Deferred-tier events were delivered in one batch at
    shutdown and `defer_drop` boxes were never freed until then.
  - **Proven as a user-visible parity bug before fixing.** New e2e
    test `test_focus_moves_to_survivor_when_focused_window_closes`
    (two wtest-clients with distinct focus colors; kill the focused
    one, assert the survivor takes focus on both its protocol output
    and in the framebuffer).  Against the C oracle: PASS.  Against
    the unfixed Rust frontend: FAIL (10.5s timeout).  After the fix:
    PASS, 3/3, in 0.59s.  C `focus_state_surface_destroy` is the
    behavior; ours rides `TrackedSurfaceGone`, a deferred event.
  - Fix: leave the loop call unwrapped (it *is* the loop — wrapping
    it defeats the rule it appears to follow) and wrap the two
    bare-bodied event-loop signal callbacks instead, so one handler
    means one drain.  `on_sigchld` in particular would otherwise
    drain between two `waitpid` iterations via its own inner
    outbound calls.  Audit of every C→Rust entry point recorded in
    the plan §3e/A4 note: everything else already wraps through
    `Listener::trampoline` / `with_ctx` / `guard_ctx` / the
    binding/xwayland/screenshooter wrappers, and the label/log/no-op
    vtable entries must stay unwrapped (draining from inside the log
    sink would run app handlers on a log call).
  - Review round: (1) the first cut of the `on_sigchld` wrap took
    `Ctx::current()` before the reap and returned early without it,
    which silently turned "no Ctx" into "never call waitpid" —
    zombie children for a wrapper-state problem.  The reap now runs
    unconditionally and only the bookkeeping and the depth wrap take
    the Ctx (`debug_assert` for the teardown-ordering bug it would
    signal, as elsewhere in §3j).  (2) `on_term_signal`'s comment
    claimed a Ctx-less late signal became "a no-op instead of an
    unguarded FFI call" while the code still terminated the display —
    the code is right (dropping a SIGTERM is worse than skipping an
    empty drain); the comment was corrected.  (3) The audit missed
    two stale/incomplete spots: `xwayland::handle_child_exit` still
    justified its own wrap by "run() holds the loop at +1", and the
    `run()` audit described the unwrapped entry points as touching no
    `Ctx` state — `desktop::tramp_get_position` reads wrapper view
    state, and is correct to stay unwrapped for a different reason
    (no outbound call, so nothing to drain).  Both rewritten.
    (4) `rust-stress-test.sh` now checks `WESTONITE_BIN` resolves
    before launching valgrind, so a typo fails immediately instead of
    at the 20s socket timeout.  All four verified and kept; (1) is a
    regression the wrap introduced (the pre-existing code reaped
    unconditionally and only skipped the *bookkeeping* without a Ctx),
    though unreachable today — `Drop` drains `signal_sources` before
    `Ctx::teardown`, checked.
  - Re-ran the entry-point audit body-scoped (walk each `extern "C"`
    fn to its closing brace) rather than with a fixed-size window,
    since the window version is what missed `tramp_get_position`.
    Two more unwrapped entries turned up, both correct as-is and now
    named in the `run()` comment: the empty grab vtable entries
    `touch_down`/`touch_frame` (no-ops, just not spelled `noop_*`),
    and `desktop::client_surfaces`' nested `collect` — not an entry
    point on C's schedule at all, but an iteration callback we hand
    to `weston_desktop_client_for_each_surface` from inside an
    already-wrapped trampoline.
  - De-risking argument, and why this was safe to take now: the
    **hybrid** build has dispatched our trampolines from C's own
    `wl_display_run` since R1 — base depth zero — so drain- and
    free-at-the-edge (§3f) have been under destroy-storm stress all
    along.  The fix makes the pure-Rust frontend match the hybrid.
    `rust-stress-test.sh` gained `WESTONITE_BIN` so the storms run
    against both: hybrid and westonite-rs, valgrind-clean, 41 clients
    mapped each.
  - Validation: fmt/clippy/unit/fence green; all 8 smoke legs
    (valgrind); both stress targets; full e2e **Rust 63 passed / 1
    skipped**, **C oracle 53 passed / 11 skipped**.

- **R2e (2026-07-27)** — screenshooter/wcap recorder (plan §7 R2e),
  branch `claude/rust-migration-j84b2p`:
  - Fence (`crates/weston/src/screenshooter.rs`, all of C
    `frontend/weston-screenshooter.c`): Super+S key binding spawns
    `weston-screenshooter` (BINDIR resolution through
    `weston_module_path_from_env` + /usr/bin, C wet_get_bindir_path;
    wayland socketpair + WAYLAND_SOCKET via westonite-spawn, C
    wet_client_start), gated by the in-flight client slot; one
    oneshot client-destroy Listener, embedded in the state and
    reattached per spawn exactly as C reuses its embedded listener,
    resets the slot (NOT a box per press retired through defer_drop,
    which was a per-press leak while the drain gap lasted — fixed
    2026-07-28, see the entry above); Super+R
    toggles `weston_recorder_start/stop` on the keyboard-focus output
    with C's first-output fallback; the capture-authority listener
    authorizes exactly the spawned client, attached by direct
    wl_signal_add on `output_capture.ask_auth` because C's helper
    overwrites the notify our Listener trampoline needs; teardown
    from Compositor::drop before weston_compositor_destroy (C
    screenshooter_destroy runs inside the destroy emission).
    Ownership: created by build() right after the shell attaches —
    the §4 "frontend owns screenshooter_create" fix, pulled forward
    from R3 (the C tree still has shell.c:2232 call it; behavior
    identical).
  - **Found live: any weston-output-capture attempt aborts the
    compositor once a VNC peer is connected** —
    `assert(wl_list_empty(&ci->pending_capture_list))`,
    output-capture.c:193: vnc_output_repaint reaches the renderer
    only through neatvnc's aml dispatch, so queued capture tasks can
    outlive the repaint cycle.  Reproduced with the C frontend for an
    authorized Super+S shot AND a denied foreign client; headless
    denies cleanly (the stock client then dies on its own
    screenshot.c:101 zero-width assert).  RPM-side, out of scope
    (our-code-only); e2e exercises the spawn path with the binary
    diverted via WESTON_MODULE_MAP and the deny path on headless —
    see docs/e2e-test-plan.md §6/§2.7.
  - Documented divergences: (a) C's recorder fallback does
    `container_of(output_list.next)` with no empty check — a wild
    pointer under --no-outputs; ours logs "no output to record" and
    no-ops.  (b) a failed spawn is reported in the parent (std's
    status pipe) and creates no wl_client, where C forks first and
    leaves a doomed client whose death resets the slot — both settle
    in the same state.  Corollary: C tracks children on a *list*, we
    keep one pid slot per subsystem, so in the narrow case where
    `wl_client_create` fails and a second Super+S lands before the
    first child's SIGCHLD, C logs both exits and we log only the
    later one.  (c) B1/B2 run sync-tier (planned "deferred" pre-R0;
    the handlers touch only wrapper state — A3 proofs in the
    inventory).
  - Review round (ae2f18a, verified claim by claim — all six changes
    kept): (1) the **doc-comment splice** in log.rs was a real
    mistake of mine (my Edit anchor cut `log_line`'s doc comment in
    half, orphaning its second line onto the new helper). (2) the
    **client-destroy listener reuse** is right and more C-faithful —
    C reattaches the single listener embedded in `struct
    screenshooter` on every spawn, and my box-per-press +
    `defer_drop` would have accumulated one box per Super+S because
    pending-drop only drains at depth zero (the R2d gap). Verified
    live in a **debug** build (so `mark_attached`'s `debug_assert!(!
    attached, "double attach")` was armed) with the client diverted
    to `/bin/true`, which makes `wl_client_create` succeed and the
    client die at once: 5 spawn→connect→die→reattach cycles gave 5
    launches, 5 exit lines, no double-attach, and valgrind 0 errors
    / 0 definitely-lost. (3) setting pid/exe **before**
    `wl_client_create` matches C, where `wet_client_launch` lists the
    process before `wet_client_start` reaches client creation. (4)
    byte-preserving `bindir_path` (argv[0] must not go through
    `to_string_lossy`) — note the C helper returns 0 rather than a
    truncated length when the mapping exceeds the buffer
    (compositor.c:10556), so the added `buf.get(..len)` is defensive,
    not a panic fix. (5) copying the exe path out before
    `log_child_exit` upholds the no-borrow-across-FFI rule. (6) the
    `re.escape` and `qemu_keys = False` test fixes are correct.
  - Chased the reviewer's `defer_drop` rationale to its root and
    **measured** it: instrumenting `defer_drop` showed the
    pending-drop list growing monotonically (len=1,2,3 at depth=2)
    across one move-grab e2e test, so the R2d deferred-drain gap is
    a lifetime-long memory leak as well as an ordering delay — plan
    §3e/A4 now records that second dimension. (A surface-destroy
    probe did *not* reproduce a retirement, so the blast radius
    beyond the grab path is unmeasured and left unclaimed.)
  - e2e infrastructure: `vncclient.py` gained the QEMU Extended Key
    Event pseudo-encoding (-258) + `key_code()` (qnum keycodes) —
    the server's keysym path never tracks the Super modifier, so
    Super+X bindings are reachable only by keycode; the capture loop
    now re-requests after a pseudo-rect-only update (neatvnc's
    one-time ext-ack consumes an update request); the compositor runs
    with cwd in the test tmp dir.  New `test_screenshooter.py` (3
    tests, §2.7), green against BOTH frontends.
  - Validation: fmt/clippy/unit/fence green; all 8 smoke legs pass
    (legs 5-8 exercise screenshooter create + authority teardown on
    every shell attach); full e2e vs the Rust frontend **62 passed,
    1 skipped**; full C-oracle suite re-run green (one resize-grab
    timing flake, 3/3 green in isolation); targeted valgrind probe of
    the recorder cycle + diverted spawns + slot reuse + TERM
    teardown: 0 errors, 0 definite leaks.

- **R2d (2026-07-27)** — xwayland (plan §7 R2d), branch
  `claude/rust-migration-j84b2p`:
  - Fence (`crates/weston/src/xwayland.rs`, all of C
    `frontend/xwayland.c`): module load + `weston_xwayland_v3` plugin
    API resolution + `api->listen` (C wet_load_xwayland, every failure
    fatal at startup); `spawn_xserver` — the module calls back on the
    first X connection, we build the wayland/WM socketpairs and the
    `-displayfd` pipe, dup the module's two listen fds (borrowed in C,
    passed by raw number; the dup is the owned equivalent), fork
    `[xwayland] path` through `westonite-spawn`, and hand the parent
    wayland end to `wl_client_create`; `handle_display_fd` — the
    newline-terminated displayfd write hands `wm_fd` to
    `api->xserver_loaded` (WM start); the SIGCHLD handler's pid match
    logs C's exit lines and relays `api->xserver_exited` (lazy respawn
    on the next connection); teardown before
    `weston_compositor_destroy` (C wet_xwayland_destroy order).
    `install_signal_sources` now also blocks SIGUSR1 process-wide
    before backend threads spawn (C main.c:4570).
  - `westonite-spawn` additions: `Command::new/arg/arg_fd` (fd passed
    by argv number, the C fdstr contract), `socketpair_stream`,
    `pipe_cloexec`; `weston` now depends on `westonite-spawn` (both
    are fence crates in rust-fence-check.sh — the dep-graph rule is
    about safe crates and is unchanged).
  - `-listenfd` vs `-listen` (C HAVE_XWAYLAND_LISTENFD): weston-sys
    build.rs runs the same pkg-config probe as this repo's C build
    (meson.build:111-114; upstream weston's xwayland/meson.build
    carries the same check) — `xwayland.pc` `have_listenfd`, verified
    `true` in the build container — and exports
    `weston_sys::XWAYLAND_LISTEN_ARG`.
  - Frontend: `--xwayland`/`[core] xwayland` + `[xwayland] path` were
    already in westonite-config (R2a); the R2d gate in
    `reject_unported` is removed and the settings now feed
    `CompositorBuilder::with_xwayland`.
  - Documented divergences: (a) displayfd EOF — C returns 1 and
    re-arms a level-triggered pipe that stays readable forever
    (spinning until the process watcher fires); we tear the watch down
    on EOF, which can never complete the line. (b) a dead server's
    parked `wm_fd` — and any readiness watch it left installed, whose
    EOF event the module can outrun by respawning inside the same
    dispatch batch — are closed on respawn, where C overwrites the
    number and leaks the old fd. (c) C's child-side `seteuid(getuid())`
    remains deferred with the R2a westonite-spawn audit note (euid ==
    uid in every supported deployment; the helper-client path revisits
    it).
  - Validation: fmt/clippy/unit/fence-check green;
    `rust-smoke.sh` gains leg 8 — the full xwayland lifecycle (lazy
    spawn → xdpyinfo → WM up → SIGTERM teardown) under valgrind, 0
    errors / 0 definite leaks; live probes: lazy spawn (no `launching`
    before the first X client), kill -9 respawn (`died on signal 9` +
    relaunch on reconnect, launch count 2), clean exit 0 both runs.
    `rust-e2e-test.sh` drops the `test_xwayland.py` ignore and sets
    `WTEST_XCLIENT`: the Rust frontend now runs the ENTIRE e2e suite —
    **59 passed, 1 skipped** (the known EPEL neatvnc resize skip),
    xwm position-sync and titlebar-drag pixel tests included; C oracle
    `test_xwayland.py` re-run green (4 passed).
  - Review round (e3afb7d, verified claim by claim): (1) the stale
    readiness watch on respawn — REAL: a server that dies with its
    displayfd EOF still queued in the same epoll batch can be replaced
    (SIGCHLD → xserver_exited → fresh X connection, all in one
    dispatch) before the EOF fires; the un-removed old source then
    dispatches against a closed fd and its cleanup would tear down the
    NEW server's watch, so `close_display_watch` on respawn (and
    libwayland's remove-during-dispatch fd=-1 skip) is the right fix.
    (2) byte-preserving the display name and (3) the compile-time pin
    of the positional `_bindgen_ty_2` constant — both correct.  (4)
    the `with_depth` around the SIGCHLD `xserver_exited` relay is kept
    as the A4 convention, but its stated hazard was not live: `run()`
    wraps `wl_display_run` at +1, so nothing inside the loop can
    return the counter to zero — probing that claim empirically
    exposed the pre-existing deferred-drain gap now tracked in plan
    §3e/A4 (mid-run deferred events drain only at loop exit; own
    slice).  (5) the meson citation was redirected to upstream's
    `xwayland/meson.build`, a file not present in this repo — restored
    to this repo's `meson.build:111-114`, whose `default_value:
    'false'` semantics are exactly what build.rs mirrors.

- **R2c-mirror (2026-07-26)** — the mirror-of machinery (plan §7 R2c,
  second slice), branch `claude/rust-migration-j84b2p`:
  - Fence: the three C mirror pieces around simple_head_enable —
    remote-head deferral (a vnc head whose section carries mirror-of=
    is skipped by heads-changed and waits for its source),
    `mirror_on_output_created` (L6: a native output's creation enables
    the deferred remote head, nested inside the source's
    output_created emission exactly as C's wet_output_handle_create),
    and `mirror_on_output_resized` (L2: reposition the remote over the
    resized source and recompute its modeline; one compositor-wide
    listener replaces C's per-tracker one — the rule table is the
    discriminator).  enable_head gained the mirror parameter:
    pre-enable sets output->mirror_of + position-over-source (C
    wet_output_overlap_pre_enable, replacing lazy_align), the VNC
    configure forces resizeable off with C's log line, post-enable
    applies the source-derived modeline (C _post_enable).  L6 guards
    an already-enabled head where C would double-create.  L3's
    source-destroy → remote-disable exists only in C's DRM layoutput
    path, so simple backends need none (verified in-source).
  - **Found live: C aborts on a headless mirror source.**  The
    headless backend points current_mode at its one static mode and
    never calls weston_output_copy_native_mode (drm/x11/pipewire do,
    and vnc/rdp get it via weston_output_set_single_mode), so a
    headless source leaves native_mode_copy zeroed and C dies
    on `assert(output->native_mode_copy.width)` (main.c:2543 —
    reproduced with the C oracle on the exact e2e config).  Deliberate
    fail-loud divergence: apply_mirror_modeline falls back to the
    source's current_mode (a static output's current mode IS its
    native mode), and flags init_failed instead of reaching
    libweston's compositing-area assert when no mode is usable at all.
    This is what makes the whole machinery container-verifiable.
  - RDP: deliberately deferred out of this slice.  Its configure path
    (head_get_monitor negotiation, client desktop-scale) has zero
    container reachability — porting it now would be sight-unseen
    code.  Its CLI drift WAS fixed now (an R-G accuracy issue): the
    invented --no-clients-resize became C's --no-resizeable, and
    --rdp4-key/--env-socket/--no-remotefx-codec were added (all in the
    unconsumed-flags table until the loader lands); the [rdp] model
    key follows the C config field name (resizeable).
  - Frontend: mirror-of unblocked with validation (requires the vnc
    backend loaded; remote sections only; self-mirror rejected);
    policy rules carry mirror_of; helpers has_mirror_of /
    mirror_rule_for_source (unit-pinned).
  - E2e (toml-only — the C oracle aborts on this config, cited in the
    tests): VNC mirrors headless at the SOURCE's 1024x640 (the
    negotiated framebuffer geometry over a real RFB connection, not
    just logs), the vnc-first order pins the deferral, and the
    no-remote-backend case is fatal.  55 passed / 1 skipped.
  - Review round on this slice, two fidelity fixes in L6:
    - The device-changed reset C does right after its enable
      (wet_output_handle_create) was missing.  The remote head is
      created with the flag set (weston_head_set_monitor_strings), and
      the heads-changed flush L6 nests inside still has that head to
      visit — now enabled — so `--backends=headless,vnc` logged a
      spurious `Detected a monitor change on head 'vnc'` that C never
      emits.  E2e now asserts the line's absence.
    - L6 hardcoded the VNC configurator for the mirrored head.  C takes
      it from the head's OWN backend (wet_get_backend_from_head +
      assert(wb)), so the kind now decides — the vnc-only shape is a
      frontend validation rule, not an assumption the fence gets to
      make — and a head from a backend we did not load is skipped
      where C asserts.
  - **Caught by the r0 valgrind gate during this slice's own
    validation**: the first cut parked the two new frontend listeners
    (L6/L2) in ctx.own_listener, whose teardown runs AFTER
    weston_compositor_destroy when no shell is attached — but the
    nodes sit on signal lists INSIDE the compositor struct, so the
    detach wrote into freed memory (r0-smoke has no shell; the shell
    path masked it by tearing down inside the destroy emission).
    They are Compositor-owned now, detached next to heads_changed
    before the destroy — the same ownership rule that listener always
    had, and the reason it has it.
  - Validation: valgrind clean on the mirror config AND the no-shell
    r0 leg (0 errors, 0 definite leaks; the 5 suppressed are the known
    upstream vnc ones); C-oracle test_outputs re-run green (new tests
    correctly skip); clippy/fmt/fence/unit (53) green; full smoke +
    rust e2e (55 passed / 1 skipped).

- **R2c-vnc (2026-07-26)** — the VNC backend + multi-backend loading
  (plan §7 R2c, first slice), branch `claude/rust-migration-j84b2p`:
  - Fence: `load_vnc` (versioned config assembly; the backend strdup's
    the strings, so temporaries are the contract), builder restructured
    to an ordered backend list (`add_headless`/`add_vnc` — C
    load_backends order), heads-changed now keeps a per-backend
    discriminator list and dispatches each head to its backend's
    configure flavor (the `wb->simple_output_configure` role):
    windowed (headless) vs VNC (three-arg `output_set_size` with
    `resizeable`; section-only scale; transform forced normal).
    `OutputPolicy::decide_vnc` + `[output] resizeable=` in the model.
  - **Found live: the VNC backend segfaults without xkb names.**
    vnc-backend strdup's `compositor->xkb_names` unconditionally
    (vnc.c:1260); the C frontend populates them via
    `weston_compositor_init_config` → `weston_compositor_set_xkb_rule_
    names` before any backend loads.  Ported that init slice:
    `[keyboard]` xkb names (C-owned strdup copies; libweston frees),
    repeat rate/delay, vt-switching, `[core] repaint-window`
    (validated -10..=1000 with C's messages; new model key — an R-G
    gap, the ini surface had it).
  - CLI: `--backend`/`--backends` are one C variable (main.c:4458-9,
    last occurrence wins, either takes a comma list) — was modeled as
    two fields with `--backend` priority, which broke
    `--backend=vnc --backends=headless,vnc` (the e2e multi-backend
    spelling); now one clap field with a visible alias and
    self-override.  New `--address` flag + `[vnc] address` (R-G gap).
  - E2e: `scripts/rust-e2e-test.sh` now runs the **whole suite** minus
    `test_xwayland.py` (R2d) against `westonite-rs` — meson-built
    wtest clients + the VNC PAM stack, TOML mode: **51 passed,
    1 skipped** (the EPEL resize-bug skip), including every
    VNC-framebuffer shell test.  The background test's config-filename
    assert became mode-aware (`CONFIG_NAME`); behaviors unchanged.
    C-oracle legs re-verified on the touched files.
  - Validation: valgrind clean on VNC startup/shutdown **for our
    code** — the run surfaces 71 bytes/5 blocks of *upstream*
    vnc-backend leaks (xkb_rule_name strdups + formats array never
    freed in vnc_destroy, + one weston_output_set_single_mode entry),
    verified byte-identical under the C frontend on the same
    invocation; RPM-side, out of scope per the our-code-only decision.
    `scripts/valgrind-upstream-vnc.supp` suppresses definite leaks
    allocated under a `vnc-backend.so` frame — broader than those five
    blocks, and deliberately so (see the header comment in the file):
    the frontend never allocates through that call tree, so the new
    rust-smoke VNC leg still gates our code at zero.  Live probes:
    VNC framebuffer capture shows the shell background (0xff002244 →
    BGR 44/22/00) at the configured geometry — empirical end-to-end
    proof of the R2b OutputCreated ordering fix.
  - Review round on the above, same branch:
    - `compositor->multi_backend` was never set.  C sets it from the
      comma in the backends string before load_backends
      (main.c:4653), and libweston keys the early buffer-release
      optimization off it (`output_accumulate_damage`,
      compositor.c:3405) — with two backends loaded and the flag
      false, the core reference is dropped while the second backend
      still needs the buffer.  Now set from the builder's backend
      count.
    - Renderer AUTO was pre-resolved to noop whenever headless was in
      the list.  C hands AUTO to every loader and lets the first
      backend to reach `if (!compositor->renderer)` decide, so
      `--backends=vnc,headless` handed the VNC backend
      WESTON_RENDERER_NOOP — which it rejects outright — where C
      comes up on pixman.  AUTO is now passed through unresolved.
    - `[core] backends` / `[core] backend` are one variable too
      (main.c:4602-4606): `backends` is read first and `backend` is
      its fallback, and either is comma-split.  We had the precedence
      inverted and never split the singular key.
    - The unhandled-option applicability table only covered flags one
      *ported* backend consumes, so `--seat`, `--fullscreen`,
      `--no-input`, the rdp flags and friends — consumed by no
      loadable backend at all — were silently ignored where C is
      fatal.  Third table added; `--use-gl`/`--use-pixman` joined the
      headless-only one.
    - Documented (not replicated) a C bug the multi-backend path
      exposes: each loader's `wet_init_parsed_options` replaces
      `compositor->parsed_options` with a zeroed table, so after two
      loads the configure callbacks read an empty one and
      `--width`/`--height`/`--scale`/`--transform` reach no output at
      all.  We apply the CLI layer to every backend; see the
      divergence list on `build_output_policy`.
    - The xkb strdup copies leaked if `set_xkb_rule_names` failed
      (it returns before taking ownership); freed on that path now.
    - rust-smoke leg 7 waited only for the wayland socket, which is
      bound *before* the heads flush — it now waits for
      `Output 'vnc' enabled`, so the VNC configure path is actually
      inside the valgrind window.

- **R2b headless slice (2026-07-26)** — output management, the
  e2e-verifiable half (plan §7 R2b), branch
  `claude/rust-migration-j84b2p`:
  - New fence module `output_policy`: typed `OutputTransform` (the C
    `transforms[]` grammar exactly), `OutputRule`/`OutputPolicy` —
    plain data resolved once by the frontend, so the heads-changed
    trampoline's A3 proof stays "wrapper state only".  Precedence per
    head mirrors C: backend defaults → name-matched `[[output]]`
    section → CLI overrides (unit-pinned).
  - heads-changed handler now implements all three C
    `simple_heads_changed` branches (enable / disable on unplug /
    device-changed log + flag reset) plus the policy lookup;
    `weston_output_lazy_align` ported (tail-of-output-list placement);
    enable path applies scale/transform/size per decision.  `off =
    true` / `mode = "off"` leaves a head unenabled (re-spec extension;
    logged).  Builder: `with_output_policy`, `with_no_outputs` (C
    --no-outputs skips head creation), `with_refresh_mhz`
    (C --refresh-rate passthrough).
  - Frontend: `--transform` CLI flag added (real C headless option the
    R-G sweep missed), `--scale`/`--no-outputs`/`--refresh-rate` and
    `[[output]]` scale/transform/off unblocked; invalid transform is a
    startup error with the C wording; invalid mode logs C's "Invalid
    mode … Using defaults.".  Still fail-loud (DRM-bound):
    clone-of/mirror-of, icc/eotf/colorimetry/color-characteristics/
    max-bpc/vrr, [core] color-management.
  - E2e: rust leg now runs `test_outputs.py`'s headless subset —
    geometry from CLI and scale+transform from config pass against
    `westonite-rs` (30 passed / 4 deselected: VNC tests wait for R2c).
  - Validation: workspace clippy-clean; 3 new policy unit tests;
    live probes (--no-outputs, off-rule, bad transform fatal,
    --refresh-rate) green; valgrind clean on the rotate-90/scale-2
    config incl. shutdown (0 errors, 0 definite leaks); smoke + fence
    checks green.
  - Review fixes (branch `claude/review-pr-20-vcjjei`):
    - **Startup no longer succeeds with missing outputs.**  C keeps
      `wet_compositor.init_failed` and checks it right after
      `weston_compositor_flush_heads_changed`; the wrapper had no
      equivalent, so a create/configure/enable failure only logged and
      left a compositor running with no outputs and exiting 0 — the
      silent degradation the fail-loud gate exists to prevent, newly
      reachable now that config decides the geometry.  `Ctx` carries
      the flag; `build()` turns it into `CompositorError::OutputInit`.
      The three failure log lines now use C's wording
      (`Could not create an output for head "%s".` / `Cannot
      configure output "%s".`; the enable failure is left to
      libweston, which already logs C's exact line).
    - **heads-changed filters by backend.**  C keys the listener off
      `struct wet_backend` and iterates only that backend's heads
      (`wet_backend_iterate_heads`).  The wrapper installs one
      compositor-wide listener and walked every head, which is
      indistinguishable today (headless only) but would configure
      foreign heads with this backend's policy and enable them through
      the *headless* windowed API the moment R2c lands a second
      backend — exactly what the deselected
      `test_multi_backend_headless_plus_vnc` covers.  The backend
      pointer is recorded at load and compared per head.
    - **Output registration moved after `weston_output_enable`**, as C
      does with `wet_head_tracker_create`.  Registering first claimed
      the registry slot before `output_created_signal` fired, so the
      shell's listener took its already-registered early return and
      `Event::OutputCreated` — which is what creates the background
      curtain — was never dispatched under the Rust frontend.  It also
      left two destroy listeners keyed on one output address, and
      emitted `OutputGone` for an output the app had never been told
      about.  The hybrid R1 path (C frontend + Rust shell) never hit
      this because C does not call the wrapper's registration.
    - `lazy_align` now uses the crate's one `container_of!` (§3c)
      instead of open-coding the same pointer arithmetic; the
      `off`-rule log is gated to once per head (it sits on a head that
      stays connected-and-unenabled, so every later flush re-logged);
      `disable_head` lost its unused `ctx` parameter.
    - `[[output]] scale` positivity moved to `westonite-config`,
      beside the `--width`/`--height`/`--scale` checks it shares a
      rule with.  `parse_mode` no longer trims, so `1024 x 640` is
      rejected as C rejects it.  The two remaining deliberate
      divergences from C's section handling (all sections validated
      eagerly rather than lazily; no `Invalid mode` warning for a
      section with no `mode` key at all) are now stated in
      `build_output_policy`/`reject_unported` and
      docs/config-migration.md instead of being contradicted by their
      own doc comments.
    - Coverage: `test_outputs.py` gains `--transform` from CLI, bad
      transform fatal, invalid-mode fallback, `--no-outputs`, and
      (Rust-only) `off = true` plus its scoping; `parse_mode` and the
      `[[output]] scale` rule get unit tests.  `lazy_align`'s peer
      branch is still unreachable on a single-head backend — noted at
      the function, covered when R2c brings a second backend.  The
      redundant `--deselect` for the already-`skip`ped VNC resize test
      is gone.
    - Docs: plan §7 records the R2b split (headless half done, DRM
      remainder with R2c) rather than leaving it in a PR description;
      callback-inventory L2 moves to R2c; frontend-capabilities'
      headless row lists the options the R-G sweep had missed
      (`--scale`, `--transform`, `--fake-seat`, the renderer forces);
      `westonite.toml.example` documents the `[[output]]` keys and
      `off`.

- **R2a (2026-07-26)** — Rust frontend core: config + spawn + headless
  (plan §7 R2a), branch `claude/rust-migration-j84b2p`:
  - New safe crates: `westonite-config` (§5 re-spec, D9–D12: serde
    `Config` over the full ini surface, kebab-case,
    `deny_unknown_fields`, clap CLI, `-o` dotted overrides, XDG
    discovery of `westonite.toml` with the D11 legacy-ini hint,
    resolution defaults → file → `-o` → flags into immutable
    `Settings`; C-parity error strings) and the `westonite` binary
    (`westonite-rs`: version/help, `--log` file sink, C
    `verify_xdg_runtime_dir` port, headless bring-up via the fence
    builder, statically linked Rust shell, autolaunch with X_OK
    precheck and watch, `-o`-driven config).  New audited-unsafe crate
    `westonite-spawn` (risk R-D: `pre_exec` limited to
    async-signal-safe calls — sigmask reset, setsid, CLOEXEC clear).
  - Fence additions: `attach_shell_native` (shared `wire_common` with
    the hybrid path), `with_shell`/`with_socket_name` builder,
    explicit-socket bind + `socket_name()`, `set_autolaunch` +
    SIGCHLD event source (installed in `build()` — before any client
    spawn, matching C's signals[]-before-`execute_autolaunch` order so
    a fast-exiting watched client is never lost), `--log` file sink in
    the log shim.  D12: no `WESTON_CONFIG_FILE` export.
  - E2e: `test_cli.py` re-specified (mode-gated via
    `WESTONITE_CONFIG_FORMAT`; new TOML-only tests: legacy-ini hint,
    `-o` override plumb-through, unknown-key startup error, unported
    backend fails loudly); harness gained a TOML config mode +
    ini→TOML translation so `test_lifecycle.py`/`test_children.py`
    behaviors run unmodified against both frontends (only the D12
    export assert is mode-gated).  New `scripts/rust-e2e-test.sh` CI
    leg: 27 passed / 1 deselected (VNC lifecycle — R2c) against
    `westonite-rs`; the C leg keeps the full suite.
  - Validation: workspace clippy-clean (`-D warnings`); 25 unit tests
    in the new crates (`westonite-config` 21, `westonite-spawn` 4) on
    top of the existing `weston`/`westonite-shell` suites; frontend
    valgrind-clean including the autolaunch/SIGCHLD watch path (0
    errors, 0 definite leaks); ASAN leg extended to the frontend
    binary; fence checks classify the new crates
    (safe: +westonite-config, +westonite; unsafe: +westonite-spawn).
  - Docs: `westonite.toml.example` (validated by running the binary
    against it), `docs/config-migration.md` (D11 ini→TOML mapping),
    callback-inventory §6 (event-loop signal sources) + log-sink row.
  - Review fixes folded into the same branch:
    - Config discovery tried `$XDG_CONFIG_HOME` **or** `$HOME/.config`;
      libweston's `open_config_file` falls through from the first to the
      second, so a set `XDG_CONFIG_HOME` was masking a config in the
      home fallback.  Both are searched now, in order.
    - `-o` overrides are parsed as TOML before deserialization, so the
      documented `-o shell.background-color=0xff002244` arrived as an
      *integer* and died as "invalid type: integer, expected a string".
      Numeric-string keys (`[shell] background-color`, `[libinput]
      scroll-button`) now accept a number as well as a string, in the
      file and through `-o`.
    - List keys (`[core] backends`, `[core] modules`) accept one
      comma-separated string as well as an array, so the ini spelling —
      which `-o core.backends=drm,vnc` and the e2e harness's ini→TOML
      translation both produce — resolves instead of erroring with
      "expected a sequence".
    - `verify_xdg_runtime_dir` masked the mode with `0o7777` (warning on
      a sticky/setgid runtime dir C accepts) and dropped C's
      `st_uid != getuid()` half of the check.  Both corrected; the
      `getuid` call sits in `westonite-spawn` (which already owns the
      process-credential syscalls, and needs the same value for
      `wet_client_launch`'s `seteuid(getuid())` later) so the frontend
      keeps `forbid(unsafe_code)`.
    - Fail-loud gaps: `--scale`, `--refresh-rate` and `[[output]]`
      `scale`/`transform`/`mode = "off"` for the headless head were
      accepted and silently ignored (the R2a bring-up passes only
      width/height to the fence); conflicting `--use-gl`/`--use-pixman`
      /`--renderer` picked a winner where C errors; non-positive
      `--width`/`--height`/`--scale` reached the backend as a geometry.
      All are startup errors now.
    - `[vnc] port` outranked `[rdp] port` regardless of the loaded
      backend; the section is selected by the active backend now.  The
      rdp/vnc TLS and listener flags were parsed by clap and dropped
      before `Settings` — plumbed through for the R2c consumer.
    - `attach_shell_native` returned success when the compositor already
      had the destroy listener, leaving a compositor running with no
      shell wired; it fails the build instead (the hybrid path's
      yield-to-the-other-shell semantics do not apply to a single call
      site).
    - `overrides::apply` inserted an empty section into the tree before
      rejecting a single-segment path; the shape is validated first.
    - E2e harness: `[color_characteristics]` translated to
      `[[color_characteristics]]`, which the kebab-case model rejects —
      it maps to `[[color-characteristics]]` now, and the rename is
      documented in `docs/config-migration.md`.
    - `westonite-spawn::from_exec_string`'s two narrowings of C's
      assignment rule (keys containing `/`, empty keys) were commented
      as C parity; documented as the deliberate divergences they are.
- **Review hardening (2026-07-26)** — post-merge review of the R0/R1 +
  planning PRs (#11, #13–#16), branch
  `claude/rust-migration-review-b9rjxw`:
  - **RPM now ships the Rust shell**: `rpm/westonite.spec` builds
    `westonite-shell-plugin` in `%build` (offline, against crates
    vendored by `scripts/rpm-build.sh` as Source1) and installs it over
    the meson-built `desktop-shell.so` — previously the release RPM
    silently shipped the C shell while CI tested the Rust one.
  - Fence-check hardening: transitive dependency-graph walk (was
    direct-deps only), unclassified-workspace-crate failure, anchored
    `forbid(unsafe_code)` + Cargo.toml lint check, bindings regenerated
    to a scratch file (tree untouched), `FENCE_CHECK_BINDINGS=1` forced
    in CI with a pinned `bindgen-cli` baked into the build image (the
    drift gate previously always skipped in CI).
  - `cargo --locked` on every build/test/metadata invocation;
    `regen-bindings.sh` pins and asserts the bindgen version.
  - Smoke hardening: valgrind leg now asserts the same startup/exit
    markers as the plain leg; the Rust shell logs an identifying init
    marker that `smoke-test.sh` requires (and requires absent under
    `WESTONITE_C_ORACLE=1`); CI gains a C-oracle smoke leg (plan R-B);
    stress-test watchdog replaced with an in-process teardown deadline.
  - Fence soundness: per-seat pointer-destroy guard listener — detaches
    the pointer-focus listener inside `weston_pointer_destroy`'s
    destroy_signal emission, because `weston_seat_release` frees the
    pointer *before* emitting the seat destroy_signal (the C shell has
    the same latent hazard, masked by backend release order).
  - Dead `weston-sys/hybrid-r1` feature removed;
    `undocumented_unsafe_blocks` extended to the plugin crate; doc
    corrections (callback-inventory vestigial rows L19/L20/L26, field
    count 29, stale anchors; plan: public-api gap and sanitizer claims
    stated honestly, dlsym compat path, count fixes; VENDOR.md deleted-
    file references).
- **R1 (2026-07-25)** — shell in Rust, frontend still C (plan §7 R1),
  branch `claude/rust-migration-j84b2p`:
  - `westonite-shell` (safe, `forbid(unsafe_code)`, no-unwrap lints):
    the shell.c policy state machine against `ShellHost`; unit-tested
    against a mock host (focus churn, child activation, replacement
    hunt, teardown ordering — D20).
  - Fence growth in `weston`: full desktop-api vtable (§3d),
    move/resize/busy pointer grabs + touch move grab with deferred drop
    (§3f), curtains, pinned shell layers (kind 4), seat/output/session/
    transform wiring, input bindings, `track_surface` focus tracking,
    hybrid bootstrap (`shell_init`, feature `hybrid-r1`, dlsym contract
    with libexec_westonite — deleted at R3).
  - `westonite-shell-plugin` cdylib installed as `desktop-shell.so` by
    `scripts/rust-shell-install.sh` (smoke/e2e now ship the Rust
    shell; `WESTONITE_C_ORACLE=1` restores the C oracle).
  - Exit criteria met: all 3 smoke tests green; **full e2e suite green
    (44 passed, 1 skipped)** against the Rust shell; destroy-storm
    stress (`scripts/rust-stress-test.sh`: sequential add/kill churn,
    staggered concurrent kills, rapid short-lived toplevel churn — all
    single xdg toplevels; transient/parent-child teardown is covered by
    the e2e suite only) valgrind-clean.  D18 note:
    the hybrid process (C frontend + Rust cdylib) is validated under
    valgrind; nightly-ASAN covers the pure-Rust legs
    (rust-asan-smoke.sh), since Rust ASAN cannot instrument the C half.
  - Found live during bring-up: installing the wrapper's weston-log
    handlers from the plugin hijacks the frontend's --log file — the
    hybrid shell must not touch weston_log_set_handler (recorded in
    shell_init.rs).
  - Post-review hardening: grab teardown parking, busy-grab
    current-grab check, ref-counted pointer-focus tracking, and a fixed
    hybrid double-init guard; CI now runs the `westonite-shell` unit
    tests, and the stress script asserts real client churn (mapped
    count) and adds a teardown watchdog.
- **R0 (2026-07-25)** — foundation phase (plan §7 R0), branch
  `claude/rust-migration-j84b2p`:
  - Cargo workspace beside the meson tree; `weston-sys` (bindgen over
    the installed EPEL 14.0.1 headers, bindings committed per D5, regen
    via `scripts/regen-bindings.sh`, pkg-config version tripwire in
    `build.rs`, `cc` shim) and the `weston` fence crate.
  - §3 primitives: generational-id registry (§3b), `Listener` (§3c),
    two-tier dispatch with depth-counted drain (§3e/A4), `Grab`
    skeleton with deferred drop (§3f), panic barrier (D16), thread-local
    `Ctx` slot (D21), `ShellHost` core (D20), fake-C-object test
    harness (D18).
  - §3 header facts re-verified against the installed headers:
    `docs/r0-header-facts.md` (2 live findings: mandatory
    `weston_compositor_backends_loaded`, `weston_output_lazy_align` is
    frontend-local).  Callback inventory: `docs/callback-inventory.md`.
  - Exit criterion met: `r0-smoke` (compositor + headless + noop
    renderer) exits 0 on SIGTERM — clean under valgrind (0 errors,
    0 definite leaks) and AddressSanitizer (nightly `-Zsanitizer=address`).
  - CI: rustfmt / clippy `-D warnings` (+ `undocumented_unsafe_blocks`,
    `unsafe_op_in_unsafe_fn`) / unit tests / fence checks
    (`scripts/rust-fence-check.sh`) / valgrind smoke / ASAN job.
    Build image gains rust-toolset 1.96, clang-libs, valgrind.
