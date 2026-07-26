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
  - Validation: workspace clippy-clean (`-D warnings`); 27 crate unit
    tests; frontend valgrind-clean including the autolaunch/SIGCHLD
    watch path (0 errors, 0 definite leaks); ASAN leg extended to the
    frontend binary; fence checks classify the new crates
    (safe: +westonite-config, +westonite; unsafe: +westonite-spawn).
  - Docs: `westonite.toml.example` (validated by running the binary
    against it), `docs/config-migration.md` (D11 ini→TOML mapping),
    callback-inventory §6 (event-loop signal sources) + log-sink row.
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
