# Future plan: Rust migration

Status: **planned, not yet implemented**. Translates the C sources in
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
   upgrade path, smoke tests pass unchanged.

Non-goals: porting libweston itself; changing the libweston version
(stays 14.0.1/EPEL); adding features during the port (the
maintenance-layer plan sequences separately — see Open questions).

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
| `frontend/weston-screenshooter.c` | port or drop | Semi-dead today (spawns a client we don't ship); recommend **drop** during the port, keep the Super+R wcap recorder decision separate. |
| `frontend/xwayland.c` | `westonite::xwayland` | Socketpairs via rustix, lazy spawn via the process module, SIGUSR1 via `wl_event_loop` signal source (bound). |
| `desktop-shell/shell.c` | `westonite-shell` crate | `state.rs` (DesktopShell), `surface.rs` (ShellSurface + DesktopApi impl), `focus.rs`, `bindings.rs`, `background.rs` (curtains/output tracking), `workspace.rs`, `fullscreen.rs`. Exported entry: `wet_shell_init`-compatible `extern "C"` if cdylib, plain fn if static (linkage question). |

## 5. Build & packaging

- **Build**: cargo replaces meson at the end state. `weston-sys`
  resolves the RPM headers via `pkg-config libweston-14` in
  `build.rs`. During the hybrid phases meson keeps building the
  not-yet-ported C parts and cargo the Rust parts (meson `custom_target`
  or just the CI script invoking both).
- **bindgen**: run at build time (needs `clang-libs`, in AppStream)
  **or** commit pre-generated `bindings.rs` with a regen script +
  CI check. Recommendation: **commit the bindings** — RPM builds get
  fewer BuildRequires and full reproducibility; the headers only
  change when EPEL bumps weston, which is exactly when we re-run the
  script (tripwire: build.rs asserts the libweston-14 pkg-config
  version matches the bindings' recorded version).
- **Toolchain**: EL10 AppStream `rust-toolset` (rolling, recent
  rustc). MSRV = whatever the build image's rust-toolset ships;
  edition 2024.
- **Dependencies policy**: minimal, RPM-friendly — `libc`, `rustix`,
  `bitflags`, `thiserror`, `cc` + `bindgen` (build-time only, only if
  not committing bindings). No clap, no tokio, no wayland-rs. RPM
  builds offline from a `cargo vendor` tarball (`Source1`), per
  standard EL Rust packaging practice.
- **RPM/installed layout unchanged**: `/usr/bin/westonite`,
  `%{_libdir}/westonite/*.so` (if the cdylib layout survives — see
  linkage question), same `Requires: weston-libs`. Spec swaps
  meson/gcc BuildRequires for rust-toolset + vendored crates.
- **CI**: existing image-build → build+smoke → rpmbuild → pristine
  install pipeline unchanged in shape; adds `cargo fmt --check`,
  `clippy -D warnings`, `cargo test`, and the unsafe-fence check.

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
  startup. Verified by the full existing smoke suite + eyes-on nested
  (x11/wayland backend) session for focus/fullscreen/move/resize
  behavior parity.
- **Phase R2 — shared + frontend in Rust** (slices, each ending
  green): R2a core startup, CLI/config, logging, headless (smoke
  passes on all-Rust path); R2b output management + DRM (the largest
  block: heads, hotplug, clone/mirror, color mgmt); R2c remaining
  backends (x11/wayland/rdp/vnc/pipewire — or fewer, per the trim
  question); R2d xwayland (Phase-3 smoke test must pass); R2e
  screenshooter/recorder if kept. The C `main.c` stays in-tree,
  buildable via meson, until R2 completes — it is the reference
  oracle for behavioral diffs.
- **Phase R3 — decommission C**: delete C sources + meson, switch
  spec + CI to cargo-only, RPM install test in pristine container,
  docs rewrite (README, VENDOR.md provenance model — below).
- **Phase R4 — idiom & hardening pass**: with parity locked in,
  refactor away remaining C-shaped code (pointer-keyed maps →
  arenas/typed handles where it pays), clippy pedantic triage, SAFETY
  comment audit, optional sanitizer run of the smoke suite.

Each phase = one PR series with the same discipline as the port
phases: build container, smoke scripts, VENDOR.md-style log entries.

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
  UB. Mitigation: narrow borrow scopes as a review rule; nested
  eyes-on testing in R1 (headless smoke cannot exercise focus churn).
- **R-B Behavior drift** in CLI/ini handling: mitigated by binding
  the RPM's config parser (not rewriting), porting option-parser
  faithfully with a test vector suite generated from the C
  implementation, and keeping the C build alive as an oracle until R3.
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

## 9. Open questions

Decisions needed before R0 (recommendations inline):

1. **Trim first?** The frontend inventory lists deployment-dependent
   blocks (RDP/VNC/PipeWire backends+plugins ~400 lines, clone/mirror
   multi-head, color management/HDR, multi-backend `--backends`,
   screenshooter, inert idle-time) totaling roughly 1–1.5k lines.
   Porting less C means less unsafe surface and fewer parity tests.
   **Recommend**: answer the capability-doc drop questions first and
   port only what survives.
2. **Shell linkage**: keep the upstream three-artifact layout
   (`westonite` + `libexec_westonite.so` + `desktop-shell.so` cdylib,
   dlopen'd — faithful, keeps `--shell`/`modules=` genericity) **or**
   link `westonite-shell` statically into the binary at R3 (one
   artifact, no cross-`.so` Rust ABI concerns, no dlopen of our own
   code; `modules=` dlopen for *third-party* C modules can stay
   either way). **Recommend**: cdylib during R1 (forced by the hybrid
   phase anyway), fold to static at R3 unless `--shell` swapping is a
   real requirement.
3. **Config parser**: bind the RPM's exported `weston_config_*`
   (zero drift, keeps `weston.ini(5)` semantics bug-for-bug — including
   the known 14.0.1 `binding-modifier none` limitation) **or**
   reimplement in Rust (idiomatic, testable, fixes that bug, but
   risks subtle divergence). **Recommend**: bind now; revisit only if
   the parser becomes a maintenance pain.
4. **Bindings generation**: committed pre-generated `bindings.rs`
   (recommended, §5) vs bindgen on every build.
5. **Parity bar**: is byte-identical CLI/`--help`/log output required,
   or is "same accepted inputs, same behavior, cosmetic text may
   differ" acceptable? **Recommend** the latter, documented.
6. **Maintenance-layer feature** (`maintenance-layer-plan.md`):
   implement in C before R1 (then port it), or make it the first
   Rust-native feature after R1? **Recommend**: after — implementing
   it twice is waste, and it lands on the freshly-designed safe
   wrapper as a proof of idiomatic feature work.
7. **Crate dependency policy**: is the §5 minimal list acceptable, or
   is there an org-level constraint (e.g. only crates already
   packaged in EL/EPEL, vs anything vendorable)?
