# Future plan: Rust migration

Status: **planned, not yet implemented**. All scoping decisions were
made at review time and are folded in below (§9 records them, D1–D8).
No open questions remain — the plan is ready to execute at R0. Translates the C sources in
this repo (frontend + desktop-shell + shared, ~9.8k lines) to Rust.
libweston 14 stays the compositor engine, consumed from the EPEL 10
RPMs exactly as today; libwayland stays the protocol/event-loop
runtime underneath it. Nothing about the runtime split changes — the
RPM still owns rendering, input, backends, and the xwayland module;
we still own picking and configuring all of it. Only the language of
*our* layer changes.

Goals, in priority order:

1. **Memory safety by construction** in our layer: all `unsafe` lives
   in one dedicated FFI-wrapper crate with documented invariants;
   application logic (shell policy, config plumbing, process
   management) is 100 % safe Rust.
2. **Idiomatic Rust**, not transliterated C: enums over int flags,
   `Option`/`Result` over sentinel returns, RAII over destroy
   listeners where we own the object, iterators over intrusive-list
   macros.
3. **Behavioral parity** with the current C build: same CLI, same
   `westonite.ini` semantics, same installed file layout, same RPM
   upgrade path — and the smoke scripts plus the **black-box e2e
   suite** (`tests/e2e/`, `docs/e2e-test-plan.md`) pass unchanged.
   The e2e suite drives the compositor exactly as a user would (VNC
   input injection + framebuffer capture, no test hooks in shipped
   code), so it runs identically against the C build, every hybrid
   phase, and the final Rust build — it is the migration's fixed
   parity oracle.

Non-goals: porting libweston itself; changing the libweston version
(stays 14.0.1/EPEL); adding features during the port (the
maintenance-layer plan is deferred separately — §9 D7); porting the
e2e suite itself (the Python harness and the C `wtest-client`/
`wtest-xclient` test drivers stay as they are — a measuring stick
must not move while the thing it measures is rebuilt).

## 1. What makes this tractable (post-trim inventory)

The T-series trims did the Rust port a big favor:

- **No custom Wayland protocol code remains.** The
  `weston-desktop-shell` globals were removed with the panel; the
  shell today creates zero `wl_global`s and generates no
  `wayland-scanner` code. The port therefore needs **no wayland-rs
  and no scanner integration** — libwayland is consumed only through
  the types embedded in libweston's API (`wl_listener`, `wl_signal`,
  `wl_list`, `wl_event_loop`, `wl_display`, `wl_client`,
  `wl_resource`).
- **The shell↔frontend contract is one function.** `shell.c` calls
  exactly one symbol from `libexec_westonite.so`: `wet_get_config()`.
  Everything else it uses is installed libweston API
  (`desktop.h`, `shell-utils.h`, `config-parser.h`).
- The `weston_config_*` parser is **exported by the RPM's
  libweston** — we bind it, we do not rewrite it (option below).

Current C surface to port (lines at trim T9):

| Component | Files | Lines | Character |
|---|---|---|---|
| frontend | `main.c`, `config-helpers.c`, `weston-screenshooter.c`, `xwayland.c`, `executable.c` | ~5.4k | wide but shallow: config → libweston-API plumbing |
| desktop-shell | `shell.c`, `shell.h` | ~2.3k | deep: callback-driven state machine (focus, layers, xdg lifecycle) |
| shared | `option-parser.c`, `os-compatibility.c`, `process-util.c` + 7 headers | ~2.1k | support code, mostly replaceable by std/rustix |

## 2. Crate architecture

Cargo workspace, replacing the meson tree at the end of the
migration (hybrid during it — see Phasing):

```
Cargo.toml                    # workspace
crates/
├── weston-sys/               # UNSAFE. bindgen over the installed RPM headers:
│                             #   libweston.h, desktop.h, shell-utils.h, weston-log.h,
│                             #   config-parser.h, windowed-output-api.h, backend-*.h,
│                             #   xwayland-api.h (+ the wayland-server types they embed)
│                             # links: libweston-14, wayland-server
├── weston/                   # UNSAFE INSIDE, SAFE API. The fence. Hand-written safe
│                             # wrappers: Compositor, Output/Head, Seat, Layer, View,
│                             # Surface, DesktopApi trait, Curtain, Config, Listener<T>,
│                             # LogScope, key/button-binding registration, module loading
├── westonite-shared/         # SAFE. Ports of shared/: option parser, process spawning
│                             # (CustomEnv/Fdstr), fd/socket helpers via rustix
├── westonite-shell/          # SAFE. desktop-shell logic (shell.c port)
└── westonite/                # SAFE. The frontend binary (main.c port); links
                              # westonite-shell (linkage question below)
```

Fencing rules, enforced by `#![forbid(unsafe_code)]` on the three
safe crates and CI grep:

- `weston-sys` is machine-generated + a small `cc`-compiled shim for
  the ~28 `static inline` helpers in the installed headers that
  bindgen cannot emit (or Rust reimplementations where trivial, e.g.
  timespec math).
- `weston` is the **only** hand-written unsafe code. Every `unsafe`
  block carries a `// SAFETY:` invariant comment; the crate's docs
  state the global invariants (single-threaded, callback reentrancy,
  object lifetime rules below).
- `westonite`, `westonite-shell`, `westonite-shared` contain no
  unsafe (one carve-out: `pre_exec` for fd setup in the spawn path
  lives inside `westonite-shared::process` behind a safe API, or in
  `weston` — decided during implementation).

## 3. The hard FFI problems and their designs

These five patterns cover essentially every unsafe interaction; each
gets one primitive in the `weston` crate.

**(a) `wl_listener`/`wl_signal` + `container_of`** — the pervasive C
idiom (65 uses across `shell.c`/`main.c`). Primitive:
`Listener<Args>` owning `Pin<Box<Inner>>` where `Inner` embeds the
`wl_listener` plus a boxed Rust closure; a single `extern "C"`
trampoline does the one `container_of` and invokes the closure.
`Drop` does `wl_list_remove`. This removes `container_of` from all
application code.

**(b) C-owned object lifetimes.** libweston owns
`weston_output/surface/seat/...` and announces death via destroy
signals. Handles in the safe layer are lightweight `Copy` newtypes
around `NonNull<T>`; **our** per-object state (e.g. the shell's
per-surface struct) is a `Box` registered via the object's user-data
slot with a destroy listener that reclaims the box — the same
ownership graph as the C code, but the pointer juggling happens once,
in the wrapper. Where libweston gives no user-data slot, a
pointer-keyed map inside the owning state struct replaces the C
intrusive lists.

**(c) `weston_desktop_api` and friends (C vtables).** Wrapped as a
Rust trait (`DesktopApi`) implemented by the shell; static `extern
"C"` trampolines fill the versioned/`struct_size`d C struct. Same
pattern for the xwayland API table and backend config structs
(builder types that assemble `weston_*_backend_config` with correct
`struct_size`/`struct_version`).

**(d) Reentrancy and aliasing.** libweston callbacks can re-enter
shell code synchronously (signal emission during teardown, focus
changes during activation). Policy: all mutable state lives in
`RefCell`s inside a single-threaded state struct; borrows are scoped
to never span a call back into libweston. Wrapper types are
`!Send + !Sync` (libweston is single-threaded). This is the one
class of bug Rust converts from memory corruption into a deterministic
`BorrowMutError` panic — strictly an improvement, and the panic policy
below makes it a clean abort with a log line.

**(e) Panics across FFI.** Every trampoline is
`catch_unwind` → `weston_log` → `abort()`; additionally
`panic = "abort"` in release profiles. No unwinding ever crosses the
C boundary.

## 4. Component-by-component mapping

| C source | Rust destination | Approach |
|---|---|---|
| `shared/option-parser.c` | `westonite-shared::options` | Faithful hand port (NOT clap — must keep exact weston CLI semantics: parse known options, pass the rest through). Note: `parse_options()` has no `WL_EXPORT`, so it must be reimplemented, unlike `weston_config_*`. |
| `shared/os-compatibility.c` | mostly deleted | EL10 glibc has `memfd_create`, `mkostemp`, `strchrnul`, `posix_fallocate`; use `rustix` for the cloexec/socketpair helpers. Only `os_socketpair_cloexec` semantics kept as a thin util. |
| `shared/process-util.c` | `westonite-shared::process` | `CustomEnv` (incl. the `ENV=x cmd arg` exec-string parser), `Fdstr`; spawn via `Command` + `pre_exec` (fd unCLOEXEC + setup), replacing the fork/exec block in `main.c`. |
| `shared/*.h` (helpers, timespec, xalloc, string-helpers, fd-util) | deleted | std (`Duration`, `?`, `String`), `rustix`; `xalloc` is irrelevant in Rust. |
| `frontend/executable.c` | deleted or 5-line stub | see linkage question. |
| `frontend/main.c` | `westonite` bin, split into modules | `cli.rs`, `config.rs`, `log.rs` (weston-log ctx, flight recorder, scopes), `backend/{drm,headless,x11,wayland,rdp,vnc,pipewire}.rs`, `output.rs` (head tracking, clone/mirror, color mgmt — the big one), `input.rs`, `autolaunch.rs`, `process.rs` (sigchld + wet_process list), `xwayland.rs`. |
| `frontend/config-helpers.c` | `weston::config` typed getters | On top of bound `weston_config_*`. |
| `frontend/weston-screenshooter.c` | `westonite::screenshooter` | Ported 1:1 (decision D1). Semi-dead at runtime (Super+S spawns a client we don't ship; Super+R wcap recorder works) — behavior preserved as-is. |
| `frontend/xwayland.c` | `westonite::xwayland` | Socketpairs via rustix, lazy spawn via the process module, SIGUSR1 via `wl_event_loop` signal source (bound). |
| `desktop-shell/shell.c` | `westonite-shell` crate | `state.rs` (DesktopShell), `surface.rs` (ShellSurface + DesktopApi impl), `focus.rs`, `bindings.rs`, `background.rs` (curtains/output tracking), `workspace.rs`, `fullscreen.rs`. Exported entry: `wet_shell_init`-compatible `extern "C"` if cdylib, plain fn if static (linkage question). |

## 5. Build & packaging

- **Build**: cargo replaces meson at the end state. `weston-sys`
  resolves the RPM headers via `pkg-config libweston-14` in
  `build.rs`. During the hybrid phases meson keeps building the
  not-yet-ported C parts and cargo the Rust parts (meson `custom_target`
  or just the CI script invoking both).
- **bindgen**: pre-generated `bindings.rs` is **committed** (decision
  D5), with a regen script + CI sync check. RPM builds get fewer
  BuildRequires and full reproducibility; the headers only change
  when EPEL bumps weston, which is exactly when we re-run the script
  (tripwire: build.rs asserts the libweston-14 pkg-config version
  matches the bindings' recorded version). `clang-libs`/bindgen are
  needed only by the regen script, not by builds.
- **Toolchain**: EL10 AppStream `rust-toolset` (rolling, recent
  rustc). MSRV = whatever the build image's rust-toolset ships;
  edition 2024.
- **Dependencies policy** (decision D8): the baseline runtime set is
  `libc`, `rustix`, `bitflags`, `thiserror` (+ `cc` at build time;
  bindgen only in the regen script). Further crates are allowed but
  **each addition requires explicit owner approval** before it lands
  (record the approval in the PR/VENDOR-log entry). No clap, no
  tokio, no wayland-rs. RPM builds offline from a `cargo vendor`
  tarball (`Source1`), per standard EL Rust packaging practice.
- **RPM/installed layout unchanged**: `/usr/bin/westonite`,
  `%{_libdir}/westonite/*.so` (if the cdylib layout survives — see
  linkage question), same `Requires: weston-libs`. Spec swaps
  meson/gcc BuildRequires for rust-toolset + vendored crates.
- **CI**: existing image-build → build+smoke+e2e → rpmbuild →
  pristine install (incl. installed-RPM e2e subset) pipeline
  unchanged in shape; adds `cargo fmt --check`, `clippy -D warnings`,
  `cargo test`, and the unsafe-fence check.

## 6. Phasing (strangler fig — a working compositor at every step)

The binary and the shell `.so` are separately loadable, so C and Rust
halves can be mixed and smoke-tested at every phase boundary.

- **Phase R0 — foundation**: workspace, `weston-sys` (bindings for
  the full header set), `weston` crate with the five primitives from
  §3 plus Compositor/log wrappers. Exit criterion: a throwaway Rust
  binary brings up compositor + headless backend + noop renderer,
  runs the event loop, exits 0 on SIGTERM (mirrors the Phase-1 smoke
  test).
- **Phase R1 — shell in Rust, frontend still C**: port `shell.c` →
  `westonite-shell` built as `desktop-shell.so` (cdylib exporting
  `wet_shell_init`, linking `libexec_westonite.so` for
  `wet_get_config`). This is deliberately first: it is the smaller
  but *deeper* half — it exercises every §3 primitive and hardens the
  wrapper design while the battle-tested C frontend still drives
  startup. Verified by the smoke scripts plus the e2e shell suites
  (`test_shell_windows.py`, `test_shell_background.py`,
  `test_children.py`): focus, activation, move/resize grabs, the
  ignored-request trims, and background pixels are asserted under CI
  via VNC — replacing the eyes-on nested session this phase would
  otherwise have needed.
- **Phase R2 — shared + frontend in Rust** (slices, each ending
  green — "green" means smoke **and** the full e2e suite): R2a core
  startup, CLI/config, logging, headless (`test_cli.py`,
  `test_lifecycle.py`, `test_children.py` on the all-Rust path — the
  CLI/config-discovery/autolaunch behaviors those tests pin are
  exactly the main.c logic this slice ports); R2b output management +
  DRM (the largest block: heads, hotplug, clone/mirror, color mgmt;
  `test_outputs.py` covers the headless/VNC-reachable subset — DRM
  paths remain hardware-verified); R2c remaining backends
  (x11/wayland/rdp/vnc/pipewire — VNC is load-bearing for the whole
  e2e control plane, so it lands first in this slice) plus the
  remoting/pipewire virtual-output plugin loaders; R2d xwayland
  (`test_xwayland.py` + Phase-3 smoke); R2e screenshooter/recorder.
  The C `main.c` stays in-tree, buildable via meson, until R2
  completes — it is the reference oracle for behavioral diffs.
- **Phase R3 — decommission C**: delete C sources + meson, switch
  spec + CI to cargo-only, RPM install test in pristine container,
  docs rewrite (README, VENDOR.md provenance model — below).
- **Phase R4 — idiom & hardening pass**: with parity locked in,
  refactor away remaining C-shaped code (pointer-keyed maps →
  arenas/typed handles where it pays), clippy pedantic triage, SAFETY
  comment audit, optional sanitizer run of the smoke suite.

Each phase = one PR series with the same discipline as the port
phases: build container, smoke scripts + e2e suite, VENDOR.md-style
log entries.

## 7. Provenance & upstream-rebase model

VENDOR.md's "verbatim import + discrete patches" model dies with the
C: a translation cannot be rebased with `git diff`. Replacement:

- A `PROVENANCE.md` table mapping every Rust module → source C file
  @ upstream tag (`14.0.1` + P0) at translation time.
- Rebase procedure when EPEL moves (e.g. 14.0.2 → 14.1): diff the
  upstream C between tags, walk the hunks, hand-apply semantic
  equivalents to the mapped Rust modules, log per-hunk disposition.
  More manual than today — this is the *structural cost* of the
  migration and should be accepted explicitly (the frontend↔libweston
  skew risk R4 from PLAN.md §9 carries over unchanged).
- `weston-sys` bindings are regenerated per libweston bump (scripted).

## 8. Risks

- **R-A Reentrancy panics** (§3d): `BorrowMutError` aborts replace C
  UB. Mitigation: narrow borrow scopes as a review rule; the e2e
  window-management suite exercises focus churn, grabs, and
  activation races under CI in R1 (headless smoke alone could not).
- **R-B Behavior drift** in CLI/ini handling: mitigated by binding
  the RPM's config parser (not rewriting), porting option-parser
  faithfully with a test vector suite generated from the C
  implementation, the e2e CLI/config/autolaunch tests pinning
  user-visible behavior, and keeping the C build alive as an oracle
  until R3.
- **R-C ABI drift on libweston bumps**: backend-config structs use
  `struct_size`/version fields — the builders in `weston` set them
  from the bound headers, and the pkg-config version tripwire (§5)
  catches silent header changes.
- **R-D `pre_exec`/fork-safety**: the spawn path runs code after
  `fork` — only async-signal-safe rustix calls allowed there;
  contained in one audited module.
- **R-E Toolchain/packaging friction**: EL10 rust-toolset version and
  vendored-crate spec mechanics; de-risked in R0 by building the
  foundation crate straight into an RPM in the existing container CI.
- **R-F Future protocol needs**: if a later feature needs a custom
  Wayland global again (e.g. reviving the shell client), the options
  are scanner-generated C + FFI, or wayland-rs's `sys` backend on the
  foreign `wl_display`. Out of scope now; noted so nobody assumes the
  no-scanner simplification is free forever.

## 9. Decisions (made at plan review)

- **D1 — Port everything 1:1, no pre-trim.** The full current
  capability surface (all seven backends' config, clone/mirror,
  color management, `--backends`, screenshooter, idle-time plumbing)
  is translated as-is. The drop-assessment questions in
  `docs/frontend-capabilities.md` stay open independently — trims can
  still happen later, in Rust.
- **D2 — Shell linkage: cdylib during migration, static at R3.**
  `westonite-shell` builds as `desktop-shell.so` (exporting
  `wet_shell_init`) for the hybrid phases; once the C frontend is
  gone it links statically into the `westonite` binary. `modules=`
  dlopen for third-party C modules survives; `--shell` swapping of
  our own shell does not (document in README at R3).
- **D3 — Config parser: bind the RPM's `weston_config_*`.** Thin safe
  wrapper over FFI; `weston.ini(5)` semantics stay bug-for-bug
  identical (including the known 14.0.1 `binding-modifier none`
  limitation). Revisit only if it becomes a maintenance pain.
- **D4 — Port order: shell first** (Phase R1 as written in §6).
- **D5 — Bindings committed**, regenerated by script on libweston
  bumps (§5).
- **D6 — Parity bar: behavioral.** Same accepted CLI options and ini
  keys, same runtime behavior and exit codes. Cosmetic text
  (`--help` wording, log phrasing) may differ where Rust idioms make
  it natural; every intentional divergence is documented in the
  porting PR.
- **D7 — Maintenance-layer feature deferred indefinitely.** The
  migration takes priority; `maintenance-layer-plan.md` stays parked
  and is rescheduled (if at all) only after R3/R4. It is *not* a
  milestone of this plan.
- **D8 — Crate policy: anything vendorable, per-crate approval.**
  Baseline set in §5; each addition beyond it needs explicit owner
  sign-off before merging.
