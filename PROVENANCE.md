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

## Migration log

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
  - **Found live: C aborts on a headless mirror source.**  Only
    DRM-class backends call weston_output_copy_native_mode, so a
    windowed/headless source leaves native_mode_copy zeroed and C dies
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
    tests): VNC mirrors headless at the SOURCE's 1024x640 (framebuffer
    capture, not just logs), the vnc-first order pins the deferral,
    and the no-remote-backend case is fatal.  55 passed / 1 skipped.
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
