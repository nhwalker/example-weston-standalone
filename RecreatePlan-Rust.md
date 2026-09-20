# RecreatePlan-Rust.md — port the `westonite` frontend and shell to Rust

An execution plan for an agent that has **only the repository produced by
`RecreatePlan.md`** (the C `westonite`: Weston 14.0.1 frontend and
desktop-shell, built against the EPEL 10 libweston RPMs, trimmed T1–T8 /
F1–F6, with its VNC-driven e2e suite and the DRM VM harness) as reference
material. Following it produces a Rust `westonite`: a cargo workspace
beside the meson tree, in which all `unsafe` and all unsoundness live
behind one fence crate, the shell and frontend are `#![forbid(unsafe_code)]`
crates with plain-data memory models, the configuration interface is
re-specified as typed TOML, and the RPM ships the Rust binary with the
shell linked in. The C tree stays in the repository, buildable, as the
behavioural oracle the whole port is measured against.

Every decision in this document is settled. Where the record it was
reconstructed from could not be re-verified, the gap is stated
(Appendix C) rather than papered over.

---

## 0. How to use this document

**Audience.** One agent, working in the C `westonite` repository at the
commit where `RecreatePlan.md` §13 is fully checked, with:

- `docker` (or `podman`) able to build the two container images that
  repository already defines and reach the CentOS Stream 10 / EPEL 10
  repositories, plus crates.io once (to populate the cargo cache and to
  `cargo vendor` for the RPM);
- push access to the same GitHub repository with Actions enabled.

**What you will produce.** The same repository, plus a cargo workspace
(`Cargo.toml`, `crates/`) that builds `target/release/westonite-rs`; an
RPM that installs that binary as `/usr/bin/westonite`, the session file
and `westonite.toml.example`, and no longer installs any `.so`; a CI
pipeline whose gate is the Rust build (smoke, valgrind, ASAN, unit tests,
fence checks, the full e2e suite in TOML mode, the DRM VM leg) while the
C build keeps running as the oracle (its e2e leg and its DRM VM leg; the
hybrid C-frontend + Rust-shell configuration exists from R1 to R3 and is
deleted at R4); and the documents that make
the port auditable: the callback inventory, the header-fact table, the
config migration table and `PROVENANCE.md`.

**Phases.** Each phase ends green before the next starts; "green" means
the smoke scripts **and** the full e2e suite, in every configuration the
phase adds.

| Phase | Delivers |
|---|---|
| R0 | Cargo workspace; `weston-sys` (committed bindgen output, C shim, version tripwire); the `weston` fence crate's primitives; the fake-C-object unit harness; the callback inventory and header-fact table; the `r0-smoke` binary green plain, under valgrind and under ASAN; the fence checks and the `rust`/`rust-asan` CI jobs |
| R1 | The shell in Rust (`westonite-shell`, safe) shipped as `desktop-shell.so` through the `westonite-shell-plugin` cdylib and loaded by the **C** frontend; the mock-host unit tests; the destroy-storm stress test; the full e2e suite green against the hybrid |
| R2a | `westonite-config` (TOML + clap + `-o`), `westonite-spawn`, the `westonite` binary: logging stack, headless bring-up, autolaunch, signals, `--debug`, every refusal; the e2e harness's TOML mode; the config tests re-specified |
| R2b | Output policy: `[[output]]` mode/scale/transform/off, the CLI overrides, all three heads-changed branches, `init_failed` |
| R2c | The remaining backends against the C oracle: VNC + multi-backend, mirror-of, x11/wayland/pipewire, colour management, `[libinput]`, DRM with the layoutput machinery (verified in the VM) |
| R2d | Xwayland |
| R2e | The Super+R wcap recorder |
| R3 | Ship: the RPM installs the Rust binary as `westonite`; CI gates on the Rust legs; the C build becomes the oracle-only configuration; docs |
| R4 | Deletion of the hybrid scaffolding (first task), then hardening: the `cargo public-api` snapshot, `cargo doc` leg, feature-complete fence walk, the known wrapper-internal debts, clippy pedantic triage, SAFETY audit |

**Rules that apply throughout.**

1. **Never modify the upstream Weston tree**, and never edit the C
   sources under `frontend/`, `desktop-shell/` or `shared/` except where
   a phase names the edit (R1 needs none; R3 touches only build/packaging
   files). The C build is the oracle; an oracle that moves measures
   nothing.
2. **All `unsafe` and all unsoundness live in two crates** (`weston`,
   `westonite-spawn`) plus the generated `weston-sys`. The three safe
   crates carry `#![forbid(unsafe_code)]` with no exceptions and no
   `unwrap`/`expect` (lint-denied). Mechanically enforced from R0 (§4.3).
3. **Every phase and every slice ends green**: `cargo fmt --check`,
   `cargo clippy -D warnings`, unit tests, fence checks, the smoke
   scripts, and the e2e suite in every mode the slice supports, all in
   the build container. Do not stack unverified steps.
4. **Fail loud.** Anything a user can ask for through the CLI or the
   TOML file that the Rust frontend does not do is a startup error naming
   the fact, never a silent no-op. Every refusal has an e2e test. There is
   no "not yet ported" state visible to users at the end: by R3 every key
   and flag is either implemented or refused by product decision.
5. **Parity is behavioural, measured by the frozen e2e suite**, except
   the configuration interface, which is re-specified (§5). The Python
   harness and the C test clients are not rewritten; the harness gains a
   config-format mode and nothing else. Any observable difference between
   the C oracle and the Rust build outside the re-specified interface is
   a bug, not a divergence, unless it is listed in §5.6.
6. **Scope is our code only.** Bugs inside the EPEL `weston-libs` RPM are
   documented, suppressed where valgrind needs it (Appendix A.24), and
   never worked around in ways a test would encode.
7. **Measure before you change.** Every slice ports against the C oracle
   running the same tests; where the C build cannot run a case (it
   aborts, or the case is TOML-only), the plan says so.
8. Commit messages describe what changed and what verification passed;
   `PROVENANCE.md` gets an entry per slice (§11.5).

**On the verbatim files.** Appendix A reproduces the Rust sources and
scripts that are load-bearing. Their comments mention identifiers such as
`PR16-C6` or `R2c-mirror`: those are internal cross-references inside the
copied text and need no action. Where a file must differ from the copy,
the delta is listed next to it.

**Not in this plan**: deleting the C sources or the meson build (they
stay as the oracle), porting libweston, any helper client, the deferred
maintenance layer (`docs/maintenance-layer-plan.md` stays a design).

---

## 1. Fixed decisions

### 1.1 Scope, linkage, end state

| ID | Decision |
|---|---|
| D-PORT-1 | Port everything the C `westonite` does after F1–F6, 1:1 in capability: all six remaining backends and their options, multi-backend `--backends`, clone-of and mirror-of, colour management, `[keyboard]`/`[libinput]`, autolaunch, logging and debug scopes, Xwayland, the Super+R recorder. The product drops of the C plan carry over as refusals with the same test wording (§5.4). |
| D-PORT-2 | libweston 14.0.1 stays the engine, consumed from the EPEL RPMs; libwayland stays the event loop. Only the language of *our* layer changes. |
| D-PORT-3 | **Strangler fig, shell first.** R1 ships the Rust shell as a cdylib loaded by the C frontend; R2 replaces the frontend slice by slice with the C frontend as oracle. At every phase boundary a working compositor exists in at least two configurations. |
| D-PORT-4 | **Shell linkage: cdylib during R1–R3, static from R3, hybrid deleted at R4.** The shipped binary links `westonite-shell` statically; `--shell` accepts only the default spelling. The `westonite-shell-plugin` cdylib, the `hybrid-r1` feature and every script and CI leg that exists only for the C-frontend + Rust-shell configuration are transitional: they are exercised by CI on every commit from R1 until the commit that deletes them, which is the first task of R4 (§11.1). They are never dormant and never packaged. The end state has two test configurations (C oracle, Rust), not three. |
| D-PORT-5 | **The C tree stays.** `frontend/`, `desktop-shell/`, `shared/`, the meson files and the C test clients remain in the repository and in CI as the oracle. `VENDOR.md` continues to govern them; `PROVENANCE.md` governs the translated Rust. The RPM stops packaging the C artifacts at R3. |
| D-PORT-6 | Binary name in cargo is `westonite-rs`; the RPM installs it as `%{_bindir}/westonite`. Test scripts refer to the cargo name; users see `westonite`. |
| D-PORT-7 | No custom Wayland protocol code, no wayland-rs, no `wayland-scanner` integration: the trimmed shell creates no globals, and libwayland is reached only through the types embedded in libweston's API. |
| D-PORT-8 | No third-party module loading, no `wet_get_config` contract: the C `wet_module_init` ABI ends with the C frontend. `--modules` and `[core] modules` are refused (same wording as the C plan's F3 test). |

### 1.2 Memory model and fence

| ID | Decision |
|---|---|
| D-MEM-1 | **Handles are generational ids** `(slot, generation)` resolved through a per-kind slot table inside `weston`; a dead or recycled slot resolves to `None`. No `Copy` pointer newtypes, no `Rc`/`Weak`, no branded lifetimes, no shadow state. Live generations start at 1 and skip 0 on wrap. |
| D-MEM-2 | **`Option` everywhere.** Safe crates handle every wrapper result with `let … else` skips; `clippy::unwrap_used` and `expect_used` are denied at crate level with no `allow`. Functions that must yield a value get a documented fallback (§5.6 item 1). |
| D-MEM-3 | **Two-tier dispatch, no interior mutability in safe crates.** The shell is plain data with `&mut self` handlers; the one `RefCell` holding it lives in `weston` and is borrowed once per delivered event. Deferred-tier events carry eagerly captured payload; the sync tier is a closed list (§4.5) with a per-entry non-reentrancy proof in the callback inventory. |
| D-MEM-4 | **Drain at the edge by depth counter**: every trampoline body and every outbound FFI call increments a dispatch depth; whoever returns it to zero drains the queue and then the pending-drop list. **`wl_display_run` is the one outbound call that is never wrapped**; event-loop signal callbacks wrap their own bodies. |
| D-MEM-5 | **Destroy notifications are split**: registry invalidation runs synchronously in the trampoline; the policy reaction is a deferred event. |
| D-MEM-6 | **Grabs**: pinned box, static vtable, grab-local state in the box, sync tier with no app borrow, ended boxes retired to the pending-drop list, back-reference is a `DesktopSurfaceId`. |
| D-MEM-7 | **Panics never cross FFI**: `panic = "unwind"` in all profiles, `catch_unwind` at every trampoline → `weston_log` → `abort()`. The `pre_exec` child side aborts. |
| D-MEM-8 | **Trampolines reach `Ctx` through one thread-local slot**; wrapper types are `!Send + !Sync`; ids are inert data. Safe crates spawn no threads and use no async runtime. |
| D-MEM-9 | **Boundary values**: no `weston_sys` type in any safe-facing API; nullable C returns become `Option`; strings are copied lossily at the fence; int flags become `bitflags`/enums; list traversals surface as snapshots. |
| D-MEM-10 | **Boundary cost**: cache strings on change notifications; no per-event allocation on hot paths; log formatting stays behind the scope-enabled guard; client-controlled text never reaches C as a format string. |
| D-MEM-11 | **Fence rules** (§4.3) are mechanically enforced: dependency-graph check, `forbid(unsafe_code)` presence check, bindings-drift check, `unsafe_op_in_unsafe_fn` + `undocumented_unsafe_blocks` in unsafe crates, the no-panic lints in safe crates, and (from R4) a `cargo public-api` snapshot. |
| D-MEM-12 | **Registration and destroy-listener attachment happen in one wrapper function**, and for outputs **after** `weston_output_enable` succeeds; listener identity is never a lookup key; intra-Rust notification never routes through `wl_signal`. |
| D-MEM-13 | **Listeners on signals embedded in the `weston_compositor` struct are owned by `Compositor`** and detached in `Drop` before `weston_compositor_destroy`; every hand-attached listener (`raw_ptr()` given to a C add-listener API) is followed by `mark_attached()`, and `Drop` asserts it in debug builds. |
| D-MEM-14 | **Teardown parks, never frees**: boxes whose trampoline frame may still be on the stack at compositor destroy (the destroy listener itself, pending drops, active grab boxes) move to a thread-local graveyard that lives until process exit. |

### 1.3 Configuration interface (re-specified)

| ID | Decision |
|---|---|
| D-CFG-1 | One serde `Config` model in `westonite-config`, TOML file `westonite.toml`, kebab-case keys, `deny_unknown_fields` everywhere: an unknown key or a type error is a startup error with a span. Repeated ini `[output]` sections become `[[output]]`. |
| D-CFG-2 | clap CLI with the C flag spellings, plus `-o`/`--set section.key=value` dotted overrides patched into the TOML tree **before** deserialization. Trailing positionals are the autolaunch command and are always watched. Unknown flags are rejected by clap on stderr. |
| D-CFG-3 | Resolution order defaults → file → `-o` → flags into an immutable `Settings`; consumers receive typed slices; nothing re-reads the file or the CLI later; the shell receives a `ShellConfig` and never sees either. |
| D-CFG-4 | Search order `$XDG_CONFIG_HOME/westonite.toml`, then `$HOME/.config/westonite.toml`, then each `$XDG_CONFIG_DIRS` entry (default `/etc/xdg`) **without** a `weston/` subdirectory; `--config`/`--no-config` kept. A legacy `westonite.ini` found where the TOML is expected logs one hint line and is otherwise ignored. No converter, no dual-format fallback. |
| D-CFG-5 | The `WESTON_CONFIG_FILE` export to clients is dropped. libweston-consumed environment variables are untouched. |
| D-CFG-6 | Don't over-model: modelines, XKB names, gbm formats, transform names, ICC paths stay strings or thin enums wrapping the C parse points. Colours are **always base-16**, prefixed or not, in the file and through `-o` (a bare TOML integer is accepted and re-rendered as hex). List keys accept an array or one comma-separated string. `--backend`/`--backends` and `[core] backend`/`backends` are one variable. |
| D-CFG-7 | Where C warns and leaves a bad value inert (`[libinput]` values, `[[output]]` keys that only one backend reads, `icc-profile` without colour management, half a colour-characteristics group), the Rust frontend refuses at startup. Where C's inertness is a runtime device fact (capability gating per libinput device) it stays silent. |
| D-CFG-8 | Every `[[output]]` section is validated at startup, whether or not a head of that name ever appears; an unparseable `mode` is the one non-fatal case (C's `Invalid mode … Using defaults.`). |

### 1.4 Build, packaging, dependencies

| ID | Decision |
|---|---|
| D-BUILD-1 | `bindings.rs` is **committed**, generated by `scripts/regen-bindings.sh` inside the build container with `bindgen-cli 0.72.1` pinned and baked into the image; `build.rs` asserts the recorded `libweston-14` pkg-config version at every build. |
| D-BUILD-2 | Toolchain: EL10 AppStream `rust`/`cargo`/`clippy`/`rustfmt` (rust-toolset), edition 2024. A rustup nightly is used **only** by the ASAN validation leg. |
| D-BUILD-3 | Runtime dependencies: `libc`, `bitflags`, `rustix`, `serde` (+derive), `toml`, `clap`; build: `cc`, `pkg-config`; dev: `tempfile`. Nothing else without an explicit owner approval recorded in `PROVENANCE.md`. Slot table, event queue and small vectors are hand-rolled inside `weston`. No tokio, no wayland-rs, no slotmap, no smallvec. |
| D-BUILD-4 | The RPM builds offline from a `cargo vendor` tarball (`Source1`) with a `.cargo/config.toml` written in `%prep`. From R3 the spec is cargo-only for the shipped artifacts; `%files` is the binary, the session file, `westonite.toml.example`, `COPYING`, `VENDOR.md`, `PROVENANCE.md`. |
| D-BUILD-5 | The meson build stays in the repository for the oracle and for the two C test clients, which the Rust e2e leg still needs. |

### 1.5 Validation

| ID | Decision |
|---|---|
| D-VAL-1 | Sanitizers and stress tests land at R0/R1, not at the end: nightly ASAN for the pure-Rust legs, valgrind memcheck for the hybrid (ASAN cannot instrument the C half) and for every pure-Rust smoke leg, a destroy-storm stress test under valgrind, fake-C-object unit tests for every primitive, registry `debug_assert`s on in CI builds. |
| D-VAL-2 | The shell is unit-tested against a mock `ShellHost` (focus churn, child activation, replacement hunt, teardown ordering); only the deep half gets a mock boundary, the frontend's plumbing does not. |
| D-VAL-3 | The e2e harness gets exactly one new dimension, `WESTONITE_CONFIG_FORMAT` (`ini` for the C binary, `toml` for the Rust one), selected by the runner; tests written in the ini dialect are translated for the TOML binary; interface details that changed by design are mode-gated; everything else asserts the same thing in both modes. |
| D-VAL-4 | The C oracle keeps running in CI until the end of this plan and beyond: the C e2e leg and the C DRM VM leg. The hybrid legs (the C-oracle smoke switch `WESTONITE_C_ORACLE=1`, the hybrid e2e run, the hybrid destroy-storm stress) run from R1 through R3 and are deleted at R4 with the hybrid itself (D-PORT-4). The value of the hybrid ends at a fixed event, not a judgement: the day the RPM ships the Rust binary (R3's gate), after which the cdylib guards nothing that ships and every shell bug is reproducible in the Rust binary and comparable against the C oracle as a separate process. |
| D-VAL-5 | Every `Compositor` outbound entry point that deliberately does **not** take the depth wrap is enumerated in a comment on `run()`; re-derive that list with a body-scoped scan for `extern "C"` functions lacking `with_depth`/`with_ctx`/`guard_ctx`, never with a fixed-size window. |
| D-VAL-6 | The DRM VM leg runs the Rust frontend **first**, then the C oracle, into per-frontend result files. |

---

## 2. Starting point and platform additions

What the C repository already provides (do not rebuild it): the two
container images, the smoke script, the e2e suite with its RFB client and
C test clients, the DRM VM harness, the RPM spec and install test, the CI
workflow with `build-and-test` and `drm-vm` jobs, `VENDOR.md`, the
capability inventories.

Additions this plan makes to the platform:

1. **Build image** (`containers/Containerfile.build`): add
   `rust cargo clippy rustfmt clang-libs clang-devel clang-tools-extra
   valgrind libasan` to the `dnf install` list; add the toolchain marker
   and the pinned bindgen install (Appendix A.20 shows the exact lines).
   `xdpyinfo` and `pipewire` are already present.
2. **Installed headers are the only source of truth.** Everything the
   bindings, the shim and the header-fact table say is verified against
   `/usr/include/libweston-14` from `weston-devel-14.0.1-3.el10_0` inside
   the container, never against the upstream tree. Two facts that differ
   from newer upstream and matter: `weston_curtain_params` has a
   `get_label` **callback** (15.x changed it to `char *label`);
   `weston_windowed_output_api_headless_v2` is the windowed API name.
3. **Header facts the design keys off** (verified at R0, table in
   Appendix A.27): `weston_keyboard`/`weston_touch` have **no**
   `destroy_signal`; `weston_output` users attach to `user_destroy_signal`
   via `weston_output_add_destroy_listener` (the sibling `destroy_signal`
   fires when *disabled*); `weston_desktop_surface` has no signal at all
   (death is the `surface_removed` API callback) and only two user-data
   slots exist in the API; `weston_desktop_api` has 14 entries;
   `weston_pointer_grab_interface` has 7; the renderer enum is AUTO=0,
   NOOP=1, PIXMAN=2, GL=3.
4. **Two libweston facts found only by running**: `weston_compositor_backends_loaded`
   is mandatory before any output is created (it installs the no-op
   colour manager that `weston_output_init` dereferences); and
   `weston_output_lazy_align` is not libweston API but a frontend-local
   static in `main.c` that the port reimplements.
5. **The installed `windowed-output-api.h` does not parse standalone**:
   its static inline uses weston's internal `ARRAY_LENGTH` macro. The
   regen script defines it on the bindgen command line.
6. **Static inlines that need the C shim**: only `wl_list_init`,
   `wl_list_remove`, `wl_list_empty`, `wl_signal_add`. Every
   `weston_*_get_api` inline is replaced by a direct
   `weston_plugin_api_get` call; coordinate math is plain Rust.
7. **`va_list` interfaces stable Rust cannot define**: the
   `weston_log_set_handler` pair; the shim owns them (§4.11).
8. **libinput and libevdev enter through `wrapper.h` directly**:
   libweston only forward-declares `struct libinput_device`, and the
   `[libinput]` port calls `libinput_device_config_*` itself. Because the
   linker drops unreferenced libraries with `--as-needed`, a unit test in
   `weston-sys` takes the address of five symbols to prove the pkg-config
   probes emitted the right `-l` flags.
9. **Xwayland `-listenfd`**: `build.rs` reads `have_listenfd` from
   `xwayland.pc` and exports `weston_sys::XWAYLAND_LISTEN_ARG`
   (`-listenfd`, else the deprecated `-listen`), the same probe the C
   meson build makes.

---

## 3. Final repository layout (additions to the C layout)

```
example-weston-standalone/
├── Cargo.toml  Cargo.lock              # workspace (A.1)
├── PLAN-Rust.md                        # this document
├── PROVENANCE.md                       # Rust module → C source map + slice log (§11.5)
├── westonite.toml.example              # annotated example config (A.25)
├── crates/
│   ├── weston-sys/                     # UNSAFE: bindgen + shim (A.2–A.8)
│   │   ├── Cargo.toml  build.rs  wrapper.h
│   │   ├── shim/shim.h  shim.c  testsupport.h  testsupport.c
│   │   └── src/lib.rs  bindings.rs     # bindings.rs is generated and committed
│   ├── weston/                         # UNSAFE INSIDE, SAFE API: the fence
│   │   ├── Cargo.toml
│   │   ├── examples/r0-smoke.rs
│   │   └── src/lib.rs ids.rs registry.rs listener.rs panic_barrier.rs ctx.rs
│   │           events.rs host.rs compositor.rs output_policy.rs layoutput.rs
│   │           layer.rs curtain.rs desktop.rs grab.rs input_bindings.rs
│   │           shell_init.rs log.rs debug.rs libinput.rs xwayland.rs screenshooter.rs
│   ├── westonite-shell/                # SAFE: shell.c policy, src/lib.rs + src/tests.rs
│   ├── westonite-shell-plugin/         # UNSAFE (cdylib entry point only): desktop-shell.so for the hybrid — R1–R3 only, deleted at R4
│   ├── westonite-config/               # SAFE: model.rs cli.rs overrides.rs resolve.rs
│   ├── westonite-spawn/                # UNSAFE (one audited module): process spawning
│   └── westonite/                      # SAFE: the frontend binary, src/main.rs → westonite-rs
├── docs/
│   ├── callback-inventory.md           # A.26 — the fence's audit surface
│   ├── r0-header-facts.md              # A.27
│   └── config-migration.md             # A.28 — ini → TOML table
├── scripts/
│   ├── regen-bindings.sh  rust-fence-check.sh  rust-smoke.sh  rust-asan-smoke.sh
│   ├── rust-stress-test.sh  rust-e2e-test.sh  rust-shell-install.sh   # rust-shell-install.sh: R1–R3 only
│   ├── valgrind-upstream-vnc.supp
│   └── drm-vm-test.sh                  # gains the `rust` frontend argument
├── rpm/westonite.spec                  # cargo-only %files from R3 (A.21)
├── .github/workflows/ci.yml            # + rust, rust-asan jobs; drm-vm runs both frontends (A.22)
└── tests/e2e/support/compositor.py     # + CONFIG_FORMAT / ini_to_config (A.23)
```

`.gitignore` gains `/target`, `/.cargo-home`, `/test-results`,
`/test-results-drm`.

---

## 4. The memory model and the FFI designs (normative)

This section exists so that ~200 call sites do not each invent an
answer, and so the safe crates never contain one. The code that
implements it is in Appendix A.9–A.16; read the two together.

### 4.1 Ownership taxonomy

Five ownership relationships with C, each with exactly one wrapper shape:

| Kind | Examples | Wrapper shape | Safe-side view |
|---|---|---|---|
| 1. C owns, announces death | outputs, heads, seats, surfaces, views | registry id | id; `Option` on resolve |
| 2. C allocates on request, **we** destroy | the shell's view per desktop surface (`unlink` then `weston_view_destroy`), curtains, log scopes, the recorder | RAII inside `weston`, `Drop` = destroy in the documented order | an id the shell may explicitly drop |
| 3. **We** allocate, C borrows by address | `wl_listener` nodes, grab structs, the `weston_desktop_api` vtable, backend config structs | `Pin<Box<…>>` inside `weston`, never exposed | nothing |
| 4. C struct **embedded by value** in our allocation | `weston_layer` (background and workspace layers) | pinned, address-stable field inside `weston`; never moved after `weston_layer_init` | `LayerId` (internal) |
| 5. C data valid for one callback only | event args, string getters | copied at the fence | owned values |

Death announcement per object, which the registry keys off:

| Object | Death announced by | Handle |
|---|---|---|
| `weston_compositor` | `weston_compositor_add_destroy_listener_once` | singleton owned by `weston` |
| `weston_output` | `user_destroy_signal` via `weston_output_add_destroy_listener` | registry id, registered **after** enable |
| `weston_head` | `destroy_signal` via `weston_head_add_destroy_listener` | registry id |
| `weston_seat`, `weston_surface`, `weston_view` | `destroy_signal` | registry id |
| `weston_desktop_surface` | **no signal** — the `surface_removed` API callback | id, invalidated inside that trampoline |
| `weston_pointer` | `destroy_signal` | never stored — scoped access |
| `weston_keyboard`, `weston_touch` | **none** | never stored — scoped access |
| `weston_layer` | none, we own it | pinned (kind 4) |

Two rules follow. **Input sub-objects are never stored**: the wrapper
re-fetches `weston_seat_get_pointer/touch` from a `SeatId` at each use
inside a call that cannot escape. **The desktop surface's half-dead
window is explicit**: after `surface_removed` the shell's per-surface
state may still be worked on while the C object is gone; the shell
models it as data captured in the event, not a nulled pointer.

Where staleness genuinely comes from, so that checked ids are
load-bearing and not defensive: cleanup callbacks that run while other
state still points at the dying object (the focus-replacement hunt runs
*inside* a destroy handler); our objects deliberately outliving the C
object; libweston destroying things in the middle of our outbound calls
(`weston_seat_break_desktop_grabs` frees other grabs and runs our
handlers before returning); libweston's own deferred destruction (DRM
output destroy); objects that never announce death; and the deferral
window dispatch itself adds (an event drained after its subject died,
where `None` is the designed outcome).

### 4.2 Handles: the slot table (Appendix A.10a–A.10b)

`weston` keeps one `SlotTable<T>` per kind (outputs, heads, seats,
surfaces, desktop surfaces). A slot is `{ptr: Option<NonNull<T>>,
generation: u32}`; ids are `(slot, generation)` newtypes (`OutputId`,
`HeadId`, `SeatId`, `SurfaceId`, `ViewId`, `DesktopSurfaceId`,
`LayerId`, `CurtainId`) that are `Copy + Eq + Ord + Hash + Debug`, have
private fields, no public constructor and no conversions. `resolve` is
an index plus a generation compare; `invalidate_ptr` clears the pointer,
bumps the generation (skipping 0) and frees the slot; `id_of` is a linear
scan for cold paths; `live_ids` is the snapshot. A `test-ids` cargo
feature exposes forged generation-0 ids for the shell's mock tests and
nothing else; generation 0 is never minted.

Stale-resolution policy: the handler shape is

```rust
let Some(surface) = host.surface(id) else { return };
```

A missed cleanup site degrades to skipped work plus one log line instead
of a use-after-free. The rule bans panic as the *stale-reference* policy;
it does not ban `debug_assert!` on invariants that are not resolution
results.

### 4.3 The fence rules (mechanically enforced — `scripts/rust-fence-check.sh`, A.19)

- **Dependency graph**: the safe crates (`westonite-shell`,
  `westonite-config`, `westonite`) must not reach `weston-sys` at all,
  directly or transitively, except through `weston` and
  `westonite-spawn`. Checked on `cargo metadata --locked`'s resolved
  graph. Every workspace member must be classified safe or unsafe in the
  script; an unclassified crate fails the check.
- **`#![forbid(unsafe_code)]`** at line start in every safe crate's root
  and `unsafe_code = "forbid"` in its `Cargo.toml` `[lints.rust]`.
- **Bindings drift**: `regen-bindings.sh` into a scratch file, `diff`
  against the committed one; CI sets `FENCE_CHECK_BINDINGS=1` so the leg
  can never skip.
- **Public-API rule**: `weston`'s and `westonite-spawn`'s public APIs
  contain no raw pointers, no `NonNull`, no `weston_sys::*` types.
  Review-enforced until R4 wires the `cargo public-api` snapshot. One
  exception, R1–R3 only: the `hybrid-r1` feature's `shell_init(*mut
  c_void, …)` bootstrap consumed by the plugin crate; it disappears with
  the R4 deletion, and the snapshot is taken after it.
- **Unsafe hygiene**: `unsafe_op_in_unsafe_fn = "deny"` and
  `clippy::undocumented_unsafe_blocks = "deny"` in `weston`,
  `westonite-shell-plugin`, `westonite-spawn`; every `unsafe` block has a
  `// SAFETY:` comment stating the invariant.
- **No-panic policy**: `clippy::unwrap_used` and `expect_used` denied in
  the three safe crates.
- **Crate classification** (the script's two arrays): safe =
  `westonite-shell westonite-config westonite`; unsafe = `weston-sys
  weston westonite-shell-plugin westonite-spawn`.

### 4.4 `Listener` (Appendix A.11)

One primitive owns the `wl_listener`/`container_of` idiom. `ListenerInner`
is `repr(C)` with the `wl_listener` at **offset 0** inside an
`UnsafeCell`, pinned in a `Box`; a single `extern "C"` trampoline casts
the `wl_listener *` C hands back to the inner, runs inside the panic
barrier, resolves `Ctx` from the thread-local slot, and calls the boxed
`FnMut(&Ctx, *mut c_void)` handler under `with_depth`. Normative details:

- explicit `attach`/`detach`/`is_attached`; `Drop` detaches;
  `wl_list_init`-after-remove is internal to the primitive;
- **oneshot** listeners (destroy signals) remove themselves from the list
  *inside* the emission, before the handler runs, because the owner's
  list head is freed afterwards;
- `raw_ptr()` exists for C APIs that do the `wl_signal_add` themselves
  (`weston_compositor_add_heads_changed_listener`,
  `weston_output_add_destroy_listener`, …) and **must** be followed by
  `mark_attached()`;
- the same listener re-entered synchronously (`try_borrow_mut` fails)
  skips: the C idiom has no answer for that shape either;
- listener identity is never a lookup key; the three C lookups that used
  the notify pointer (`get_shell_seat`, `wet_head_tracker_from_head`,
  `wet_output_from_weston_output`) become registry lookups keyed by the C
  object;
- the shell's own `wl_signal destroy_signal` in `shell_surface` and the
  two grab listeners on it are **not ported**: intra-Rust notification is
  registry invalidation plus explicit fixups in the dispatch path.

One C helper writes `listener->notify` itself and would overwrite the
trampoline: `weston_compositor_add_screenshot_authority` (used by
`--debug`'s allow-all authority). Attach those by hand with
`wsys_wl_signal_add` on `output_capture.ask_auth` and `mark_attached()`.

### 4.5 Dispatch (Appendix A.13)

`Ctx` (a cheap-to-clone `Rc<CtxInner>`) holds: the raw compositor and
display pointers; the loaded backends with their kinds; the DRM
layoutputs; the `[libinput]` config; `use_color_manager`; `init_failed`;
the depth counter, `draining`, `in_sync_handler`, `shutting_down`; the
event `VecDeque`; the pending-drop list; the `Option<Box<dyn ShellApp>>`
app slot; the five slot tables; per-desktop-surface wrapper records;
wrapper-owned destroy listeners keyed by C address; the shell layers,
curtains, `weston_desktop`, active grabs; the autolaunch watch; the
xwayland and screenshooter states. **No borrow of any field is held
across an FFI call or an app-handler call.**

- **Deferred tier** (default): the trampoline captures a small `Event`
  value and `enqueue`s it.
- **Sync tier** (closed list; growing it requires a callback-inventory
  row with a proof): `weston_desktop_api::{surface_added,
  surface_removed, committed, set_parent, move, resize,
  set_xwayland_position, get_position}`; the three input bindings; the
  `seat_created` and `output_created` create halves (the shell creates
  the background curtain **inside** the `output_created` emission);
  `heads_changed`; `output_resized` and the frontend's `output_created`
  mirror listeners; the compositor destroy
  listener; `curtain_get_label`; the log handlers; `spawn_xserver`;
  `configure_device`; grab callbacks.
- **Sync-tier borrow rule**: a sync handler may take the app borrow only
  if its inventory row records a proof that it cannot be invoked while
  the borrow is held (invoked only from client-request dispatch, commit
  processing or event-loop sources — never synchronously from an
  outbound call the app makes). Handlers without a proof answer from
  wrapper-held or callback-local data only.
- `dispatch_sync` takes the app out of its slot, calls it, puts it back.
  If the slot is empty: mid-drain and no-app-yet are quiet requeues; a
  sync handler on the stack outside a drain is a **failed proof** —
  `debug_assert!` (the unit harness stops there), then log and degrade to
  the queue in release.
- `with_depth` increments, runs, decrements; at zero and not already
  draining it drains: pop events until empty (discarding when shutting
  down), each taking the app borrow once, then drops the pending-drop
  list. Events enqueued during a drain run in the same drain.
- At teardown the queue is discarded; the sync invalidation half has
  already run.
- **`Compositor::run` does not wrap `wl_display_run`**, so the loop's
  base depth is zero and each trampoline's own wrap goes 0→1→0. The two
  event-loop signal callbacks (`on_term_signal`, `on_sigchld`) wrap their
  own bodies so one handler is one drain. Label/no-op vtable entries,
  the log sink, `get_position` and the nested `collect` inside
  `client_surfaces` stay unwrapped (D-VAL-5 lists them on `run()`).

**`ShellApp`** is the one entry point (`fn handle(&mut self, host: &Ctx,
event: Event)`); **`ShellHost`** (A.15) is the trait the shell is written
against — queries return `Option<plain data>` or snapshots, commands take
ids and are logged no-ops on stale ids — implemented by `Ctx` and by the
mock in the shell's tests.

### 4.6 C vtables and versioned config structs

`weston_desktop_api` is a `static` filled with `extern "C"` trampolines,
`struct_size` from the bound headers, the four trimmed entries `None`
(`show_window_menu`, `fullscreen_requested`, `maximized_requested`,
`minimized_requested`) so `wm_capabilities` stays empty. The xwayland
API table and every `weston_*_backend_config` builder set
`struct_version`/`struct_size` from the bound headers; the pkg-config
tripwire is what keeps that true across RPM bumps.

### 4.7 Grabs

`PointerGrabInner`/touch inner: `repr(C)`, raw grab struct first in an
`UnsafeCell`, kind, `DesktopSurfaceId`, move delta, resize edges and
start size, `ended`, `PhantomPinned`. Trampolines are sync with no app
borrow; policy consequences (clear the grabbed flag, activate on busy
click) are `Event`s. Ending a grab moves the box to the pending-drop
list and enqueues `GrabEnded`. `wl_fixed_from_double` is mirrored with
the union trick (add `3 << 43` as a double and take the low bits), not
`(d * 256.0) as i32`. The safe side stores only mirrors of the policy.
A busy-grab left click activates the window and starts no move grab
(T7).

### 4.8 Panics, globals, threads

`panic_barrier::guard(what, body)` wraps every trampoline body:
`catch_unwind` → log `westonite: fatal: Rust panic in callback '{what}':
{msg}; aborting` → `abort()`. `wet_shell_init` in the plugin crate has its
own `catch_unwind` backstop returning `-1`. No `static mut` anywhere;
process-global C state (`log_scope`, `cached_tm_mday`) lives in the C
shim as file statics because `weston_log_set_handler` has no user-data
slot. `Ctx::new` debug-asserts there is no second live `Ctx` on the
thread.

### 4.9 Boundary value rules

POD geometry is re-declared (`Rect {x, y, width, height}`,
`OutputInfo {name, geometry}`); `ResizeEdges` and `ActivateFlags` are
`bitflags`; `ActivateTarget` selects the view inside the input frame
(`SurfaceView`, `PointerFocusView(seat)`, `TouchFocusView(seat)`);
strings come back as owned `String` via `to_string_lossy`; every nullable
C return is `Option`.

### 4.10 Iterator invalidation

All C-list traversals exposed to the safe layer are snapshots (`Vec<Id>`
taken while no safe code runs). The C shell's restart-loop in
`desktop_shell_destroy_layer` becomes an ordinary loop over a snapshot
whose dead entries resolve to `None`.

### 4.11 The C shim and logging (Appendix A.5–A.8, A.16)

`shim.c` exports `wsys_wl_list_init/remove/empty`, `wsys_wl_signal_add`,
and `wsys_install_log_handlers(scope)`, which installs C-side
`vlog`/`vlog_continue` through `weston_log_set_handler`. With a scope,
they behave exactly like C's: `vlog` timestamps (`weston_log_timestamp`
with a cached `tm_mday`) and prints into the `"log"` scope when it is
enabled; `vlog_continue` goes straight to `weston_log_scope_vprintf`.
With `NULL` they format into a 1024-byte buffer and call the Rust
`wsys_rust_log_sink(buf, len, cont)`, which appends to the `--log` file
if open, else stderr — the path used by the R0 smoke binary, the unit
harness, and a panic line after teardown (C would drop those lines).

`LogContext` (A.16) is the frontend's RAII owner of the log context, the
`"log"` scope, the `--log` file, the file subscriber (`--logger-scopes`,
default `log`) and the flight recorder (`--flight-rec-scopes`, default
`log,drm-backend`, 5 MiB; an explicitly empty list disables it). It is
created before the first log line, handed to `weston_compositor_create`,
and dropped **after** the display. `Drop` destroys the scope explicitly
(or libweston warns and leaks it), destroys the subscribers, then the
context, then closes the file, then reinstalls the scope-free handlers.
Outbound logging always formats in Rust and passes `%s`.

### 4.12 Bring-up and teardown order (Appendix A.17)

`CompositorBuilder::build()`:

1. refuse an empty backend list; `Ctx::new`; `wl_display_create`; the
   log context is **borrowed** from the frontend and required;
2. `weston_compositor_create(display, log_ctx, NULL, NULL)`;
3. `weston_compositor_set_xkb_rule_names` with C-owned `strdup` copies
   (libweston stores and frees them; free them yourself only when the
   call fails), `kb_repeat_rate` (40), `kb_repeat_delay` (400),
   `vt_switching` (true), `repaint_msec` from `[core] repaint-window`
   (−10..=1000, else log `Invalid repaint_window value in config:`), then
   log `Output repaint window is N ms maximum.`;
   `weston_compositor_load_color_manager` when asked; `multi_backend =
   backends.len() > 1` (load-bearing inside libweston's damage tracking);
4. the `"proto"` scope and `wl_display_add_protocol_logger`
   **unconditionally** (free until subscribed); `--debug` enables the
   debug protocol and the allow-all screenshot authority;
5. **signal sources before any backend loads**: SIGTERM and SIGUSR2 on
   the loop (`on_term_signal` logs `caught signal N` and terminates),
   SIGCHLD on the loop (reap; watched-autolaunch exit terminates; an
   Xwayland pid match relays `xserver_exited`), SIGINT via plain
   `sigaction` whose handler only `raise(SIGUSR2)`s (so gdb still catches
   Ctrl+C), SIGUSR1 blocked process-wide. Backend worker threads (VNC
   spawns neatvnc/aml threads during load) inherit the mask; installing
   later lets a process-directed SIGTERM land on a worker and kill the
   process. A partially failed install removes the sources it did
   create;
6. the sync-tier `heads_changed` listener (policy + DRM runtime captured
   by value, no app borrow), the frontend's `output_created` and
   `output_resized` mirror listeners (Compositor-owned, D-MEM-13);
7. load each backend in list order, passing the **unresolved** renderer
   choice (AUTO stays AUTO: the first backend to reach
   `!compositor->renderer` decides, as in C — resolving it in the
   frontend would hand VNC `NOOP` under `--backends=vnc,headless`);
8. `weston_compositor_backends_loaded` (mandatory);
9. the Super+D flight-recorder dump debug binding when a recorder exists;
10. attach the shell (**before** the flush, so its `output_created`
    listener sees every output), then create the recorder state
    (`screenshooter::create`, frontend-owned);
11. `weston_compositor_flush_heads_changed` under `with_depth`; if
    `init_failed` is set, fail (an output that could not be created,
    configured or enabled is a startup failure, never a quietly short
    compositor);
12. bind the socket **after** the flush (`wl_display_add_socket_auto` or
    the named socket) and log `westonite: wayland socket NAME`;
13. load `xwayland.so` if asked (fatal on failure);
14. `weston_compositor_wake`.

`Compositor::drop`: `shutting_down`; `xwayland::teardown` (the module
must still be live for `xserver_exited`); `screenshooter::teardown`;
detach `heads_changed`; detach every Compositor-owned listener (debug
assert `is_attached`); destroy the protocol logger, then the `proto`
scope; remove the signal sources; `weston_compositor_destroy` under
`with_depth`; `Ctx::teardown` (park, see D-MEM-14); `wl_display_destroy`.
The log context outlives all of it in `main`.

### 4.13 Heads-changed and output registration

One compositor-wide listener replaces C's per-backend ones; it walks the
compositor's head list, skips heads whose backend is not one we loaded,
and for each head takes one of C's three branches: **enable** (not
enabled, connected, non-desktop rules per backend), **disable**
(enabled, not connected), **device changed** (log `Detected a monitor
change on head 'NAME', not bothering to do anything about it.` and reset
the flag). DRM heads route to the layoutput machinery (§9.3); every
other backend takes the simple path: policy lookup → create output →
attach head → configure (mode, scale, transform, per-backend API) →
`weston_output_enable` → **only then** register the output with its
destroy listener → place it (`lazy_align`: to the right of the rightmost
existing output, with the C `int` truncation of the position). A failed
configure or enable destroys the output and sets `init_failed`.

`track_surface`/`untrack_surface` implement the C `focus_state`
listener centrally and ref-counted: `activate_input` returns the focused
`weston_surface`'s id as one acquisition; several seats focusing the same
surface share one destroy listener; the listener emits
`TrackedSurfaceGone {surface, main}` with the main desktop surface
captured eagerly. A per-seat **pointer-destroy guard** detaches the
pointer-focus listener inside `weston_pointer_destroy`'s emission,
because `weston_seat_release` frees the pointer before it emits the seat
destroy signal.

---

## 5. Configuration interface (re-specified)

### 5.1 Model (Appendix A.29, `model.rs`)

`Config { core, shell, keyboard, libinput, autolaunch, xwayland, rdp,
vnc, pipewire, output: Vec<Output>, remote_output: Vec<RemoteOutput>,
pipewire_output: Vec<PipewireOutput>, color_characteristics:
Vec<ColorCharacteristics> }`, every struct `#[serde(rename_all =
"kebab-case", deny_unknown_fields, default)]`. Defaults mirror the C
frontend's. Three custom deserializers: `de_opt_color_string` (string or
non-negative integer re-rendered as `0x%08x`), `de_opt_scalar_string`
(`[libinput] scroll-button` as string or number), `de_string_or_seq`
(list keys as array or comma string).

Keys that exist **only so the refusal can name them** (D-PORT-1, the C
plan's F-series): `[core] modules`, `[core] idle-time`, `[[remote-output]]`,
`[[pipewire-output]]`, `[libinput] touchscreen-calibrator` (alias
`touchscreen_calibrator`), `[libinput] calibration-helper` (alias
`calibration_helper`), `[libinput] enable_tap` (C's deprecated spelling,
kept as a separate field so the error can say `enable-tap`), the `[rdp]`
section and `rdp` in the backend list. `[shell] client` is accepted and
ignored (the shared `test_shell_client_setting_is_ignored` runs in both
modes). Everything else is live.

Additions relative to the ini surface, all documented in
`config-migration.md`: `[vnc] address` (CLI/file symmetry for
`--address`); `[[output]] off = true` as the TOML spelling of
`mode = "off"`; `[shell] cursor-theme`/`cursor-size` (read by the nested
wayland backend); `[[color-characteristics]]` with kebab-case keys
(`max-luminance`, `min-luminance`, `max-fall`, `red-x` …) replacing
C's `max_L`/`min_L`/`maxFALL`. Removed: `vrr-mode` and `max-cll` (read
nowhere in 14.0.1 — they become unknown keys).

### 5.2 CLI (Appendix A.30, `cli.rs`)

clap derive, `name = "westonite"`, `disable_version_flag` (`--version`
is a bool that prints `westonite 14.0.1`). Flags keep C spellings:
`--backend`/`-B` with `visible_alias = "backends"` and
`overrides_with = "backend"` (last occurrence wins, either takes a comma
list); `--renderer`, `--use-gl`, `--use-pixman`; `--socket`/`-S`,
`--log`, `--config`/`-c`, `--no-config`, `-o`/`--set` (repeatable);
`--wait-for-debugger`, `--debug`, `--logger-scopes`,
`--flight-rec-scopes`; `--width`, `--height`, `--scale`, `--transform`,
`--fullscreen`, `--output-count`, `--no-input`, `--sprawl`, `--display`,
`--no-outputs`, `--refresh-rate`; `--seat`, `--drm-device`,
`--additional-devices`, `--current-mode`, `--continue-without-input`;
`--port`, `--address`, `--vnc-tls-cert`, `--vnc-tls-key`,
`--disable-transport-layer-security`; `--xwayland`, `--modules`,
`--shell`; trailing `autolaunch: Vec<String>`
(`trailing_var_arg`, `allow_hyphen_values = false`). The RDP flags
(`--rdp-tls-cert`, `--rdp-tls-key`, `--external-listener-fd`,
`--no-resizeable`, `--rdp4-key`, `--env-socket`, `--no-remotefx-codec`,
`--force-no-compression`) and `--idle-time` are kept **hidden**
(`hide = true`) so their presence produces C's `fatal: unhandled option:
--x` line in the log instead of a clap usage error; a genuinely unknown
flag is a clap error on stderr (mode-gated in the test).

`-o` values parse as TOML when they can (`true`, `40`, `0xff002244`,
`["drm","vnc"]`) and fall back to strings; `output.0.mode=off` indexes
array-of-tables; a rejected override leaves the tree untouched; a
single-segment path is an error.

### 5.3 Resolution (Appendix A.31, `resolve.rs`)

`resolve_from(cli, env)`: discover the file (D-CFG-4) unless
`--no-config`/`--config`; parse to a `toml::Table`; apply overrides;
deserialize (errors carry the file path or `config overrides:`); then
flags on top. Backend list: `--backend` > `[core] backends` > `[core]
backend` > `drm`; each name through `Backend::parse` (`unknown backend
"NAME"` is the C wording); duplicates dropped. Renderer: `--renderer`
conflicts with `--use-gl`/`--use-pixman` and the two conflict with each
other (`Conflicting renderer specifications`); the `[core]` spellings
apply to every backend, not headless only. `--width`/`--height`/`--scale`
and `[[output]] scale` must be positive. Colours through `parse_color`
(`0` allowed; else 8 or 10 characters, optional `0x`, base 16). The
`[vnc]`/`[rdp]` port is selected by the loaded backend. Autolaunch:
positional args beat `[autolaunch] path` and are always watched. The
result is `Settings` (typed scalars plus the full `config` for the
structured sections).

### 5.4 Refusals — `reject_unported` in `main.rs` (Appendix A.32)

Runs after `Settings` exists and before anything is created, in this
order, each a `fatal:` log line and non-zero exit:

1. `rdp` in the backend list → `the rdp backend is not supported by
   westonite (deliberately dropped); use the vnc backend for remote
   access`; a non-empty `[rdp]` section → the same line.
2. `[[remote-output]]` present → `[[remote-output]] is not supported by
   westonite (the remoting plugin is deliberately dropped); use the vnc
   backend for remote access`.
3. `[[pipewire-output]]` present → `[[pipewire-output]] is not supported
   by westonite (the pipewire virtual-output plugin is deliberately
   dropped); the pipewire *backend* (--backend=pipewire) is supported`.
4. The per-backend flag table: a flag whose consuming backend is not
   loaded → `unhandled option: --x (not consumed by any loaded backend)`.
   Rows: `--seat`, `--drm-device`, `--additional-devices`,
   `--current-mode`, `--continue-without-input` (drm); `--scale`
   (headless, x11, wayland); `--transform`, `--no-outputs`,
   `--refresh-rate`, `--use-gl` (headless); `--use-pixman` (headless,
   x11, wayland); `--fullscreen`, `--output-count` (x11, wayland);
   `--no-input` (x11); `--sprawl`, `--display` (wayland); `--port`,
   `--address`, `--vnc-tls-cert`, `--vnc-tls-key`,
   `--disable-transport-layer-security` (vnc). `--width`/`--height` are
   in every C table and never appear.
5. Unconsumed by any shipped backend: the eight hidden RDP flags and
   `--idle-time` → `unhandled option: --x`.
6. `[core] idle-time` set → `idle-time is not supported by westonite
   (displays never sleep)`.
7. `[libinput] touchscreen-calibrator = true` → `[libinput]
   touchscreen-calibrator is not supported by westonite (the touch
   calibration protocol is deliberately dropped; no calibrator client
   ships with it)`; a non-empty `calibration-helper` → `[libinput]
   calibration-helper is only consulted by touchscreen-calibrator, which
   is not supported by westonite`.
8. `[libinput] enable_tap` → `[libinput] enable_tap is C's deprecated
   spelling and is not supported by westonite; spell it enable-tap`.
9. `--modules` / `[core] modules` non-empty → `--modules / [core] modules
   is not supported by westonite (third-party plugin loading is
   deliberately dropped); the shell is built in`.
10. Per `[[output]]` section: `clone-of` without drm → `[[output]] 'N':
    clone-of applies to the drm backend only`; `mirror-of` without vnc →
    `[[output]] 'N': mirror-of requires a remote backend (vnc) in the
    loaded backends`; `mirror-of` on a section not named `vnc` →
    `mirror-of is supported on remote outputs only (the vnc head)`;
    `mirror-of` naming itself → `mirror-of must name a different output`;
    `max-bpc` without drm → `max-bpc applies to the drm backend only`;
    `eotf-mode`/`colorimetry-mode`/`color-characteristics` without drm →
    `eotf-mode, colorimetry-mode and color-characteristics apply to the
    drm backend only`; `icc-profile` without `color-management` →
    `icc-profile requires [core] color-management = true`.
11. `--shell` other than `desktop`/`desktop-shell.so` → `unknown shell
    "X": the Rust frontend ships only the built-in desktop shell`.

Then the typed builders validate what they consume:
`build_output_policy` (`Invalid transform "X"` is fatal; an unparseable
mode logs `Invalid mode for output NAME. Using defaults.`),
`build_input_config` (`not a valid accel-profile`, `accel-speed … out of
range`, `not a valid scroll-method`, `not an evdev button name`,
`scroll-button only applies with scroll-method = "button"`),
`build_color_setup` (`not a valid EOTF mode`, colorimetry likewise, the
entirely-or-not-at-all characteristics group, chromaticities in 0..=1, a
missing `[[color-characteristics]]` block named by an output, `:` in a
name), `RequireOutputs::parse` (`any|all|none`).

### 5.5 Migration (docs only — Appendix A.28)

`westonite.toml.example` (A.25) plus `docs/config-migration.md`, which
carries the ini→TOML table. No converter. The README's configuration
section points at both.

### 5.6 Intentional divergences from the C oracle (the running record)

1. `get_output_work_area` on a stale output: C asserts; Rust logs and
   returns a zeroed rectangle.
2. Reentrant callback shapes run as queued events at the same stack
   boundary instead of nested; must be e2e-invisible.
3. `has_keyboard_focused_child` at child depth ≥ 2: C is UB (a `bool **`
   passed as `bool *`); Rust implements the intent (any transitive
   focused child ⇒ activated), pinned by the unit test
   `activation_redirects_through_grandchild_chain`.
4. Recorder initialisation moves from the shell to the frontend
   (structural; same registration at the same startup point).
5. The stale-resolution policy replaces scattered C null-check sites
   with one central check.
6. A headless mirror **source** works (C aborts on
   `assert(native_mode_copy.width)` because headless publishes no native
   mode); the Rust frontend falls back to the source's current mode.
7. `[core] use-gl`/`use-pixman` apply to every backend, not only headless.
8. The Super+R recorder with no output logs `no output to record` and
   returns (matches the C plan's F5 guard).
9. `handle_display_fd` tears the watch down on EOF where C re-arms a
   level-triggered pipe forever; exactly one watch may exist.
10. `Pong` from a client with no surfaces left ends no busy grab (C
    matches on client identity); the grab self-heals on the next focus.
    Closed at R4 (§11.3).
11. Output move: the curtain moves synchronously in the trampoline, the
    windows on the drain; output resize: the curtain is recreated on the
    drain. Not observable while the drain follows in the same dispatch.
    Closed at R4.
12. Config interface: everything in §5.1–§5.4.
13. `--width`/`--height`/`--scale` of `0` are errors where C silently
    used the default (a `0` scale trips a libweston assert).
14. `$XDG_CONFIG_DIRS` entries are searched without a `weston/`
    subdirectory.
15. Log lines that C would drop (before the log context exists, after it
    is destroyed) reach the `--log` file or stderr.

---

## 6. Testing the port

### 6.1 The harness's TOML mode (Appendix A.23)

`tests/e2e/support/compositor.py` reads `WESTONITE_CONFIG_FORMAT`
(`ini` default, `toml` for the Rust binary) and sets `CONFIG_NAME`
(`westonite.ini`/`westonite.toml`) and `SHELL_READY_PATTERN`
(`Loading module '.*/desktop-shell\.so'` / `westonite-shell: Rust shell
initialized`). `ini_to_config(text)` translates the ini dialect the
tests are written in: `[output]`, `[remote-output]`, `[pipewire-output]`,
`[color_characteristics]` become `[[output]]`, `[[remote-output]]`,
`[[pipewire-output]]`, `[[color-characteristics]]`; `key=value` becomes
`key = value` with `true`/`false`, integers and floats bare and
everything else quoted (backslash and quote escaped). Nothing else in
the harness changes.

Mode-gated asserts in shared tests (exactly these):
`test_unhandled_option_is_fatal` (ini: log `unhandled option:
--bogus-option`; toml: `--bogus-option` on stderr);
`test_autolaunch_config_spawns` (ini: `WESTON_CONFIG_FILE` in the stub's
env; toml: absent). `toml_only = pytest.mark.skipif(CONFIG_FORMAT !=
"toml", …)` marks the Rust-only tests of §12.

### 6.2 Configurations under test, by phase

| Configuration | Binary | Shell | Config | From |
|---|---|---|---|---|
| C oracle | `/usr/bin/westonite` (meson) | C `desktop-shell.so` | ini | exists |
| Hybrid | `/usr/bin/westonite` (meson) | Rust cdylib installed as `desktop-shell.so` | ini | R1, deleted at R4 |
| Rust | `target/release/westonite-rs` | static | toml | R2a |

Scripts, R1 through R3: `smoke-test.sh` runs the hybrid (or the oracle
with `WESTONITE_C_ORACLE=1`); `e2e-test.sh` runs the full suite against
the hybrid; `rust-stress-test.sh` targets the hybrid by default and the
Rust frontend with `WESTONITE_BIN=target/release/westonite-rs`. Scripts
that outlive R4: `smoke-test.sh` and `e2e-test.sh` (plain C oracle again),
`rust-smoke.sh` (nine legs, A.18), `rust-asan-smoke.sh`,
`rust-stress-test.sh` (Rust binary by default; `WESTONITE_BIN` keeps
pointing it at the C oracle for an RPM-side valgrind baseline),
`rust-e2e-test.sh` (Rust binary, TOML mode), `drm-vm-test.sh /results
rust|c`.

### 6.3 Where the oracle cannot run a case

The C oracle aborts on a headless mirror source (§5.6 item 6), cannot
express the TOML-only keys, and refuses nothing the Rust frontend
accepts. Such cases are `toml_only`; their expected behaviour is fixed
by this plan, not by the oracle.

---

## 7. Phase R0 — foundation

### 7.1 Steps

1. Add the Rust packages and the pinned bindgen to
   `containers/Containerfile.build` (A.20); rebuild the image.
2. Create the workspace (A.1) with all seven crate manifests (A.2, A.9
   header block, A.33–A.37). Every manifest is in its final form from
   the start, including the lint tables; crates that do not exist yet
   are added to `members` when their phase lands (R1: shell + plugin;
   R2a: config, spawn, bin).
3. `weston-sys`: `wrapper.h` (A.3), the shim (A.5–A.8), `build.rs`
   (A.4), `src/lib.rs` (A.2). Run `scripts/regen-bindings.sh` (A.19a)
   inside the container and commit `bindings.rs`. Its first three lines
   are the regen marker, `// libweston-modversion: 14.0.1` and
   `// bindgen: bindgen 0.72.1`. The allowlist is `wl_.*`, `WL_.*`,
   `weston.*|WESTON.*`, `wet_.*`, `wsys_.*`, `pixman_.*`, `xkb_.*`,
   `libinput_.*|LIBINPUT_.*`, `libevdev_event_code_from_name`, `EV_KEY`;
   the blocklist removes the four variadic/`va_list` functions
   (`weston_vlog.*`, `weston_log_scope_vprintf`, `weston_log_set_handler`,
   `wl_resource_post_error_vargs`); enums are `moduleconsts` without the
   enum-name prefix.
4. Write `docs/r0-header-facts.md` (A.27) by checking each row against
   `/usr/include/libweston-14` in the container. Do not skip this: two
   of the design's rows are only true for 14.x.
5. `weston` crate primitives, verbatim: `lib.rs` (A.9), `ids.rs`
   (A.10a), `registry.rs` (A.10b), `listener.rs` (A.11),
   `panic_barrier.rs` (A.12), `ctx.rs` (A.13), `events.rs` (A.14),
   `host.rs` (A.15), `log.rs` (A.16). `compositor.rs` at R0 carries:
   `BackendKind`, the option structs, `KeyboardConfig`, `RendererKind`,
   `CompositorError`, `CompositorBuilder` with `headless()`,
   `with_log_context`, `renderer`, `output_size`, `with_socket`,
   `build()` in the order of §4.12 (steps 1–5, 6 without the mirror
   listeners, 7 headless only, 8, 11–12, 14), `install_signal_sources`,
   `run()` unwrapped, `Drop` in the §4.12 order, the three signal
   callbacks, and a canned `heads_changed` that enables every headless
   head at the builder's size and registers the output after enable
   (A.17 is the final form; take from it what R0 needs).
6. `examples/r0-smoke.rs` (A.37).
7. `docs/callback-inventory.md` (A.26) with every row's Status at `—`
   except the R0 rows.
8. `scripts/rust-fence-check.sh` (A.19), `scripts/rust-smoke.sh` legs 1–4
   (A.18), `scripts/rust-asan-smoke.sh` (A.18a) up to its first frontend
   leg, the `rust` and `rust-asan` CI jobs (A.22).

### 7.2 Rules that must hold from the first commit

- `Compositor::run` does not wrap `wl_display_run`; signal callbacks wrap
  their own bodies (D-MEM-4).
- `init_failed` exists and fails `build()` after the flush (§4.12 step 11).
- Outputs are registered **after** `weston_output_enable` succeeds.
- The signal shape is C's (§4.12 step 5) and the sources are installed
  before any backend loads.
- The socket binds after the flush.
- `SlotTable` generations start at 1 and never revisit 0.
- Listeners on compositor-embedded signals are `Compositor`-owned
  (D-MEM-13); `heads_changed` is attached through
  `weston_compositor_add_heads_changed_listener` + `mark_attached()`.
- `Ctx::new` asserts a single live context per thread (debug).
- The shim's `wsys_install_log_handlers(NULL)` fallback is installed by
  `install_stderr_handlers()` before the first log line of any binary
  that has no `LogContext` yet; with a `LogContext` the scope-aware pair
  replaces it and `build()` must **not** reinstall anything.

### 7.3 Gate

Inside the build container:

```sh
cargo fmt --check
cargo clippy --locked --workspace --all-targets --features weston/testsupport -- -D warnings
FENCE_CHECK_BINDINGS=1 scripts/rust-fence-check.sh          # ALL FENCE CHECKS PASSED
scripts/rust-smoke.sh                                       # legs 1-4 (legs 5-9 are added by later phases)
scripts/rust-asan-smoke.sh                                  # ASAN SMOKE PASSED
```

Unit tests that exist at the gate: `registry.rs`
(`stale_id_resolves_none_after_invalidation`,
`recycled_slot_does_not_resurrect_old_id`,
`invalidate_unknown_ptr_is_noop`, `generation_zero_is_never_live`);
`listener.rs` (`emit_reaches_handler_and_multishot_stays_attached`,
`oneshot_self_detaches_inside_emission`,
`drop_detaches_from_live_signal`); `ctx.rs`
(`events_drain_at_depth_zero_only`,
`events_enqueued_during_drain_run_in_same_drain`,
`sync_dispatch_with_borrow_held_outside_drain_is_loud`,
`sync_dispatch_mid_drain_queues_quietly_into_same_drain`,
`sync_dispatch_before_app_installed_queues_quietly`,
`teardown_discards_queued_events`); `weston-sys`
(`libinput_and_libevdev_symbols_resolve`). CI green on both new jobs.

---

## 8. Phase R1 — the shell in Rust, loaded by the C frontend

### 8.1 What lands

- **`westonite-shell`** (A.33 manifest; `src/lib.rs`, `src/tests.rs`):
  `ShellConfig { background_color }` (default `0xff002244`); `Shell {
  config, surfaces: HashMap<DesktopSurfaceId, SurfState>, seats:
  HashMap<SeatId, SeatState>, focus: HashMap<SeatId, SurfaceId>, rng }`
  with `SurfState { parent, children (tail = most recent), focus_count,
  xwayland: Option<(f64, f64)>, last_width, last_height }` and
  `SeatState { focused_surface }`; a deterministic xorshift placement rng
  (bit-parity with C's `random()` is not required). `on_event(&mut self,
  host: &dyn ShellHost, ev: Event)` dispatches every `Event` variant to
  the policy ported from `shell.c` after the trims: surface added/removed
  (the removal walks seats, releases focus acquisitions, fixes up
  children, destroys wrapper state), committed (map on first commit at
  the C initial position — centred-random inside the pointer's output
  work area — then activate on every seat and raise; on resize commits
  apply the edge-dependent offset with C's signs), set_parent, xwayland
  position, ping timeout (busy cursor on every seat whose focus is a
  surface of that client) and pong, activate (via pointer/touch binding
  or busy click: `activate_input` with the right target and flags,
  `set_activated` bookkeeping with focus counts, raise, redirect to the
  last mapped child and through grandchild chains with a cycle bound),
  seat created/gone, output created (`recreate_background`) / gone /
  resized / moved (reposition views by the delta), tracked-surface-gone
  (the focus-replacement hunt over `workspace_views_top_down`), grab
  ended (clear the grabbed mirror), session activated (re-issue input
  activation), shutdown (destroy every surface's state, clear maps).
  `impl weston::ShellApp for Shell` forwards to `on_event`.
- **Fence growth in `weston`**: `desktop.rs` (the static vtable of
  §4.6; `surface_added` creates the view, inserts the registry entries
  and the wrapper record, writes the id into the desktop surface's
  user-data slot, dispatches `SurfaceAdded` sync; `surface_removed`
  dispatches `SurfaceRemoved` sync while the id still resolves, then
  tears the record down and invalidates the slot; `committed` reads
  geometry and the wrapper-held resize edges; `move`/`resize` start the
  grabs from the request's pointer/touch serial; `get_position` answers
  from the view with no app borrow); `grab.rs` (§4.7: move, resize and
  busy pointer grabs, the touch move grab, `set_busy_cursor`,
  `end_busy_grabs_for_client_of`, a guard that no-ops when no `Ctx`
  exists); `curtain.rs` (`weston_curtain_create` per output with the
  sync `get_label` answering from the surface's own output pointer;
  `recreate_background`, `destroy_all_backgrounds`,
  `move_backgrounds_with_output`); `layer.rs` (two pinned
  `weston_layer`s at the C positions, `workspace_layer_ptr`);
  `input_bindings.rs` (BTN_LEFT and BTN_RIGHT click-to-activate with C's
  default-grab guard, touch-to-activate,
  `weston_install_debug_key_binding`); `shell_init.rs` with the
  `hybrid-r1` feature: `shell_init(ec, make_app)` for the plugin
  (`catch_unwind` → log → `false`), `wire_common` (compositor destroy
  listener attached with `weston_compositor_add_destroy_listener_once`,
  transform listener sending xwayland positions, session listener, seat
  create + `register_seat` (destroy, caps-changed, pointer-focus and
  pointer-destroy-guard listeners per seat; caps run once by hand for
  seats that exist already; `dispatch_sync(SeatCreated)`), output
  create/move/resized listeners + `register_output_shell`
  (`dispatch_sync(OutputCreated)` **inside** the emission), the layers,
  `weston_desktop_create`, the bindings, `set_app`), the
  `read_background_color` path that reads `[shell] background-color`
  once through `dlsym(RTLD_DEFAULT, "wet_get_config")` +
  `weston_config_get_section`/`weston_config_section_get_color`
  (hand-declared externs; never retained), and the log line
  `westonite-shell: Rust shell initialized`. The hybrid bootstrap
  **never touches** `weston_log_set_handler` (the C frontend owns the
  log file) and never reads or writes the compositor user-data slot.
- **`Ctx::teardown` parks** (D-MEM-14): the compositor-destroy listener's
  own box, pending drops and active grab boxes go to the graveyard; the
  backends' input teardown cancels grabs through the embedded vtable
  **after** the destroy emission, so those boxes must outlive it.
- **`westonite-shell-plugin`** (A.34 manifest, A.36 `lib.rs`): the
  `#[unsafe(no_mangle)] extern "C" fn wet_shell_init(ec, argc, argv)`
  entry point, returning 0/−1. Transitional (D-PORT-4): lives R1–R3,
  deleted at R4 (§11.1).
- **`scripts/rust-shell-install.sh`** (A.18c); `smoke-test.sh` calls it
  after `ninja install` and asserts the Rust marker (absent under
  `WESTONITE_C_ORACLE=1`); `e2e-test.sh` inherits the same install.
- **`scripts/rust-stress-test.sh`** (A.18b): three destroy storms against
  the hybrid under valgrind, asserting at least one mapped client.
- The RPM spec ships the Rust plugin over the meson-installed
  `desktop-shell.so` (interim: `cargo build --release --offline --locked
  -p westonite-shell-plugin` in `%build`, `install -m 755` in
  `%install`, `Source1` vendor tarball, `rpm-build.sh` runs `cargo vendor
  --locked`). This is the R1–R2 spec; R3 replaces it (A.21).
- CI: `build-and-test` gains the C-oracle smoke step and the stress step
  (A.22); the `rust` job adds `cargo test -p westonite-shell`.

### 8.2 Rules

- Every row L11–L28, B3–B6 and the desktop-api table in the callback
  inventory carries its tier and proof **as implemented** (the create
  halves of L27/L28 are sync). A tier cell that disagrees with the
  dispatch site is a review failure.
- Grab-local state never leaves the box; the shell's `ActiveGrab` mirror
  is policy only.
- Focus tracking is ref-counted through `activate_input` /
  `untrack_surface`.
- `dispatch_sync` outside a drain with the app borrow held is a
  `debug_assert!`.

### 8.3 Gate

`smoke-test.sh` (hybrid) and `WESTONITE_C_ORACLE=1 smoke-test.sh`;
`e2e-test.sh /results` full suite green against the hybrid;
`rust-stress-test.sh` valgrind-clean; `cargo test -p westonite-shell`
green with the nine mock-host tests
(`map_activates_on_every_seat_and_raises`,
`activating_second_surface_deactivates_first`,
`activation_redirects_to_last_mapped_child`,
`focus_replacement_hunt_on_surface_death`,
`shutdown_destroys_every_surface_and_clears_state`,
`activation_redirects_through_grandchild_chain`,
`parent_cycle_activation_terminates`,
`multi_seat_shared_focus_keeps_other_seats_tracking`,
`resize_edge_commit_offsets_match_c_signs`); RPM built and
install-tested with the Rust shell; CI green.

---

## 9. Phase R2 — the frontend in Rust, slice by slice

Each slice ends with the full e2e suite green in **both** the hybrid and
the Rust configuration (the Rust one restricted to the tests its
backends can run, by pytest `-k`/deselection, until R2c finishes — after
R2d the Rust binary runs the entire suite).

### 9.1 R2a — config, spawn, core, logging, refusals

1. **`westonite-config`** (A.35, A.29–A.31): the whole model, CLI,
   overrides and resolution of §5, in one slice. Before writing it,
   perform the **mechanical key audit**: list every
   `weston_config_section_get_*` key and every `WESTON_OPTION_*` entry
   in the C `main.c`, `xwayland.c` and `shell.c` of the oracle tree
   (after F1–F6), and map each to a model field, a CLI flag, or a
   refusal in §5.4. The audit is committed as the R-G checklist in
   `docs/config-migration.md`'s companion table; a key with no row does
   not exist.
2. **`westonite-spawn`** (A.38 manifest; `src/lib.rs`): `real_uid()`,
   `socketpair_stream()`, `pipe_cloexec()`, `Command::{new, from_argv,
   from_exec_string, env, arg, arg_fd, pass_fd, spawn}`. `pre_exec`:
   clear `CLOEXEC` on passed fds, `setsid`, reset the signal mask —
   raw `libc` only, no allocation, no locking; every failure returns
   `Err`. `from_exec_string` parses C's `ENV=x cmd arg` grammar (a key
   containing `/` or an empty key is a program name, documented as
   deliberate).
3. **`westonite` binary** (A.32 for `main.rs`): `Cli::parse`;
   `--version`; `install_stderr_handlers`; `LogContext::new` (fatal on
   error); banner `westonite 14.0.1 (Rust frontend, weston 14 based)`,
   `Command line: …`, `Flight recorder: enabled|disabled`;
   `verify_xdg_runtime_dir` (unset or not a directory: fatal on stderr
   naming `XDG_RUNTIME_DIR`; wrong mode **or owner**: warning);
   `resolve`; `Using config file 'PATH'` or `Starting with no config
   file.`; the legacy-ini warning; `reject_unported`;
   `wait_for_debugger` (log `waiting for debugger, send SIGCONT to
   continue`, `SIGSTOP` self); `build_output_policy`;
   `build_input_config`; the builder chain of §4.12 with `with_shell`
   (static `westonite_shell::Shell`); `start_autolaunch` (config path
   must be executable — `Specified autolaunch path (P) is not
   executable`; `WAYLAND_DISPLAY` set; **no** `WESTON_CONFIG_FILE`;
   `set_autolaunch(pid, watch)`); `run`; drop the compositor before the
   log context.
4. **Fence additions**: `attach_shell_native` (shares `wire_common`
   with the hybrid path; fails `build()` if the compositor already has
   the destroy listener). **Split `shell_init.rs` here**: `wire_common`,
   `attach_shell_native` and everything the native binary uses are
   unconditional; only the dlsym half (`shell_init`, `read_background_color`,
   `create_screenshooter`, the hand-declared config-parser externs) sits
   behind `#[cfg(feature = "hybrid-r1")]`, and the `westonite` binary's
   manifest does **not** enable the feature (delta to A.38). This is what
   makes the R4 deletion a pure removal instead of a refactor. Also
   `with_shell`, `with_socket_name`, `set_autolaunch`, the SIGCHLD source's autolaunch branch
   (`autolaunched client exited, terminating`), `debug.rs` (`enable`
   returns a `Listener` that is `mark_attached`; `protocol_log_fn`
   formats one line Rust-side, `rq`/`ev` prefixes, argument formatting
   per C's `types[]`-by-argument-index quirk kept), `LogContext` fully
   (§4.11), `wait_for_debugger`, `log_child_exit`.
5. **Harness**: `WESTONITE_CONFIG_FORMAT`/`ini_to_config` (§6.1);
   `scripts/rust-e2e-test.sh` (A.18d) — builds the release binary,
   builds the meson test clients, sets up the PAM stack and runs the
   suite as `e2e` with `WESTONITE_BIN=/src/target/release/westonite-rs`
   and `WESTONITE_CONFIG_FORMAT=toml`. CI: the `rust` job runs it.
6. **Tests that land** (§12): the Rust-only `test_cli.py` cases, the
   mode gates, and every refusal of §5.4 running against the Rust
   binary through the shared `test_refusals.py`.
7. `rust-smoke.sh` legs 5, 6 and 9 (headless frontend; autolaunch watch
   under valgrind; `--debug` under valgrind in a debug build);
   `rust-asan-smoke.sh`'s frontend leg; `westonite.toml.example`
   validated by the unit test `example_config_is_valid`.

Gate: `rust-e2e-test.sh` green on the headless-reachable subset
(`-k "not vnc and not xwayland …"` as needed, recorded in the script
until R2c/R2d lift it); hybrid suite unchanged and green; valgrind and
ASAN legs green; `cargo test -p westonite-config` (the resolve/overrides
tests named in A.31's test module) and `-p westonite-spawn` green.

### 9.2 R2b — output policy (headless slice)

`output_policy.rs`: `OutputTransform::parse` (C's names), `OutputRule`
(one per `[[output]]`: name, mode string, scale, transform, off,
clone-of, mirror-of, resizeable, the DRM and colour keys as typed
options), `OutputCliOverrides` (`--width/--height/--scale/--transform`),
`OutputPolicy::{defaults, decide, decide_sized, has_mirror_of,
mirror_rule_for_source}` with precedence backend default → name-matched
section → CLI. `heads_changed` in its final three-branch form (§4.13),
`enable_head` for the windowed backends through
`weston_windowed_output_api_headless_v2` (`weston_plugin_api_get`), the
`off` rule (log `output NAME disabled by config`), `lazy_align` with C's
`int` truncation, `init_failed`. Frontend: `build_output_policy` in
`main.rs` validates every section at startup (D-CFG-8).

Tests: `test_outputs.py` headless subset shared plus the Rust-only
`off` cases; unit tests `precedence_defaults_section_cli`,
`off_rule_disables_only_its_head`, `transform_grammar_matches_c`,
`lazy_align_places_first_output_at_origin`,
`lazy_align_truncates_through_int_like_c`. Gate: both suites green.

### 9.3 R2c — the remaining backends, against the oracle

Land in this order, each green before the next.

**R2c-vnc.** `add_vnc(VncOptions { bind_address, port, refresh_rate_hz,
tls_cert, tls_key, disable_tls })`; `decide_vnc` (mode, resizeable
forced off under mirror-of, transform always normal); multi-backend
loading with `multi_backend` set before load; `[keyboard]` and
`repaint-window` compositor init (§4.12 step 3); the renderer AUTO
passthrough. `rust-smoke.sh` leg 7 (VNC under valgrind with
`valgrind-upstream-vnc.supp`, A.24). After this slice the Rust binary
runs the whole suite except `test_xwayland.py` and the DRM module.

**R2c-mirror.** `mirror_on_output_created` (enable the deferred remote
head when its source appears; guard an already-enabled head; reset the
source head's device-changed flag so the flush stays quiet) and
`mirror_on_output_resized` (reposition the remote over its source,
recompute its modeline; log `Setting modeline to output 'vnc' to WxH,
scale: N`, `Use of mirror_of disables resizing for output vnc`); the
headless-source fallback (§5.6 item 6); the remote-head deferral in
`heads_changed`. Tests: the three Rust-only mirror cases in
`test_outputs.py`.

**R2c-nested.** `add_x11(X11Options { fullscreen, no_input,
output_count })`, `add_wayland(WaylandOptions { display_name,
fullscreen, sprawl, output_count, cursor_theme, cursor_size })`,
`add_pipewire(PipewireOptions { gbm_format, num_outputs })`;
`create_windowed_heads` with `DefaultHeadNumbering::AfterNamed` for x11
(`X-1` + `screen1`) and `FromZero` for wayland (`WL-1` + `wayland0`);
x11's 1024x600 default; `decide_pipewire`. Tests: `test_backends_nested.py`
shared. `test_pipewire_backend_is_still_supported` is the guard that
the F2 refusal cannot widen.

**R2c-color.** `color_management(bool)` loads the manager before
backends; `ColorSetup`/`EotfMode`/`ColorimetryMode`/`ColorCharacteristics`
(`decide_color`, `apply_color` at configure time; `allow-hdcp` on every
backend; `icc-profile` gated on the manager; eotf/colorimetry/
characteristics DRM-only and gated on the manager with C's own error
text). Tests: `test_color_management.py` (nine, Rust-only).

**R2c-input.** `libinput.rs`: `AccelProfile`, `ScrollMethod`,
`ScrollButton` (`libevdev_event_code_from_name`, `EV_KEY`),
`InputConfig` with every `[libinput]` key incl. `disable-while-typing`;
`tramp_configure_device` installed **unconditionally** into
`weston_drm_backend_config.configure_device` (called during load for
present devices and on hotplug; full trampoline shape; config reached
through `CtxInner::input_config`, populated before the load call); per
device log `libinput: configuring device "NAME"` followed by one
ten-space-indented line per applied key in C's order; capability gating
silent. Parsing/validation in `build_input_config` at startup (§5.4).
Unit tests `accel_profile_grammar_matches_c`,
`scroll_method_grammar_matches_c`,
`scroll_button_names_resolve_through_libevdev`,
`default_config_asks_for_nothing`; e2e: the Rust-only `test_cli.py`
libinput cases now, the VM cases with R2c-drm.

**R2c-drm.** `add_drm(DrmOptions { seat_id, specific_device,
additional_devices, gbm_format, pageflip_timeout, use_pixman_shadow,
current_mode, continue_without_input, input })`,
`require_outputs(RequireOutputs::{Any, AllFound, None})`; `layoutput.rs`
(the module doc in A.39 is normative): a layoutput per `[[output]]`
name or head name, heads **staged** on the flush, then
attach-then-enable-with-undo (attach staged heads, try
`weston_output_enable`, on failure detach one head at a time and retry,
losers pushed to the next round; `MAX_CLONE_HEADS` = 16 with C's
one-below guard), `process_layoutputs` after the head loop, the
`require-outputs` verdict, a destroy listener per layoutput output that
nulls the pointer. `decide_drm` **to the letter of C**: no section →
stage under the head's own name with defaults, non-desktop or not;
section with `off` → prune; section without `mode` on a non-desktop head
→ skip (the backend turns it off); `clone-of` resolved through
`drm_controlling_rule` (depth 10; `Configuration error: output section
referred to with 'clone-of=X' not found.` / `'clone-of' nested too deep
for output 'N'.`); `--current-mode` forces `mode=current`; the
`DrmOutputSetup` carries mode (preferred/current/modeline), scale,
transform, gbm-format, pixman-shadow, seat, max-bpc (with the
mode=current interaction), content-type, force-on. Unit tests
`drm_non_desktop_matrix_matches_c`,
`drm_non_desktop_follows_the_controlling_section`.

`scripts/drm-vm-test.sh` gains the `rust` frontend argument (A.23b):
`cargo build --locked --release -p westonite`, copy `westonite-rs` to
the guest's `/usr/local/bin`, `WESTONITE_BIN`/`WESTONITE_CONFIG_FORMAT`
per frontend, per-frontend console and JUnit names. The `drm-vm` CI job
runs `rust` then `c` and is a gate from here on. Tests: the ten
`test_backend_drm.py` cases against the Rust frontend inside the VM.

### 9.4 R2d — Xwayland

`xwayland.rs` (A.40 header is normative): `load` (dlopen
`xwayland.so` from `LIBWESTON_MODULEDIR` through
`weston_compositor_load_xwayland`, `weston_xwayland_get_api`,
`api->listen(xwayland, state, spawn_xserver)`); `spawn_xserver` sync with
a return value (socketpairs via `westonite-spawn`, `-displayfd` pipe,
`XWAYLAND_LISTEN_ARG`, `-wm`, the borrowed `abstract_fd`/`unix_fd`
dup'ed for the child and never closed, `wl_client_create` on the parent
end, log `launching '/usr/bin/Xwayland'`); `handle_display_fd` (on the
newline hand `wm_fd` to `xserver_loaded`; EOF tears the watch down);
the SIGCHLD pid match → `xserver_exited` (respawn is the module's job);
`teardown` calls `xserver_exited` for a still-running server before the
compositor is destroyed. `rust-smoke.sh` leg 8. After this slice the
Rust binary runs the **entire** e2e suite: `rust-e2e-test.sh` drops its
deselection.

### 9.5 R2e — the Super+R recorder

`screenshooter.rs`: `ScreenshooterState { recorder: Cell<*mut
weston_recorder> }`; `create(ctx)` registers the **one** key binding
Super+R (`KEY_R`, modifier SUPER) after the shell attaches; the binding
toggles `weston_recorder_start(output, "capture.wcap")` on the pointer's
focus output, else the first output, else logs `no output to record`
and returns; `weston_recorder_stop` on the second press; `teardown`
stops a running recorder. No Super+S binding, no client slot, no
authority listener (F5). Test: `test_super_r_toggles_wcap_recorder`
shared; `test_super_s_is_inert` and
`test_foreign_capture_client_is_denied` shared.

---

## 10. Phase R3 — ship the Rust binary

1. **`rpm/westonite.spec`** → A.21: `Release: 3%{?dist}`; drop the meson
   BuildRequires and macros; `BuildRequires: cargo rust gcc pkgconfig(libweston-14)
   pkgconfig(wayland-server) pkgconfig(libinput) pkgconfig(libevdev)
   pkgconfig(pixman-1) pkgconfig(xwayland)`; `%build` is `cargo build
   --release --offline --locked -p westonite`; `%install` installs
   `target/release/westonite-rs` as `%{buildroot}%{_bindir}/westonite`,
   `data/westonite.desktop` into `wayland-sessions`,
   `westonite.toml.example` into `%{_datadir}/doc/westonite/`; `%files`
   per D-BUILD-4; `%changelog` entry.
2. **`scripts/rpm-install-test.sh`**: the installed smoke and the
   `-m installed` subset now run the Rust binary, so the pytest
   invocation adds `WESTONITE_CONFIG_FORMAT=toml`; the shell-load assert
   becomes the Rust marker.
3. **CI** (A.22): `build-and-test` keeps building the meson tree and
   running `e2e-test.sh` (hybrid) and the C-oracle smoke through R3 (they
   go at R4, §11.1); the `rust` job is the gate for the shipped artifact;
   the RPM steps move to the `rust` job (or a job that depends on it) so
   the packaged binary is the one the Rust legs tested; `drm-vm` unchanged (both frontends).
4. **Docs**: `README.md` (what the Rust build is; the crate map and the
   fence rules in five lines; configuration: `westonite.toml`, `-o`,
   the migration table link; building: `cargo build --release -p
   westonite`; testing: the three configurations of §6.2); `PROVENANCE.md`
   (§11.5); `docs/config-migration.md` final; the callback inventory's
   Status column complete; `PLAN-Rust.md` = this document.
5. `westonite.ini.example` stays in `data/` for the C oracle but is no
   longer packaged.

Gate: `westonite-14.0.1-3.el10.x86_64.rpm` installs in a pristine
`centos:stream10` container with only `weston-libs` as the compositor
dependency, `rpm -q weston` fails, no `%{_libdir}/westonite` directory
is created, the installed smoke and the eight `installed` tests pass in
TOML mode; CI green on every job.

---

## 11. Phase R4 — hardening

### 11.1 Delete the hybrid (first task; D-PORT-4, D-VAL-4)

The RPM has shipped the Rust binary at R3, so the C-frontend +
Rust-shell configuration guards nothing that ships. Delete it in one
commit, mechanically, and leave nothing dormant:

1. `crates/westonite-shell-plugin/` and its `members` entry in the
   workspace `Cargo.toml`; the `hybrid-r1` feature in
   `crates/weston/Cargo.toml` and the `#[cfg(feature = "hybrid-r1")]`
   half of `shell_init.rs` (the dlsym bootstrap, `read_background_color`,
   `create_screenshooter`, the hand-declared config-parser externs); the
   `weston` dependency of `crates/westonite/Cargo.toml` already carries
   no feature (the R2a split).
2. `scripts/rust-shell-install.sh`; the call to it and the
   `WESTONITE_C_ORACLE` branch in `scripts/smoke-test.sh` (the C smoke is
   plain C again, asserting `Loading module '/usr/lib64/westonite/desktop-shell.so'`
   and the absence of the Rust marker without a switch); the interim
   `cargo build … -p westonite-shell-plugin` and `install` lines are
   already gone from the spec since R3 (A.21); leave the interim release
   in `%changelog`.
3. `scripts/rust-stress-test.sh`: the default `BIN` becomes
   `target/release/westonite-rs`; keep the `WESTONITE_BIN` knob (pointing
   it at `/usr/bin/westonite` stresses the C oracle, a legitimate
   valgrind baseline for RPM-side leaks) and update the header comment.
4. `.github/workflows/ci.yml`: remove the C-oracle smoke step and the
   hybrid stress step from `build-and-test`; add a Rust-binary stress
   step to the `rust` job if one is not already there.
5. `docs/callback-inventory.md`: remove the hybrid notes (the
   `wet_get_config` and compositor user-data remarks); `PROVENANCE.md`:
   the plugin row becomes a log entry recording the deletion;
   `README.md`: no `WESTONITE_C_ORACLE`, two configurations in the
   testing section; §8.2's two hybrid-only rules are retired here.
6. Verify: `grep -rn 'hybrid-r1\|WESTONITE_C_ORACLE\|shell-plugin\|shell_plugin' --exclude-dir=.git --exclude-dir=target --exclude=PLAN-Rust.md .`
   is empty; the fence check's `UNSAFE_CRATES` list drops
   `westonite-shell-plugin`; every CI job green.

### 11.2 Fence and tooling

- Wire `cargo public-api` for `weston` and `westonite-spawn`: a committed
  snapshot per crate, a `rust` job step that diffs it; the snapshot must
  contain no `*mut`, `*const`, `NonNull` or `weston_sys::` token (the
  hybrid entry point is gone by now, §11.1).
- `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"` as
  a CI step (broken intra-doc links are errors).
- `rust-fence-check.sh` fence 1 runs `cargo metadata` with
  `--all-features` in addition to the default graph, and states in its
  header that dev-dependencies count.
- Pin the rustup bootstrap in `rust-asan-smoke.sh` to a checksummed
  installer.

### 11.3 Wrapper-internal debts (each with a unit or e2e test)

- Move the per-seat records and the focus track counts from module
  thread-locals into `CtxInner` so `Ctx::teardown` clears them.
- Replace the XOR-derived `own_listener` keys for compositor-level
  listeners with a two-variant key (`Addr(usize)` / `Slot(u8)`).
- Carry the desktop client identity in `PingTimeout`/`Pong` so a
  surfaceless pong still ends its busy grabs (§5.6 item 10).
- Apply output move/resize to curtain and windows in one step (§5.6
  item 11): make `OutputMoved`/`OutputResized` sync-tier with proofs, or
  move the curtain work into the deferred half.
- Re-derive the `run()` unwrapped-entry-point list (D-VAL-5) and the
  callback-inventory tier cells against the code.

### 11.4 Idiom pass

Clippy `pedantic` triage (allow with a reason or fix; no blanket allow),
SAFETY-comment audit (every `unsafe` block names the invariant that
makes it sound, not what it does), removal of C-shaped code that the
port no longer needs (sentinel returns, int flags inside `weston`,
duplicated teardown closures), and a re-baseline of the public-API
snapshot.

### 11.5 `PROVENANCE.md`

A table mapping every Rust module to its source C file at `14.0.1` (+P0
and the T/F trims) with line ranges, followed by one log entry per
slice: what landed, the validation that passed, and every divergence
the slice added to §5.6. The rebase procedure when EPEL moves: diff the
upstream C between tags, walk the hunks, hand-apply semantic equivalents
to the mapped Rust modules, log per-hunk disposition; regenerate the
bindings; re-verify the header-fact table.

---

## 12. Test inventory — what this plan adds

The C plan's inventory (85 container tests, 10 in the VM) is the shared
base: every one of those tests runs against the C oracle in ini mode
**and** against the Rust binary in TOML mode, with the two mode-gated
asserts of §6.1. The tests below are `toml_only`. Column **lands**: the
slice whose commit adds the test.

**`test_cli.py`**

| Test | Asserts | lands |
|---|---|---|
| `test_legacy_ini_is_hinted_and_ignored` | a `westonite.ini` in `$XDG_CONFIG_HOME`, no `--no-config` → log `found legacy ini config '.*westonite\.ini'`; no `Using config file` | R2a |
| `test_dotted_override_reaches_the_config_tree` | `-o core.backend=bogus --no-config` → non-zero; log `unknown backend "bogus"` | R2a |
| `test_dotted_override_of_a_hex_valued_key` | `-o shell.background-color=0xff336699` → starts; log has no `invalid type` | R2a |
| `test_unknown_config_key_is_fatal` | `[core] bakend = "headless"` → non-zero; log contains `bakend` | R2a |
| `test_deprecated_enable_tap_spelling_names_the_rename` | `[libinput] enable_tap = true` → non-zero; log contains `enable_tap`, `enable-tap` and `not supported by westonite` | R2a |
| `test_libinput_device_settings_without_drm_are_inert` | `[libinput] enable-tap=true` on headless → starts, no `fatal` in the log, exit 0 | R2a |
| `test_libinput_bad_values_are_startup_errors[…]` ×5 | `accel-profile = "swift"` → `not a valid accel-profile`; `accel-speed = 4.0` → `out of range`; `scroll-method = "twofinger"` → `not a valid scroll-method`; `scroll-method = "button"` + `scroll-button = "BTN_NONSENSE"` → `not an evdev button name`; `scroll-button = "BTN_RIGHT"` alone → `only applies with scroll-method`; each non-zero | R2c-input |

**`test_lifecycle.py`**

| Test | Asserts | lands |
|---|---|---|
| `test_socket_binds_after_outputs_are_configured` | in the log, the first `' enabled` precedes `westonite: wayland socket` | R2a |

**`test_outputs.py`**

| Test | Asserts | lands |
|---|---|---|
| `test_output_off_from_config_leaves_head_unenabled` | `[output] name=headless off=true` → log `output headless disabled by config`; no `wl_output` in `wayland-info` | R2b |
| `test_output_off_is_scoped_to_its_own_head` | `[output] name=DP-1 off=true` → the headless output is 1024x640 | R2b |
| `test_vnc_mirrors_headless_output` | `--backends=headless,vnc`, `[output] name=vnc mirror-of=headless` → log `Use of mirror_of disables resizing for output vnc` and `Setting modeline to output 'vnc' to 1024x640, scale: 1`; RFB geometry (1024, 640); no `Detected a monitor change` | R2c-mirror |
| `test_vnc_mirror_defers_until_source_exists` | `--backends=vnc,headless` with the same section → RFB geometry (1024, 640) | R2c-mirror |
| `test_mirror_of_without_remote_backend_is_fatal` | headless only + `mirror-of` → log `mirror-of requires a remote backend`, non-zero | R2c-mirror |

**`test_color_management.py`** (`pytestmark = toml_only`; helper
`fails_with(westonite, config, pattern, backend)` starts with `wait=False`,
waits for the pattern, asserts non-zero exit; `backend="drm"` where the
DRM-only gate would otherwise mask the error — validation runs before
any backend loads, so no VM is needed)

| Test | Asserts | lands |
|---|---|---|
| `test_unknown_eotf_mode_is_refused` | `[output] name=headless eotf-mode=not-a-mode`, drm → `not a valid EOTF mode` | R2c-color |
| `test_unknown_colorimetry_mode_is_refused` | likewise for `colorimetry-mode` | R2c-color |
| `test_eotf_mode_on_a_non_drm_output_is_refused` | `eotf-mode=st2084` on headless → `apply to the drm backend only` | R2c-color |
| `test_icc_profile_without_color_management_is_refused` | `icc-profile=/x.icc` without `color-management` → `requires [core] color-management = true` | R2c-color |
| `test_half_a_characteristics_group_is_refused` | three of six primaries → the entirely-or-not-at-all error | R2c-color |
| `test_out_of_range_chromaticity_is_refused` | `red-x=1.5` → range error | R2c-color |
| `test_missing_characteristics_section_is_refused` | `color-characteristics=nope` with `color-management=true` → the missing-section error | R2c-color |
| `test_allow_hdcp_is_accepted_on_any_backend` | `allow-hdcp=false` on headless → starts, no `fatal` | R2c-color |
| `test_vrr_mode_and_max_cll_are_unknown_keys` | `vrr-mode=game` → `unknown field|vrr-mode` | R2c-color |

**Unit tests** (cargo): the R0 primitives (§7.3), the nine shell tests
(§8.3), `westonite-config` (`example_config_is_valid`,
`defaults_match_c`, `unknown_backend_message_matches_c`,
`file_then_override_then_flag_precedence`,
`unknown_key_is_a_startup_error`, `xdg_discovery_and_legacy_ini_hint`,
`home_fallback_when_no_xdg_config_home`,
`autolaunch_trailing_args_beat_config`,
`positional_command_is_always_watched`,
`hex_color_override_is_not_a_type_error`,
`color_accepts_quoted_and_bare_spellings`,
`color_strings_are_base_16_like_c`,
`color_rejects_what_c_rejects_but_loudly`,
`deprecated_enable_tap_spelling_reaches_the_model`,
`list_keys_accept_comma_strings_and_arrays`,
`backend_and_backends_are_one_variable`,
`xdg_config_home_falls_through_to_home_config`,
`conflicting_renderer_flags_are_rejected`,
`non_positive_geometry_is_rejected`,
`non_positive_output_section_scale_is_rejected`,
`backend_sections_do_not_leak_across_backends`,
`output_sections_pass_through_typed`; overrides:
`scalar_and_typed_values`, `array_of_tables_index`,
`malformed_specs_are_errors`, `rejected_specs_leave_the_tree_untouched`,
`prefixed_integers_stay_typed_for_the_model_to_widen`),
`westonite-spawn` (`exec_string_parses_env_prefix`,
`exec_string_path_with_equals_is_not_env`, `empty_inputs`,
`spawn_runs_with_env_and_clean_session`,
`arg_fd_survives_exec_by_number`, `socketpair_ends_are_connected`), the
`weston` policy tests of §9.2/§9.3, and `main.rs`'s `simple_mode_grammar`.

**Totals.** TOML mode collects the 85 shared tests plus 26 `toml_only`
tests = 111 in the container, of which 1 is the skip-marked VNC resize
test; 10 in the VM. These counts are computed from the inventories, not
measured (Appendix C).

---

## 13. Final acceptance checklist

- [ ] `cargo build --locked --workspace --examples --bins` and
      `cargo build --release -p westonite` succeed in the build image
      with zero warnings under `clippy -D warnings`; `cargo fmt --check`
      clean.
- [ ] `FENCE_CHECK_BINDINGS=1 scripts/rust-fence-check.sh` prints
      `ALL FENCE CHECKS PASSED`; the public-API snapshots match.
- [ ] `scripts/rust-smoke.sh`: all nine legs; `scripts/rust-asan-smoke.sh`:
      `ASAN SMOKE PASSED`.
- [ ] `scripts/rust-stress-test.sh` valgrind-clean against
      `target/release/westonite-rs` (its default after R4).
- [ ] `scripts/rust-e2e-test.sh /results`: 110 passed, 1 skipped.
- [ ] `scripts/e2e-test.sh /results` (the C oracle): the C plan's 84
      passed, 1 skipped.
- [ ] `scripts/drm-vm-test.sh /results rust` and `… c`: `DRMVM-EXIT=0`,
      10 passed each.
- [ ] `rpm/westonite.spec` builds `westonite-14.0.1-3.el10.x86_64.rpm`
      containing `/usr/bin/westonite` (Rust), the session file,
      `westonite.toml.example`, and no `.so`; the pristine install test
      passes in TOML mode.
- [ ] GitHub Actions green on `build-and-test`, `rust`, `rust-asan`,
      `drm-vm`.
- [ ] `grep -rn 'unsafe' crates/westonite-shell crates/westonite-config
      crates/westonite` finds only the `forbid` attributes.
- [ ] `docs/callback-inventory.md` has no `—` in its Status column;
      `docs/r0-header-facts.md` rows all verified; `PROVENANCE.md` has an
      entry per slice; `docs/config-migration.md` and
      `westonite.toml.example` agree with `model.rs`
      (`example_config_is_valid` passes).
- [ ] The C tree still builds and its e2e and DRM legs still pass.
- [ ] No hybrid remains: no `crates/westonite-shell-plugin`, no
      `hybrid-r1` feature, no `scripts/rust-shell-install.sh`, no
      `WESTONITE_C_ORACLE` anywhere outside `PLAN-Rust.md`; exactly two
      test configurations (§6.2).

---

## Appendix A — exact file contents

Reproduced verbatim so they can be copied rather than re-derived. "Copy as-is" means the file is final; "copy, then apply" lists the required deltas. Line ranges refer to the reproduced text.


### A.1 `Cargo.toml` (workspace) — copy as-is

```toml
# Cargo workspace for the Rust migration (docs/rust-migration-plan.md).
# Hybrid phase: this workspace lives alongside the meson tree until R3.
#
# Crate roles and the unsafe/safe fence are normative in the plan §2:
#   weston-sys             UNSAFE  bindgen over the installed libweston 14 headers
#   weston                 UNSAFE INSIDE, SAFE API — the fence crate
#   westonite-shell        SAFE    shell.c policy port (R1)
#   westonite-shell-plugin UNSAFE  R1-only cdylib shim, deleted at R3
#   westonite-config       SAFE    §5 config interface (R2a)
#   westonite-spawn        UNSAFE  process spawning, pre_exec audit (R2a)
#   westonite              SAFE    the frontend binary (R2, growing by slice)

[workspace]
resolver = "2"
members = [
    "crates/weston-sys",
    "crates/weston",
    "crates/westonite-shell",
    "crates/westonite-shell-plugin",
    "crates/westonite-config",
    "crates/westonite-spawn",
    "crates/westonite",
]

[workspace.package]
version = "14.0.1"
edition = "2024"
license = "MIT"
repository = "https://github.com/nhwalker/example-weston-standalone"

[workspace.dependencies]
weston-sys = { path = "crates/weston-sys" }
weston = { path = "crates/weston" }
westonite-shell = { path = "crates/westonite-shell" }
westonite-config = { path = "crates/westonite-config" }
westonite-spawn = { path = "crates/westonite-spawn" }
bitflags = "2"
libc = "0.2"
# D9/D10 approved additions (plan §6 D8): serde/toml/clap for the
# re-specified config interface; rustix for the spawn crate.
serde = { version = "1", features = ["derive"] }
toml = "0.8"
clap = { version = "4", features = ["derive", "wrap_help"] }
rustix = { version = "0.38", features = ["process", "net", "std"] }

[profile.release]
# D16: panics unwind in ALL profiles so the trampoline catch_unwind +
# logged-abort barrier works in release, where it matters.
panic = "unwind"
debug = "line-tables-only"
```

### A.2 `crates/weston-sys/Cargo.toml` and `src/lib.rs` — copy as-is

```toml
[package]
name = "weston-sys"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "UNSAFE: raw bindgen bindings to the installed libweston 14 RPM headers, plus the C shim (static inlines, va_list log interfaces, fake-C-object test support)."
links = "weston-14"

[dependencies]

[build-dependencies]
cc = "1"
pkg-config = "0.3"

[features]
# Compiles the fake-C-object test harness (shim/testsupport.c) so the
# `weston` crate's primitives are unit-testable without a compositor (D18).
testsupport = []
```
```rust
//! UNSAFE: raw FFI surface for libweston 14 (installed EPEL RPM headers).
//!
//! Nothing in this crate is safe to call; it exists so that exactly two
//! crates — `weston` (the fence) and, later, `westonite-spawn` — can reach
//! C.  The safe crates must never depend on this crate (plan §2, enforced
//! by the CI dependency-graph check).
//!
//! `src/bindings.rs` is committed, generated by `scripts/regen-bindings.sh`
//! inside the build container (D5); `build.rs` enforces that the installed
//! libweston version still matches the one the bindings were generated
//! from.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unsafe_op_in_unsafe_fn)] // bindgen output predates the lint
#![allow(clippy::all)]

#[rustfmt::skip] // generated file — must stay byte-identical to the regen script's output
mod bindings;
pub use bindings::*;

/// The Xwayland listen-socket flag (C frontend xwayland.c LISTEN_STR):
/// `-listenfd` when the build-time xwayland.pc advertises
/// `have_listenfd`, the deprecated `-listen` spelling otherwise —
/// build.rs runs the same probe as the C meson.build.
pub const XWAYLAND_LISTEN_ARG: &str = match env!("WSYS_XWAYLAND_LISTENFD").as_bytes() {
    [b'1'] => "-listenfd",
    _ => "-listen",
};

#[cfg(test)]
mod link_tests {
    //! The one thing a bindings commit can get wrong without any
    //! compile error: producing declarations the linker cannot resolve.
    //!
    //! `cargo build` does not catch it, because with `--as-needed` a
    //! library nothing references is dropped from DT_NEEDED entirely —
    //! so weston-sys built clean against libinput/libevdev while the
    //! binary linked against neither.  Taking the addresses forces the
    //! symbols to resolve, which is all these assert: that the
    //! pkg-config probes in build.rs emitted the right `-l` flags.

    #[test]
    fn libinput_and_libevdev_symbols_resolve() {
        let addrs: [*const (); 5] = [
            super::libinput_device_config_tap_get_finger_count as *const (),
            super::libinput_device_config_accel_set_speed as *const (),
            super::libinput_device_config_scroll_set_method as *const (),
            super::libinput_device_get_name as *const (),
            super::libevdev_event_code_from_name as *const (),
        ];
        assert!(addrs.iter().all(|p| !p.is_null()));
    }
}
```

### A.3 `crates/weston-sys/wrapper.h` — copy, then apply

Delta: remove the `#include <libweston/backend-rdp.h>` line (F1: the C build does not probe that header either). Everything else as-is.

```c
/* bindgen input for weston-sys (regenerated by scripts/regen-bindings.sh).
 *
 * Everything here must come from the *installed* EPEL 10 weston-devel
 * 14.0.1 headers (or wayland-devel / pixman headers they pull in) — never
 * from the reference weston tree.  The §3a header-fact verification and
 * the pkg-config version tripwire in build.rs are keyed to these headers.
 */

#include <wayland-server-core.h>

#include <libweston/libweston.h>
#include <libweston/desktop.h>
#include <libweston/shell-utils.h>
#include <libweston/weston-log.h>
#include <libweston/windowed-output-api.h>
#include <libweston/version.h>
#include <libweston/xwayland-api.h>

#include <libweston/backend-drm.h>
#include <libweston/backend-headless.h>
#include <libweston/backend-pipewire.h>
#include <libweston/backend-rdp.h>
#include <libweston/backend-vnc.h>
#include <libweston/backend-wayland.h>
#include <libweston/backend-x11.h>

/* libinput + libevdev: the DRM backend's configure_device hook hands us a
 * `struct libinput_device *` and the frontend applies [libinput] to it
 * (C configure_input_device, main.c:2239).  These are the only two
 * libraries outside weston/wayland the fence talks to, and they enter
 * here rather than through libweston because libweston's headers only
 * forward-declare libinput_device. */
#include <libinput.h>
#include <libevdev/libevdev.h>

#include "shim/shim.h"
#include "shim/testsupport.h"
```

### A.4 `crates/weston-sys/build.rs` — copy as-is

```rust
use std::fs;
use std::path::Path;

fn main() {
    // Resolve the installed RPM headers/libs.  This links libweston-14 and
    // wayland-server for every downstream crate (plan §2).
    let libweston = pkg_config::Config::new()
        .atleast_version("14.0.1")
        .probe("libweston-14")
        .expect(
            "libweston-14.pc not found — build inside the westonite build container (weston-devel)",
        );
    let wayland = pkg_config::Config::new()
        .probe("wayland-server")
        .expect("wayland-server.pc not found");
    // libinput/libevdev: the [libinput] device-config hook (§3, R2c-input).
    // libweston links libinput itself but only forward-declares
    // `struct libinput_device`, so the fence needs the real headers — and
    // the direct link, since we call libinput_device_config_* ourselves.
    let libinput = pkg_config::Config::new()
        .probe("libinput")
        .expect("libinput.pc not found — build inside the westonite build container");
    let libevdev = pkg_config::Config::new()
        .probe("libevdev")
        .expect("libevdev.pc not found — build inside the westonite build container");

    // Version tripwire (plan §6, risk R-C): the committed bindings.rs
    // records the libweston version it was generated from; a pkg-config
    // mismatch means EPEL moved and the regen script must be re-run.
    let bindings = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bindings.rs");
    let text = fs::read_to_string(&bindings)
        .expect("src/bindings.rs missing — run scripts/regen-bindings.sh in the build container");
    let recorded = text
        .lines()
        .find_map(|l| l.strip_prefix("// libweston-modversion: "))
        .expect("bindings.rs lacks the libweston-modversion marker — regenerate it")
        .trim();
    assert_eq!(
        recorded, libweston.version,
        "libweston version skew: bindings.rs was generated against {recorded} but \
         pkg-config reports {}. Run scripts/regen-bindings.sh and re-verify the \
         §3 header facts (docs/rust-migration-plan.md §8).",
        libweston.version
    );

    // Xwayland -listenfd support (this repo's C build, meson.build:111-114;
    // upstream weston carries the same probe in xwayland/meson.build):
    // HAVE_XWAYLAND_LISTENFD comes from xwayland.pc's have_listenfd variable
    // at build time, with "-listen" as the fallback when the pc file is
    // absent (C: `dependency('xwayland', required: false)` not found) or says
    // false.  Exposed to the fence as weston_sys::XWAYLAND_LISTEN_ARG.
    let listenfd = pkg_config::get_variable("xwayland", "have_listenfd")
        .map(|v| v.trim() == "true")
        .unwrap_or(false);
    println!(
        "cargo:rustc-env=WSYS_XWAYLAND_LISTENFD={}",
        if listenfd { "1" } else { "0" }
    );

    // The C shim (§3k) + optional fake-C-object test harness (D18).
    let mut cc = cc::Build::new();
    cc.file("shim/shim.c");
    if std::env::var_os("CARGO_FEATURE_TESTSUPPORT").is_some() {
        cc.file("shim/testsupport.c");
    }
    for inc in libweston
        .include_paths
        .iter()
        .chain(&wayland.include_paths)
        .chain(&libinput.include_paths)
        .chain(&libevdev.include_paths)
    {
        cc.include(inc);
    }
    cc.warnings(true).compile("weston-sys-shim");

    println!("cargo:rerun-if-changed=shim/shim.c");
    println!("cargo:rerun-if-changed=shim/shim.h");
    println!("cargo:rerun-if-changed=shim/testsupport.c");
    println!("cargo:rerun-if-changed=shim/testsupport.h");
    println!("cargo:rerun-if-changed=src/bindings.rs");
}
```

### A.5 `crates/weston-sys/shim/shim.h` — copy as-is

```c
/* C shim for weston-sys (plan §3k).
 *
 * Two jobs:
 *  1. Export callable symbols for the static-inline helpers in the
 *     installed headers that the Rust side actually uses (bindgen cannot
 *     bind static inlines).  The set is grown deliberately, one function
 *     at a time, as the port needs them — not bulk-generated.
 *  2. Own the va_list interfaces stable Rust cannot define: the
 *     weston_log_set_handler pair.  The C side formats into a bounded
 *     buffer and hands Rust a length-delimited UTF-8-unchecked byte
 *     string.
 */
#ifndef WESTON_SYS_SHIM_H
#define WESTON_SYS_SHIM_H

#include <stddef.h>
#include <stdbool.h>

struct wl_list;
struct wl_signal;
struct wl_listener;

/* --- wayland-util static inlines ---------------------------------- */

void wsys_wl_list_init(struct wl_list *list);
void wsys_wl_list_remove(struct wl_list *elm);
bool wsys_wl_list_empty(const struct wl_list *list);

/* --- wayland-server-core static inlines --------------------------- */

void wsys_wl_signal_add(struct wl_signal *signal, struct wl_listener *listener);

/* --- weston-log va_list handlers (§3k) ----------------------------- */

/* Rust-side sink, defined in the `weston` crate:
 * receives one formatted log line (not NUL-terminated, len bytes),
 * `cont` distinguishes vlog_continue.  Returns chars "printed" so the
 * C handler can satisfy weston_log's int return. */
extern int wsys_rust_log_sink(const char *buf, size_t len, bool cont);

struct weston_log_scope;

/* Install wsys-owned vlog/vlog_continue via weston_log_set_handler.
 *
 * `scope` is the "log" scope the frontend created on its log context
 * (C main.c's `log_scope`).  Pass it and the handlers behave exactly
 * like C's vlog/vlog_continue: timestamp, then into the scope, where
 * libweston's subscribers -- the log file, the flight recorder -- pick
 * it up.  Pass NULL and they fall back to formatting straight into
 * wsys_rust_log_sink, which is what the R0 smoke binary, the unit
 * harness and the panic barrier outside the compositor's lifetime
 * need: there is no scope to print into then, and C's own vlog would
 * silently drop the line. */
void wsys_install_log_handlers(struct weston_log_scope *scope);

#endif
```

### A.6 `crates/weston-sys/shim/shim.c` — copy as-is

```c
/* See shim.h for the contract.  Everything here compiles against the
 * installed EPEL headers only. */
#include "shim.h"

#include <stdarg.h>
#include <stdio.h>

#include <wayland-server-core.h>
#include <libweston/libweston.h>
#include <libweston/weston-log.h>

void
wsys_wl_list_init(struct wl_list *list)
{
	wl_list_init(list);
}

void
wsys_wl_list_remove(struct wl_list *elm)
{
	wl_list_remove(elm);
}

bool
wsys_wl_list_empty(const struct wl_list *list)
{
	return wl_list_empty(list);
}

void
wsys_wl_signal_add(struct wl_signal *signal, struct wl_listener *listener)
{
	wl_signal_add(signal, listener);
}

/* --- weston-log va_list handlers ----------------------------------- */

/* One bounded stack buffer per call; long lines are silently truncated
 * (the sink receives only buf/len/cont -- vsnprintf's return value is
 * not forwarded, so truncation is invisible to it; PR16-S5). */

#define WSYS_LOG_BUF 1024

/* C main.c keeps these two as file statics for the same reason: the
 * weston_log_set_handler signature has no user-data slot. */
static struct weston_log_scope *wsys_log_scope;
static int wsys_cached_tm_mday = -1;

static int
wsys_sink(const char *fmt, va_list ap, bool cont)
{
	char buf[WSYS_LOG_BUF];
	int n = vsnprintf(buf, sizeof buf, fmt, ap);

	if (n < 0)
		return 0;
	return wsys_rust_log_sink(buf, (size_t)(n < WSYS_LOG_BUF ? n : WSYS_LOG_BUF - 1),
				  cont);
}

/* C main.c vlog (214): timestamp the line and print it into the scope.
 * The is_enabled check is C's -- with no subscriber on "log" the line
 * is dropped rather than formatted, which is exactly what makes
 * `--logger-scopes=drm-backend` remove ordinary log output. */
static int
wsys_vlog(const char *fmt, va_list ap)
{
	char timestr[128];
	char buf[WSYS_LOG_BUF];
	int n;

	if (!wsys_log_scope)
		return wsys_sink(fmt, ap, false);

	if (!weston_log_scope_is_enabled(wsys_log_scope))
		return 0;

	n = vsnprintf(buf, sizeof buf, fmt, ap);
	if (n < 0)
		return weston_log_scope_printf(wsys_log_scope, "%s %s",
					       weston_log_timestamp(timestr,
								    sizeof timestr,
								    &wsys_cached_tm_mday),
					       "Out of memory");
	return weston_log_scope_printf(wsys_log_scope, "%s %s",
				       weston_log_timestamp(timestr, sizeof timestr,
							    &wsys_cached_tm_mday),
				       buf);
}

/* C main.c vlog_continue (241): no timestamp, straight into the scope
 * -- this is how multi-line weston_log_continue blocks stay attached to
 * the line above them. */
static int
wsys_vlog_continue(const char *fmt, va_list ap)
{
	if (!wsys_log_scope)
		return wsys_sink(fmt, ap, true);

	return weston_log_scope_vprintf(wsys_log_scope, fmt, ap);
}

void
wsys_install_log_handlers(struct weston_log_scope *scope)
{
	wsys_log_scope = scope;
	weston_log_set_handler(wsys_vlog, wsys_vlog_continue);
}
```

### A.7 `crates/weston-sys/shim/testsupport.h` — copy as-is

```c
/* Fake-C-object test support (D18), compiled only under the
 * `testsupport` cargo feature.  Provides just enough real libwayland
 * machinery — wl_signal emission against real wl_list links — that the
 * `weston` crate's Listener / registry / dispatch primitives are
 * exercisable in `cargo test` without a compositor. */
#ifndef WESTON_SYS_TESTSUPPORT_H
#define WESTON_SYS_TESTSUPPORT_H

struct wl_signal;

/* A heap-allocated wl_signal the test controls. */
struct wl_signal *wsys_test_signal_create(void);
void wsys_test_signal_emit(struct wl_signal *signal, void *data);
void wsys_test_signal_destroy(struct wl_signal *signal);

#endif
```

### A.8 `crates/weston-sys/shim/testsupport.c` — copy as-is

```c
#include "testsupport.h"

#include <stdlib.h>

#include <wayland-server-core.h>

struct wl_signal *
wsys_test_signal_create(void)
{
	struct wl_signal *s = calloc(1, sizeof *s);

	if (s)
		wl_signal_init(s);
	return s;
}

void
wsys_test_signal_emit(struct wl_signal *signal, void *data)
{
	wl_signal_emit(signal, data);
}

void
wsys_test_signal_destroy(struct wl_signal *signal)
{
	free(signal);
}
```

### A.9 `crates/weston/Cargo.toml` and `src/lib.rs` — copy as-is

```toml
[package]
name = "weston"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "The fence crate (plan §2/§3): safe, sound wrappers over libweston 14. All unsafe and all unsoundness in the westonite stack live here (and in westonite-spawn); the public API is sound for arbitrary safe callers."

[dependencies]
weston-sys = { workspace = true }
bitflags = { workspace = true }
libc = { workspace = true }
westonite-spawn = { workspace = true }

[features]
# Pulls in the fake-C-object shim so primitives are testable without a
# compositor (D18).
testsupport = ["weston-sys/testsupport"]
# R1 hybrid phase: the shell bootstrap that reads westonite.ini through
# the C frontend and dlsym-resolves the two libexec contract symbols
# (hand-declared externs in shell_init.rs — weston-sys carries no
# hybrid-only surface).  Deleted at R3 (plan §2).
hybrid-r1 = []
# Test-only opaque-id forgery for mock-ShellHost unit tests (D20).
test-ids = []

[dev-dependencies]
weston-sys = { workspace = true, features = ["testsupport"] }

[lints.rust]
unsafe_op_in_unsafe_fn = "deny"

[lints.clippy]
undocumented_unsafe_blocks = "deny"
```
```rust
//! The fence crate (docs/rust-migration-plan.md §2, §3 — normative).
//!
//! Everything unsafe about talking to libweston lives behind this crate's
//! public API, and that API must be *sound for arbitrary safe callers*:
//! a `#![forbid(unsafe_code)]` crate holding stale handles, storing them
//! forever, or misusing the API in any order must not be able to cause
//! undefined behavior (plan goal 1).
//!
//! Load-bearing internal rules (risk R-I) — reviewers hold changes to
//! this crate against these:
//!
//! * Stored cross-references are **generational ids** resolved through
//!   the registry; resolution of a dead or recycled slot yields `None`
//!   (§3b).  Registration and destroy-listener attachment happen in one
//!   function, so no object is ever registered without its invalidator.
//! * Kind-3/kind-4 allocations (listener inners, grab inners, vtables,
//!   embedded `weston_layer`s) live behind `Pin<Box<_>>` created here;
//!   safe crates cannot name the types (§3a).
//! * Dispatch is two-tier with a depth-counted drain-at-edge (§3e):
//!   trampolines and outbound FFI calls increment the depth; whoever
//!   returns it to zero drains the deferred-event queue and then the
//!   pending-drop list.  The app state is borrowed once per delivered
//!   event, never across FFI.
//! * Every trampoline body runs inside the panic barrier: panics are
//!   caught, logged via weston_log, and turned into `abort()` — no
//!   unwinding crosses the C boundary (D16).
//! * Wrapper types are `!Send + !Sync`; ids are inert data, but resolving
//!   one requires `&Ctx`, which never leaves the libweston thread (§3j).

/// The one `container_of` helper (wrapper-internal; §3c keeps the idiom
/// in exactly one place per shape — this macro serves the read-only
/// intrusive-list walks).
macro_rules! container_of {
    ($ptr:expr, $T:ty, $field:ident) => {
        $ptr.cast::<u8>()
            .sub(core::mem::offset_of!($T, $field))
            .cast::<$T>()
    };
}
pub(crate) use container_of;

pub mod compositor;
pub mod events;
pub mod host;
pub mod ids;
pub mod libinput;
pub mod log;
pub mod output_policy;

pub(crate) mod ctx;
pub(crate) mod curtain;
pub(crate) mod debug;
pub(crate) mod desktop;
pub(crate) mod grab;
pub(crate) mod input_bindings;
pub(crate) mod layer;
pub(crate) mod layoutput;
pub(crate) mod listener;
pub(crate) mod panic_barrier;
pub(crate) mod registry;
pub(crate) mod screenshooter;
pub(crate) mod xwayland;

#[cfg(feature = "hybrid-r1")]
pub mod shell_init;

pub use compositor::{
    BackendKind, Compositor, CompositorBuilder, CompositorError, DrmOptions, HeadlessOptions,
    KeyboardConfig, PipewireOptions, RendererKind, VncOptions, WaylandOptions, X11Options,
};
pub use ctx::{Ctx, ShellApp};
pub use events::{ActivateVia, Event};
pub use host::{ActivateFlags, ActivateTarget, OutputInfo, Rect, ResizeEdges, ShellHost};
pub use ids::{CurtainId, DesktopSurfaceId, HeadId, LayerId, OutputId, SeatId, SurfaceId, ViewId};
pub use layoutput::RequireOutputs;
pub use libinput::{AccelProfile, InputConfig, ScrollButton, ScrollMethod};
pub use log::wait_for_debugger;
pub use output_policy::{ColorCharacteristics, ColorSetup, ColorimetryMode, EotfMode};
pub use output_policy::{
    OutputCliOverrides, OutputPolicy, OutputRule, OutputSetup, OutputTransform, VncOutputSetup,
};
```

### A.10a `crates/weston/src/ids.rs` — copy as-is

```rust
//! Opaque generational handles (plan §3b, D13).
//!
//! An id is `(slot, generation)` into the per-kind registry table — no
//! pointer, no public constructor, no conversions (§2 "opaque ids" fence
//! rule).  Ids are freely `Copy`/comparable/hashable and may outlive the
//! object; resolution through the registry is the only way to reach the
//! underlying C object, and a stale id resolves to `None`.

/// Internal representation shared by all id kinds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct RawId {
    pub(crate) slot: u32,
    pub(crate) generation: u32,
}

macro_rules! define_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub(crate) RawId);
    };
}

define_id!(
    /// A `weston_output` (kind 1: C owns, announces death).
    OutputId
);
define_id!(
    /// A `weston_head` (kind 1).
    HeadId
);
define_id!(
    /// A `weston_seat` (kind 1).
    SeatId
);
define_id!(
    /// A `weston_surface` (kind 1).
    SurfaceId
);
define_id!(
    /// A `weston_view` (kind 2 when shell-created: we destroy it).
    ViewId
);
define_id!(
    /// A `weston_desktop_surface` (kind 1, but death is announced by the
    /// `surface_removed` desktop-api callback, not a signal — §3a).
    DesktopSurfaceId
);
define_id!(
    /// A `weston_layer` we own, pinned inside the wrapper (kind 4).
    LayerId
);
define_id!(
    /// A `weston_curtain` (kind 2: we destroy it).
    CurtainId
);

/// Test-only id forgery, for the safe crates' mock-`ShellHost` unit
/// tests (D20).  Feature-gated so production dependency graphs cannot
/// enable it — the opaque-id fence rule (§2) holds outside tests.
/// Forged ids carry generation 0, which `SlotTable` never mints (live
/// generations start at 1 — see `registry.rs`), so a forged id used
/// against the real wrapper always resolves `None`.
#[cfg(feature = "test-ids")]
#[doc(hidden)]
pub mod forge {
    use super::*;

    fn raw(n: u32) -> RawId {
        RawId {
            slot: n,
            generation: 0,
        }
    }
    pub fn ds(n: u32) -> DesktopSurfaceId {
        DesktopSurfaceId(raw(n))
    }
    pub fn seat(n: u32) -> SeatId {
        SeatId(raw(n))
    }
    pub fn output(n: u32) -> OutputId {
        OutputId(raw(n))
    }
    pub fn surf(n: u32) -> SurfaceId {
        SurfaceId(raw(n))
    }
}
```

### A.10b `crates/weston/src/registry.rs` — copy as-is

```rust
//! The per-kind slot table behind generational ids (plan §3b).
//!
//! One `SlotTable<T>` per object kind.  A slot holds the raw pointer and
//! the generation ids were minted with; invalidation clears the pointer
//! and bumps the generation, permanently staling every id minted for the
//! old occupant.  Live generations start at 1 and never revisit 0, so a
//! generation-0 id (the `test-ids` forge, a zeroed id) can never alias a
//! live entry.  The reverse direction (pointer → id) is a linear scan —
//! cold paths only (§3b); per-object user-data slots are a private
//! optimization added where profiling demands it.

use core::ptr::NonNull;

use crate::ids::RawId;

pub(crate) struct SlotTable<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

struct Slot<T> {
    ptr: Option<NonNull<T>>,
    generation: u32,
}

impl<T> Default for SlotTable<T> {
    fn default() -> Self {
        SlotTable {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> SlotTable<T> {
    /// Register a live C object; returns the id all cross-references use.
    ///
    /// Callers in `ctx.rs` pair every `insert` with the attachment of the
    /// destroy listener that will `invalidate` this slot — in the same
    /// function, so the table never holds an object without its
    /// invalidator (§3b).
    pub(crate) fn insert(&mut self, ptr: NonNull<T>) -> RawId {
        if let Some(slot) = self.free.pop() {
            let s = &mut self.slots[slot as usize];
            debug_assert!(s.ptr.is_none());
            s.ptr = Some(ptr);
            RawId {
                slot,
                generation: s.generation,
            }
        } else {
            let slot = u32::try_from(self.slots.len()).expect("slot table overflow");
            // Generation 0 is never minted (module doc): fresh slots
            // start live at 1.
            self.slots.push(Slot {
                ptr: Some(ptr),
                generation: 1,
            });
            RawId {
                slot,
                generation: 1,
            }
        }
    }

    /// Resolve an id to the live pointer, or `None` if the object died
    /// (or the slot was recycled for a new object).
    pub(crate) fn resolve(&self, id: RawId) -> Option<NonNull<T>> {
        let s = self.slots.get(id.slot as usize)?;
        if s.generation != id.generation {
            return None;
        }
        s.ptr
    }

    /// Invalidate the slot holding `ptr` (called synchronously from the
    /// object's destroy trampoline — §3e "sync invalidation half").
    /// Returns the staled id so the caller can build the deferred policy
    /// event.  A miss is a no-op: destroy signals can fire for objects
    /// we never registered.
    pub(crate) fn invalidate_ptr(&mut self, ptr: NonNull<T>) -> Option<RawId> {
        let idx = self.slots.iter().position(|s| s.ptr == Some(ptr))?;
        let s = &mut self.slots[idx];
        let staled = RawId {
            slot: idx as u32,
            generation: s.generation,
        };
        s.ptr = None;
        s.generation = s.generation.wrapping_add(1);
        if s.generation == 0 {
            // Keep the "generation 0 is never live" invariant even
            // across a (theoretical) u32 wrap.
            s.generation = 1;
        }
        self.free.push(staled.slot);
        Some(staled)
    }

    /// Pointer → id for a live object (cold paths: hotplug, trampolines).
    pub(crate) fn id_of(&self, ptr: NonNull<T>) -> Option<RawId> {
        self.slots.iter().enumerate().find_map(|(i, s)| {
            (s.ptr == Some(ptr)).then_some(RawId {
                slot: i as u32,
                generation: s.generation,
            })
        })
    }

    /// Snapshot of all live ids (§3l: C-list traversals surface as
    /// snapshots; this is the registry-side equivalent).
    pub(crate) fn live_ids(&self) -> Vec<RawId> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.ptr.is_some())
            .map(|(i, s)| RawId {
                slot: i as u32,
                generation: s.generation,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: usize) -> NonNull<u8> {
        NonNull::new(v as *mut u8).unwrap()
    }

    #[test]
    fn stale_id_resolves_none_after_invalidation() {
        let mut t = SlotTable::<u8>::default();
        let id = t.insert(p(0x1000));
        assert!(t.resolve(id).is_some());
        t.invalidate_ptr(p(0x1000)).unwrap();
        assert!(t.resolve(id).is_none());
    }

    #[test]
    fn recycled_slot_does_not_resurrect_old_id() {
        let mut t = SlotTable::<u8>::default();
        let old = t.insert(p(0x1000));
        t.invalidate_ptr(p(0x1000)).unwrap();
        // Same address recycled for a brand-new object — the C hazard the
        // generational design exists for (§3b).
        let new = t.insert(p(0x1000));
        assert_ne!(old, new);
        assert!(t.resolve(old).is_none());
        assert_eq!(t.resolve(new), Some(p(0x1000)));
    }

    #[test]
    fn invalidate_unknown_ptr_is_noop() {
        let mut t = SlotTable::<u8>::default();
        assert!(t.invalidate_ptr(p(0x2000)).is_none());
    }

    #[test]
    fn generation_zero_is_never_live() {
        // The test-ids forge (and any zeroed id) uses generation 0; it
        // must never alias a real entry, fresh or recycled.
        let mut t = SlotTable::<u8>::default();
        let id = t.insert(p(0x1000));
        assert_ne!(id.generation, 0);
        let forged = RawId {
            slot: id.slot,
            generation: 0,
        };
        assert!(t.resolve(forged).is_none());
        t.invalidate_ptr(p(0x1000)).unwrap();
        let recycled = t.insert(p(0x3000));
        assert_ne!(recycled.generation, 0);
        assert!(t.resolve(forged).is_none());
    }
}
```

### A.11 `crates/weston/src/listener.rs` — copy as-is

```rust
//! `Listener`: the `wl_listener`/`container_of` primitive (plan §3c).
//!
//! One trampoline owns the pervasive C idiom: the `wl_listener` is
//! embedded at offset 0 of a pinned, wrapper-private `ListenerInner`
//! (kind-3 allocation, §3a), so the trampoline's cast *is* the one
//! `container_of` in the design.  Attach/detach are explicit;
//! `Drop` detaches; listener identity is never a lookup key (§3c).

use std::cell::{Cell, RefCell, UnsafeCell};
use std::ffi::c_void;
use std::marker::PhantomPinned;
use std::pin::Pin;

use crate::ctx::Ctx;
use crate::panic_barrier;

/// Handler contract: runs inside the panic barrier at +1 dispatch depth.
/// `data` is the raw signal argument, valid only for this call (§3a
/// kind 5) — handlers copy what they need and never store it.
pub(crate) type Handler = Box<dyn FnMut(&Ctx, *mut c_void)>;

#[repr(C)]
pub(crate) struct ListenerInner {
    /// MUST remain the first field: the trampoline recovers
    /// `ListenerInner` by casting the `wl_listener *` C hands back.
    /// `UnsafeCell`: both C (list surgery during signal add/emit) and
    /// the wrapper mutate the node through shared references.  The
    /// offset-0 cast is valid because this struct is `repr(C)` with
    /// `raw` first; `UnsafeCell` being `repr(transparent)` only keeps
    /// the field's own layout identical to `wl_listener` (PR16-S3).
    raw: UnsafeCell<weston_sys::wl_listener>,
    attached: Cell<bool>,
    /// One-shot listeners (destroy signals) self-detach inside the
    /// emission — after it, the signal owner's list head is gone and a
    /// later `wl_list_remove` would touch freed memory.
    oneshot: bool,
    what: &'static str,
    handler: RefCell<Handler>,
    _pin: PhantomPinned,
}

impl ListenerInner {
    fn link_ptr(&self) -> *mut weston_sys::wl_list {
        // SAFETY: projecting a raw field pointer out of the UnsafeCell;
        // no reference is formed.
        unsafe { &raw mut (*self.raw.get()).link }
    }
}

unsafe extern "C" fn trampoline(listener: *mut weston_sys::wl_listener, data: *mut c_void) {
    // SAFETY: C hands back exactly the pointer we attached — offset 0 of
    // the pinned, still-live `ListenerInner` (detach-before-drop
    // invariant), so the cast recovers the allocation.
    let inner: &ListenerInner = unsafe { &*listener.cast::<ListenerInner>() };
    panic_barrier::guard(inner.what, || {
        if inner.oneshot && inner.attached.get() {
            // Removing the current node during wl_signal_emit is safe
            // (emit iterates with the safe variant); afterwards the
            // dying owner's list must never see this node again.
            // SAFETY: the link is a live list node while attached.
            unsafe {
                weston_sys::wsys_wl_list_remove(inner.link_ptr());
                weston_sys::wsys_wl_list_init(inner.link_ptr());
            }
            inner.attached.set(false);
        }
        let Some(ctx) = Ctx::current() else {
            // A callback with no live Ctx is a wrapper teardown-ordering
            // bug (§3j): loud in debug, benign no-op in release.
            debug_assert!(false, "listener fired with no live Ctx");
            return;
        };
        ctx.with_depth(|| {
            // try_borrow: the same listener re-entered synchronously
            // (emit within emit) would double-borrow the FnMut; skip —
            // the C idiom has no answer for that shape either.
            if let Ok(mut h) = inner.handler.try_borrow_mut() {
                h(&ctx, data);
            }
        });
    });
}

pub(crate) struct Listener {
    inner: Pin<Box<ListenerInner>>,
}

impl Listener {
    pub(crate) fn new(what: &'static str, oneshot: bool, handler: Handler) -> Listener {
        // SAFETY: wl_listener is POD; a zeroed value is valid storage.
        // The link is properly initialized right below, before any use.
        let mut raw: weston_sys::wl_listener = unsafe { std::mem::zeroed() };
        raw.notify = Some(trampoline);
        let inner = Box::pin(ListenerInner {
            raw: UnsafeCell::new(raw),
            attached: Cell::new(false),
            oneshot,
            what,
            handler: RefCell::new(handler),
            _pin: PhantomPinned,
        });
        // SAFETY: initializing the pinned node's own link; the address
        // is stable from here on (Pin<Box>).
        unsafe {
            weston_sys::wsys_wl_list_init(inner.link_ptr());
        }
        Listener { inner }
    }

    /// Attach to a signal.  Caller (wrapper code only) guarantees the
    /// signal outlives the attachment or announces its owner's death
    /// through this very listener (oneshot destroy pattern).
    #[allow(dead_code)] // R0: used by tests; wl_signal attach sites arrive at R1
    pub(crate) unsafe fn attach(&self, signal: *mut weston_sys::wl_signal) {
        debug_assert!(!self.inner.attached.get(), "double attach");
        // SAFETY: caller contract (live signal); the node is initialized
        // and currently detached.
        unsafe {
            weston_sys::wsys_wl_signal_add(signal, self.inner.raw.get());
        }
        self.inner.attached.set(true);
    }

    pub(crate) fn detach(&self) {
        if self.inner.attached.get() {
            // SAFETY: attached ⇒ the node is on a live list (the oneshot
            // path already cleared `attached` before the owner died).
            unsafe {
                weston_sys::wsys_wl_list_remove(self.inner.link_ptr());
                weston_sys::wsys_wl_list_init(self.inner.link_ptr());
            }
            self.inner.attached.set(false);
        }
    }

    /// The C-facing node, for registration APIs that take the
    /// `wl_listener *` directly (wrapper code only).  Callers that hand
    /// this to a C add-listener API MUST call [`Listener::mark_attached`]
    /// right after, or detach-on-drop will skip the node.
    pub(crate) fn raw_ptr(&self) -> *mut weston_sys::wl_listener {
        self.inner.raw.get()
    }

    /// Record that a C-side add-listener API put the node on a list
    /// (same effect as [`Listener::attach`], for APIs that do the
    /// wl_signal_add themselves).
    pub(crate) fn mark_attached(&self) {
        debug_assert!(!self.inner.attached.get(), "double attach");
        self.inner.attached.set(true);
    }

    pub(crate) fn is_attached(&self) -> bool {
        self.inner.attached.get()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.detach();
    }
}

#[cfg(test)]
#[allow(clippy::undocumented_unsafe_blocks)] // tests drive the fake-C harness directly
mod tests {
    use super::*;
    use std::rc::Rc;

    // Uses the fake-C-object harness (D18): a real wl_signal from the
    // shim, real wl_list links, no compositor.

    #[test]
    fn emit_reaches_handler_and_multishot_stays_attached() {
        let ctx = crate::ctx::test_ctx();
        let hits = Rc::new(Cell::new(0u32));
        let h = Rc::clone(&hits);
        let l = Listener::new(
            "test",
            false,
            Box::new(move |_ctx, _data| h.set(h.get() + 1)),
        );
        // SAFETY(test): the harness signal outlives the attachment.
        let sig = unsafe { weston_sys::wsys_test_signal_create() };
        unsafe { l.attach(sig.cast()) };
        unsafe { weston_sys::wsys_test_signal_emit(sig, std::ptr::null_mut()) };
        unsafe { weston_sys::wsys_test_signal_emit(sig, std::ptr::null_mut()) };
        assert_eq!(hits.get(), 2);
        assert!(l.is_attached());
        l.detach();
        unsafe { weston_sys::wsys_test_signal_emit(sig, std::ptr::null_mut()) };
        assert_eq!(hits.get(), 2);
        unsafe { weston_sys::wsys_test_signal_destroy(sig) };
        ctx.teardown();
    }

    #[test]
    fn oneshot_self_detaches_inside_emission() {
        let ctx = crate::ctx::test_ctx();
        let hits = Rc::new(Cell::new(0u32));
        let h = Rc::clone(&hits);
        let l = Listener::new(
            "test-oneshot",
            true,
            Box::new(move |_ctx, _| h.set(h.get() + 1)),
        );
        let sig = unsafe { weston_sys::wsys_test_signal_create() };
        unsafe { l.attach(sig.cast()) };
        unsafe { weston_sys::wsys_test_signal_emit(sig, std::ptr::null_mut()) };
        assert_eq!(hits.get(), 1);
        assert!(!l.is_attached());
        // Owner death after emission: destroying the signal (freeing its
        // list head) must be safe with the node already off the list,
        // and dropping the listener afterwards must not touch it again.
        unsafe { weston_sys::wsys_test_signal_destroy(sig) };
        drop(l);
        ctx.teardown();
    }

    #[test]
    fn drop_detaches_from_live_signal() {
        let ctx = crate::ctx::test_ctx();
        let hits = Rc::new(Cell::new(0u32));
        let h = Rc::clone(&hits);
        let l = Listener::new(
            "test-drop",
            false,
            Box::new(move |_ctx, _| h.set(h.get() + 1)),
        );
        let sig = unsafe { weston_sys::wsys_test_signal_create() };
        unsafe { l.attach(sig.cast()) };
        drop(l);
        // The dropped listener's node must be gone from the list.
        unsafe { weston_sys::wsys_test_signal_emit(sig, std::ptr::null_mut()) };
        assert_eq!(hits.get(), 0);
        unsafe { weston_sys::wsys_test_signal_destroy(sig) };
        ctx.teardown();
    }
}
```

### A.12 `crates/weston/src/panic_barrier.rs` — copy as-is

```rust
//! No unwinding across FFI (plan §3g, D16).
//!
//! `panic = "unwind"` stays on in all profiles; every `extern "C"`
//! trampoline body runs inside [`guard`], which catches any panic, logs
//! it with the callback's context through weston_log, and aborts
//! deliberately.

use std::panic::{self, AssertUnwindSafe};

/// Run a trampoline body.  On panic: log with `what` for context, then
/// abort the process.  Never unwinds into C.
pub(crate) fn guard<R>(what: &str, body: impl FnOnce() -> R) -> R {
    match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(r) => r,
        Err(payload) => {
            let msg: &str = if let Some(s) = payload.downcast_ref::<&str>() {
                s
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s
            } else {
                "<non-string panic payload>"
            };
            crate::log::log_line(&format!(
                "westonite: fatal: Rust panic in callback '{what}': {msg}; aborting"
            ));
            std::process::abort();
        }
    }
}
```

### A.13 `crates/weston/src/ctx.rs` — copy as-is

The `CtxInner` fields that later slices need (`layoutputs`, `input_config`, `use_color_manager`, `xwayland`, `screenshooter`, `autolaunch`, `active_grabs`, `shell_layers`, `curtains`, `desktop`) may be added when their slice lands; the dispatch core (`dispatch_sync`, `with_depth`, `drain`, `enqueue`, `defer_drop`, `retire_listener`, `own_listener`, `teardown`, the graveyard) is final at R0.

```rust
//! `Ctx`: the wrapper's central state — registry, dispatch, drain
//! (plan §3b, §3e, §3j, D21).
//!
//! Aliasing model (load-bearing): C can call back into Rust while an
//! outbound wrapper call is on the stack, so `CtxInner` is only ever
//! reached through shared references; every field is individually
//! interior-mutable and **no borrow of any field is held across an FFI
//! call or an app-handler call**.  Trampolines reach `CtxInner` through
//! one private thread-local slot (D21) holding an `Rc` clone; `Ctx` is
//! `!Send + !Sync` (`Rc`), so the slot and all C state stay on the
//! libweston thread (§3j).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ptr::NonNull;
use std::rc::Rc;

use crate::compositor::BackendKind;
use crate::events::Event;
use crate::ids::DesktopSurfaceId;
use crate::listener::Listener;
use crate::registry::SlotTable;

/// The app (shell at R1, frontend later).  One entry point for both
/// dispatch tiers (§3e): sync-tier trampolines call [`Ctx::dispatch_sync`]
/// (each site carries a non-reentrancy proof — A3), deferred events are
/// drained at depth zero.  The borrow is taken once per delivered event
/// and never held across FFI.
pub trait ShellApp {
    fn handle(&mut self, host: &Ctx, event: Event);
}

/// Wrapper-held per-desktop-surface state (never visible to safe code).
pub(crate) struct SurfaceRec {
    pub(crate) view: NonNull<weston_sys::weston_view>,
    /// The underlying weston_surface (registered in `surfaces`).
    pub(crate) wsurf: NonNull<weston_sys::weston_surface>,
    /// Grab state the wrapper owns (C: shsurf->grabbed / resize_edges).
    pub(crate) grabbed: Cell<bool>,
    pub(crate) resize_edges: Cell<u32>,
    pub(crate) unresponsive: Cell<bool>,
}

pub(crate) struct CtxInner {
    // Raw C singletons (borrowed for the compositor's lifetime).
    pub(crate) compositor: Cell<*mut weston_sys::weston_compositor>,
    pub(crate) display: Cell<*mut weston_sys::wl_display>,
    /// The backends this builder loaded, in load order.  C keys one
    /// heads-changed listener off each `struct wet_backend` and filters
    /// heads by `head->backend == wb->backend` (main.c
    /// simple_heads_changed via wet_backend_iterate_heads); the wrapper
    /// installs one listener, so it carries the discriminators here and
    /// dispatches per head to that backend's configure flavor (the
    /// `wb->simple_output_configure` role).
    pub(crate) backends: RefCell<Vec<(*mut weston_sys::weston_backend, BackendKind)>>,
    /// C wet_compositor::layoutput_list — the DRM head→output grouping
    /// (see crate::layoutput).  Empty unless the DRM backend is loaded.
    pub(crate) layoutputs: RefCell<Vec<crate::layoutput::Layoutput>>,
    /// The `[libinput]` section, consulted by the DRM backend's
    /// `configure_device` hook (crate::libinput).  There is no user
    /// data on that callback, so this is how it reaches the config.
    /// Left at its default (every key absent) unless DRM is loaded.
    pub(crate) input_config: RefCell<Rc<crate::libinput::InputConfig>>,
    /// C wet_compositor::use_color_manager — set only when
    /// `[core] color-management=true` loaded the manager successfully.
    /// Gates icc-profile, and gates the non-default eotf/colorimetry
    /// modes with C's own error message.
    pub(crate) use_color_manager: Cell<bool>,
    /// C `wet_compositor.init_failed`: an output that could not be
    /// created, configured or enabled aborts startup rather than
    /// leaving a running compositor with missing outputs (main.c
    /// simple_head_enable → `wet_main`'s post-flush check).
    pub(crate) init_failed: Cell<bool>,

    // §3e: dispatch depth across trampolines AND outbound FFI calls;
    // whoever returns it to zero drains.
    depth: Cell<u32>,
    draining: Cell<bool>,
    /// True while a sync-tier handler invocation is on the stack (set
    /// around `app.handle` in [`Ctx::dispatch_sync`] only — the drain
    /// loop's invocations are covered by `draining`).  This is what
    /// lets the empty-app-slot case in `dispatch_sync` tell a failed
    /// A3 proof (loud) apart from the two quiet reasons the slot can
    /// be empty: mid-drain requeueing and no app installed yet.
    in_sync_handler: Cell<bool>,
    /// Set once teardown begins: enqueued events are discarded (§3e).
    pub(crate) shutting_down: Cell<bool>,

    queue: RefCell<VecDeque<Event>>,
    /// Boxes that must outlive the C frame currently on the stack
    /// (listener inners whose trampoline is executing, ended grabs — §3f).
    pending_drop: RefCell<Vec<Box<dyn Any>>>,

    app: RefCell<Option<Box<dyn ShellApp>>>,

    // §3b registries: one table per kind; insertion and destroy-listener
    // attachment are paired in the register_* helpers.
    pub(crate) outputs: RefCell<SlotTable<weston_sys::weston_output>>,
    pub(crate) heads: RefCell<SlotTable<weston_sys::weston_head>>,
    pub(crate) seats: RefCell<SlotTable<weston_sys::weston_seat>>,
    pub(crate) surfaces: RefCell<SlotTable<weston_sys::weston_surface>>,
    pub(crate) desktop_surfaces: RefCell<SlotTable<weston_sys::weston_desktop_surface>>,

    /// Wrapper state per live desktop surface.
    pub(crate) surface_recs: RefCell<HashMap<DesktopSurfaceId, Rc<SurfaceRec>>>,

    /// Wrapper-owned destroy listeners, keyed by the C object address.
    /// On death the entry migrates to `pending_drop` (its trampoline
    /// frame is still on the stack when the handler runs).
    pub(crate) listeners: RefCell<Vec<(usize, Listener)>>,

    // ---- shell-side wrapper state (R1) ----
    /// The two shell layers (kind-4 pinned allocations, §3a).
    pub(crate) shell_layers: RefCell<Option<crate::layer::ShellLayers>>,
    /// Per-output background curtain (kind 2: we destroy).
    pub(crate) curtains: RefCell<HashMap<crate::ids::OutputId, crate::curtain::Curtain>>,
    /// libweston-desktop context (destroyed at shell teardown).
    pub(crate) desktop: Cell<*mut weston_sys::weston_desktop>,
    /// Live pointer/touch grabs the wrapper started (§3f: the boxes
    /// live here while C dispatches on them; ending moves them to
    /// `pending_drop`).
    pub(crate) active_grabs: RefCell<Vec<crate::grab::ActiveGrab>>,
    /// Autolaunch watch (R2a): (pid, watch) — the SIGCHLD handler
    /// terminates the display when the watched client exits.
    pub(crate) autolaunch: Cell<Option<(i32, bool)>>,
    /// Xwayland frontend glue (R2d, C wet_xwayland): present iff
    /// --xwayland loaded the module.  Rc so trampolines clone out and
    /// drop the borrow before any FFI call.
    pub(crate) xwayland: RefCell<Option<Rc<crate::xwayland::XwaylandState>>>,
    /// Screenshooter/recorder glue (R2e, C struct screenshooter):
    /// present iff a shell attached (C: the shell calls
    /// screenshooter_create).  Same Rc clone-out discipline.
    pub(crate) screenshooter: RefCell<Option<Rc<crate::screenshooter::ScreenshooterState>>>,
}

thread_local! {
    /// D21: the one route from `extern "C"` trampolines back to the
    /// context.  Holding an `Rc` clone keeps resolution entirely safe;
    /// single-threadedness is guaranteed by `Rc`'s `!Send`.
    static CURRENT: RefCell<Option<Rc<CtxInner>>> = const { RefCell::new(None) };

    /// Boxes that must outlive even teardown.  The compositor-destroy
    /// trampoline tears the wrapper down while **its own listener's
    /// frame is still on the stack** (C has the identical shape:
    /// shell_destroy free()s the struct containing the executing
    /// wl_listener — but C never touches it afterwards, while our
    /// trampoline's RefMut guard writes the borrow flag on return).
    /// Teardown therefore detaches and PARKS boxes here instead of
    /// dropping them; they stay reachable until process exit.  Found by
    /// valgrind in the destroy-storm stress (invalid write in
    /// weston_compositor_destroy's destroy_signal emission).
    ///
    /// Active grab boxes are parked here too: C still holds
    /// pointer->grab / touch->grab (there is no detach for a grab), and
    /// weston_compositor_destroy shuts the backends down AFTER the
    /// destroy emission — the backends' input teardown then cancels any
    /// current grab through the embedded vtable
    /// (weston_seat_release_pointer/touch → weston_*_cancel_grab), so
    /// the box must outlive teardown.  The late cancel finds no Ctx and
    /// no-ops (grab.rs guard_ctx); the C shell leaks the allocation at
    /// the same point for the same reason.
    static GRAVEYARD: RefCell<Vec<Box<dyn Any>>> = const { RefCell::new(Vec::new()) };
}

/// The public handle.  Cloning is cheap (`Rc`); all clones refer to the
/// one compositor context on this thread.
#[derive(Clone)]
pub struct Ctx {
    pub(crate) inner: Rc<CtxInner>,
}

impl Ctx {
    /// Create the context and install it in the thread-local slot.
    /// One live `Ctx` per thread; the slot is cleared by
    /// [`Ctx::teardown`].
    pub(crate) fn new() -> Ctx {
        let inner = Rc::new(CtxInner {
            compositor: Cell::new(std::ptr::null_mut()),
            display: Cell::new(std::ptr::null_mut()),
            backends: RefCell::new(Vec::new()),
            layoutputs: RefCell::new(Vec::new()),
            input_config: RefCell::new(Rc::new(crate::libinput::InputConfig::default())),
            use_color_manager: Cell::new(false),
            init_failed: Cell::new(false),
            depth: Cell::new(0),
            draining: Cell::new(false),
            in_sync_handler: Cell::new(false),
            shutting_down: Cell::new(false),
            queue: RefCell::new(VecDeque::with_capacity(64)),
            pending_drop: RefCell::new(Vec::new()),
            app: RefCell::new(None),
            outputs: RefCell::new(SlotTable::default()),
            heads: RefCell::new(SlotTable::default()),
            seats: RefCell::new(SlotTable::default()),
            surfaces: RefCell::new(SlotTable::default()),
            desktop_surfaces: RefCell::new(SlotTable::default()),
            surface_recs: RefCell::new(HashMap::new()),
            listeners: RefCell::new(Vec::new()),
            shell_layers: RefCell::new(None),
            curtains: RefCell::new(HashMap::new()),
            desktop: Cell::new(std::ptr::null_mut()),
            active_grabs: RefCell::new(Vec::new()),
            autolaunch: Cell::new(None),
            xwayland: RefCell::new(None),
            screenshooter: RefCell::new(None),
        });
        CURRENT.with(|c| {
            let mut slot = c.borrow_mut();
            // Debug-only by decision (PR16-C10): in release a second
            // live Ctx would silently steal the trampoline slot from
            // the first (wrong registries, wrong queue), but the only
            // callers are CompositorBuilder::build and the hybrid
            // attach, each of which the frontend runs exactly once.
            // A hard error would need a fallible Ctx::new for a path
            // that cannot happen; the debug assert keeps the invariant
            // checked where tests run.
            debug_assert!(slot.is_none(), "second live Ctx on one thread");
            *slot = Some(Rc::clone(&inner));
        });
        Ctx { inner }
    }

    /// Resolve the thread-local context (trampoline path).  An empty
    /// slot during a live callback would be a wrapper teardown-ordering
    /// bug: debug_assert, benign `None` in release (§3j).
    pub(crate) fn current() -> Option<Ctx> {
        CURRENT.with(|c| {
            c.borrow().as_ref().map(|rc| Ctx {
                inner: Rc::clone(rc),
            })
        })
    }

    pub(crate) fn teardown(&self) {
        self.inner.shutting_down.set(true);
        self.inner.queue.borrow_mut().clear();
        // Listeners: detach now (their signals are still valid during
        // the destroy emission), but never drop the boxes — one of them
        // is the compositor-destroy listener whose trampoline frame is
        // executing this very function.  Parked in the GRAVEYARD (see
        // its doc comment); Drop at thread exit is a no-op detach.
        let parked: Vec<Box<dyn Any>> = self
            .inner
            .listeners
            .borrow_mut()
            .drain(..)
            .map(|(_, l)| {
                l.detach();
                Box::new(l) as Box<dyn Any>
            })
            .collect();
        // Pending-drop boxes may likewise have frames on the stack
        // deeper in this emission: park, don't drop.
        let pending: Vec<Box<dyn Any>> = std::mem::take(&mut *self.inner.pending_drop.borrow_mut());
        // Active grabs: C still holds pointer->grab / touch->grab, and
        // the backends' input teardown runs AFTER this emission,
        // cancelling any current grab through the embedded vtable —
        // dropping the boxes here would send that cancel through freed
        // memory.  Park them (see the GRAVEYARD doc); the late cancel
        // no-ops in grab.rs's guard_ctx once the slot below is cleared.
        let grabs: Vec<Box<dyn Any>> = self
            .inner
            .active_grabs
            .borrow_mut()
            .drain(..)
            .map(|g| Box::new(g) as Box<dyn Any>)
            .collect();
        GRAVEYARD.with(|g| {
            let mut g = g.borrow_mut();
            g.extend(parked);
            g.extend(pending);
            g.extend(grabs);
        });
        self.inner.surface_recs.borrow_mut().clear();
        self.inner.curtains.borrow_mut().clear();
        self.inner.shell_layers.borrow_mut().take();
        self.inner.app.borrow_mut().take();
        CURRENT.with(|c| c.borrow_mut().take());
    }

    /// Install the app.
    pub fn set_app(&self, app: Box<dyn ShellApp>) {
        *self.inner.app.borrow_mut() = Some(app);
    }

    /// Deliver a sync-tier event to the app right now (§3e sync tier;
    /// the calling trampoline's inventory row carries the A3 proof that
    /// the app borrow cannot already be held).  Falls back to enqueueing
    /// if the proof is ever violated — loud in debug builds.
    pub(crate) fn dispatch_sync(&self, ev: Event) {
        if self.inner.shutting_down.get() && !matches!(ev, Event::Shutdown) {
            return;
        }
        let taken = self.inner.app.borrow_mut().take();
        match taken {
            Some(mut app) => {
                self.inner.in_sync_handler.set(true);
                app.handle(self, ev);
                self.inner.in_sync_handler.set(false);
                let mut slot = self.inner.app.borrow_mut();
                if slot.is_none() {
                    *slot = Some(app);
                }
            }
            None => {
                // App slot empty.  Quiet cases: mid-drain (the drain
                // loop delivers the queued event in this same drain)
                // and no app installed yet.  A sync handler on the
                // stack outside a drain is neither — it's a failed A3
                // proof (the calling site's inventory row is wrong),
                // and silently deferring changes delivery order in
                // ways the policy cannot see.  The assert precedes the
                // log line on purpose: debug builds (incl. the unit
                // harness, which installs no weston_log handler) stop
                // here, and the panic barrier reports the message;
                // release builds log and degrade to the queue.
                if self.inner.in_sync_handler.get() && !self.inner.draining.get() {
                    debug_assert!(
                        false,
                        "A3 violation: sync event {ev:?} arrived with the app borrow held outside a drain"
                    );
                    crate::log::log_line(&format!(
                        "westonite: A3 violation: sync event {ev:?} deferred (app borrow held)"
                    ));
                }
                self.enqueue(ev);
            }
        }
    }

    /// Run a trampoline/outbound body at +1 dispatch depth; drain the
    /// deferred queue (then the pending-drop list) when returning to
    /// depth zero (§3e, amendment A4).
    pub(crate) fn with_depth<R>(&self, f: impl FnOnce() -> R) -> R {
        let inner = &self.inner;
        inner.depth.set(inner.depth.get() + 1);
        let r = f();
        inner.depth.set(inner.depth.get() - 1);
        if inner.depth.get() == 0 && !inner.draining.get() {
            self.drain();
        }
        r
    }

    fn drain(&self) {
        let inner = &self.inner;
        inner.draining.set(true);
        loop {
            let ev = inner.queue.borrow_mut().pop_front();
            let Some(ev) = ev else { break };
            if inner.shutting_down.get() {
                continue; // §3e: discard at teardown
            }
            let taken = inner.app.borrow_mut().take();
            if let Some(mut app) = taken {
                app.handle(self, ev);
                let mut slot = inner.app.borrow_mut();
                if slot.is_none() {
                    *slot = Some(app);
                }
            }
        }
        inner.draining.set(false);
        // Safe moment for freeing trampoline-owning boxes (§3f).  Note
        // the outermost trampoline's own C frame may still be on the
        // stack (this drain runs from its with_depth exit): the
        // invariant is narrower than "no C frame live" — every frame
        // still on the stack has already made its last access to any
        // box retired into this list (trampolines never touch their
        // inner after the handler returns, and deeper frames have
        // returned).  Teardown-time boxes go to the GRAVEYARD instead,
        // where even this invariant cannot be shown.
        let dead: Vec<Box<dyn Any>> = std::mem::take(&mut *inner.pending_drop.borrow_mut());
        drop(dead);
    }

    /// Enqueue a deferred-tier event.
    pub(crate) fn enqueue(&self, ev: Event) {
        if !self.inner.shutting_down.get() {
            self.inner.queue.borrow_mut().push_back(ev);
        }
    }

    /// Move a live box (listener/grab inner) to the deferred-drop list:
    /// it is freed at the next depth-zero drain, never inside the C
    /// frame that may be executing it (§3f).
    pub(crate) fn defer_drop(&self, b: Box<dyn Any>) {
        self.inner.pending_drop.borrow_mut().push(b);
    }

    /// Detach + retire a wrapper-owned listener for a dying C object:
    /// the box rides the pending-drop list because its own trampoline
    /// frame may be on the stack (§3f).
    pub(crate) fn retire_listener(&self, key: usize) {
        let mine = {
            let mut ls = self.inner.listeners.borrow_mut();
            ls.iter()
                .position(|(k, _)| *k == key)
                .map(|i| ls.swap_remove(i).1)
        };
        if let Some(l) = mine {
            self.defer_drop(Box::new(l));
        }
    }

    pub(crate) fn own_listener(&self, key: usize, l: Listener) {
        self.inner.listeners.borrow_mut().push((key, l));
    }

    pub(crate) fn compositor_ptr(&self) -> *mut weston_sys::weston_compositor {
        self.inner.compositor.get()
    }
}

/// Test-only: clear any previous test's thread-local ctx, then create a
/// fresh one (tests in this crate share threads).
#[cfg(test)]
pub(crate) fn test_ctx() -> Ctx {
    CURRENT.with(|c| c.borrow_mut().take());
    Ctx::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recorder(Rc<RefCell<Vec<Event>>>);
    impl ShellApp for Recorder {
        fn handle(&mut self, _ctx: &Ctx, ev: Event) {
            self.0.borrow_mut().push(ev);
        }
    }

    use super::test_ctx as fresh_ctx;

    #[test]
    fn events_drain_at_depth_zero_only() {
        let ctx = fresh_ctx();
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_app(Box::new(Recorder(Rc::clone(&seen))));

        ctx.with_depth(|| {
            ctx.enqueue(Event::HeadsChanged);
            ctx.with_depth(|| {
                ctx.enqueue(Event::CompositorShutdown);
                assert!(seen.borrow().is_empty());
            });
            assert!(seen.borrow().is_empty());
        });
        assert_eq!(
            *seen.borrow(),
            vec![Event::HeadsChanged, Event::CompositorShutdown]
        );
        ctx.teardown();
    }

    #[test]
    fn events_enqueued_during_drain_run_in_same_drain() {
        struct Chainer {
            seen: Rc<RefCell<Vec<Event>>>,
        }
        impl ShellApp for Chainer {
            fn handle(&mut self, ctx: &Ctx, ev: Event) {
                if matches!(ev, Event::HeadsChanged) {
                    ctx.with_depth(|| ctx.enqueue(Event::CompositorShutdown));
                }
                self.seen.borrow_mut().push(ev);
            }
        }
        let ctx = fresh_ctx();
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_app(Box::new(Chainer {
            seen: Rc::clone(&seen),
        }));
        ctx.with_depth(|| ctx.enqueue(Event::HeadsChanged));
        assert_eq!(
            *seen.borrow(),
            vec![Event::HeadsChanged, Event::CompositorShutdown]
        );
        ctx.teardown();
    }

    #[test]
    #[should_panic(expected = "A3 violation")]
    fn sync_dispatch_with_borrow_held_outside_drain_is_loud() {
        struct Nested;
        impl ShellApp for Nested {
            fn handle(&mut self, ctx: &Ctx, ev: Event) {
                if matches!(ev, Event::HeadsChanged) {
                    // A sync trampoline firing while a sync handler is
                    // on the stack is the A3-violation shape: debug
                    // builds must stop here (PR17-C1), not silently
                    // change delivery order.  (Release builds log and
                    // degrade to the queue — not testable from here.)
                    ctx.dispatch_sync(Event::SessionActivated);
                }
            }
        }
        let ctx = fresh_ctx();
        ctx.set_app(Box::new(Nested));
        ctx.with_depth(|| ctx.dispatch_sync(Event::HeadsChanged));
    }

    #[test]
    fn sync_dispatch_mid_drain_queues_quietly_into_same_drain() {
        struct Nested {
            seen: Rc<RefCell<Vec<Event>>>,
        }
        impl ShellApp for Nested {
            fn handle(&mut self, ctx: &Ctx, ev: Event) {
                if matches!(ev, Event::HeadsChanged) {
                    // The handler is running FROM the drain loop here,
                    // so a sync dispatch finding the app borrow out is
                    // the ordinary mid-drain case: quiet requeue,
                    // delivered later in this same drain — no assert.
                    ctx.dispatch_sync(Event::SessionActivated);
                    assert_eq!(self.seen.borrow().len(), 0);
                }
                self.seen.borrow_mut().push(ev);
            }
        }
        let ctx = fresh_ctx();
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_app(Box::new(Nested {
            seen: Rc::clone(&seen),
        }));
        ctx.with_depth(|| ctx.enqueue(Event::HeadsChanged));
        assert_eq!(
            *seen.borrow(),
            vec![Event::HeadsChanged, Event::SessionActivated]
        );
        ctx.teardown();
    }

    #[test]
    fn sync_dispatch_before_app_installed_queues_quietly() {
        // Builder-phase shape: no app yet.  The empty slot must stay
        // the quiet fallback (queue for later), not trip the A3 assert
        // — only a sync handler on the stack outside a drain is loud.
        let ctx = fresh_ctx();
        ctx.dispatch_sync(Event::HeadsChanged);
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_app(Box::new(Recorder(Rc::clone(&seen))));
        ctx.with_depth(|| {});
        assert_eq!(*seen.borrow(), vec![Event::HeadsChanged]);
        ctx.teardown();
    }

    #[test]
    fn teardown_discards_queued_events() {
        let ctx = fresh_ctx();
        let seen = Rc::new(RefCell::new(Vec::new()));
        ctx.set_app(Box::new(Recorder(Rc::clone(&seen))));
        ctx.enqueue(Event::HeadsChanged);
        ctx.teardown();
        ctx.with_depth(|| {});
        assert!(seen.borrow().is_empty());
    }
}
```

### A.14 `crates/weston/src/events.rs` — copy as-is

```rust
//! Shell/frontend events (plan §3e).
//!
//! One enum for both dispatch tiers.  Sync-tier events (the closed §3e
//! list: desktop-api lifecycle, bindings, session) are delivered to the
//! app synchronously from the trampoline — each such delivery carries a
//! non-reentrancy proof in docs/callback-inventory.md (A3).  Deferred
//! events are queued and drained at depth zero.  Events carry
//! **eagerly-captured payload**: by handling time a deferred subject may
//! be dead, and its id then resolves to `None` by design.

use crate::host::ResizeEdges;
use crate::ids::{DesktopSurfaceId, OutputId, SeatId, SurfaceId};

/// How an [`Event::Activate`] was triggered; decides the input-frame
/// view target and the C activation flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivateVia {
    /// BTN_LEFT/BTN_RIGHT binding: CLICKED|CONFIGURE on the pointer
    /// focus view.
    PointerBinding,
    /// Touch binding: CONFIGURE on the touch focus view.
    TouchBinding,
    /// Left click on a busy (unresponsive) window's grab: CONFIGURE on
    /// the surface's own view.
    BusyClick,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Event {
    // ---- desktop-api tier (sync; proofs in the callback inventory) ----
    /// A new desktop surface exists; the wrapper has already created its
    /// view and registered everything.
    SurfaceAdded {
        surface: DesktopSurfaceId,
    },
    /// The C object is going away (the "half-dead window", §3a): the id
    /// no longer resolves; payload is what the policy needs.
    SurfaceRemoved {
        surface: DesktopSurfaceId,
    },
    /// Commit processed.  `resize_edges` is the wrapper-held grab state
    /// at commit time (C: `shsurf->resize_edges`); width/height/mapped
    /// are the surface's state inside this commit frame.
    Committed {
        surface: DesktopSurfaceId,
        buf_dx: f64,
        buf_dy: f64,
        resize_edges: ResizeEdges,
        width: i32,
        height: i32,
        mapped: bool,
    },
    /// Parent changed (xdg transient stacking).
    ParentSet {
        surface: DesktopSurfaceId,
        parent: Option<DesktopSurfaceId>,
    },
    /// Xwayland told us where the window goes.
    XwaylandPosition {
        surface: DesktopSurfaceId,
        x: f64,
        y: f64,
    },
    /// A client stopped answering pings; payload: all its surfaces.
    PingTimeout {
        surfaces: Vec<DesktopSurfaceId>,
    },
    /// The client answered again.
    Pong {
        surfaces: Vec<DesktopSurfaceId>,
    },

    // ---- input tier ----
    /// Activation request from a binding or the busy-cursor grab (sync:
    /// the pointer/touch focus view is read inside the input frame).
    Activate {
        seat: SeatId,
        /// The main desktop surface being activated.
        surface: DesktopSurfaceId,
        via: ActivateVia,
    },

    // ---- object lifecycle: creation (sync; inventory L27/L28) ----
    /// Delivered via `dispatch_sync` from inside the `output_created`
    /// emission (`register_output_shell`): the shell creates the
    /// background curtain here, and that in-emission creation has been
    /// load-bearing since R2b (PR19-C1 was this curtain missing).
    OutputCreated {
        output: OutputId,
    },
    /// Delivered via `dispatch_sync` from inside the `seat_created`
    /// emission (`register_seat`).
    SeatCreated {
        seat: SeatId,
    },

    // ---- object lifecycle (deferred policy halves) ----
    /// A weston_surface we tracked (keyboard focus) died; the id is
    /// already stale.  The shell runs the C focus-replacement hunt
    /// (focus_state_surface_destroy).  `main` is the eager-captured
    /// desktop surface of the *main* surface when the dead one was a
    /// sub-surface (C's "activate its main surface" branch).
    TrackedSurfaceGone {
        surface: SurfaceId,
        main: Option<DesktopSurfaceId>,
    },
    /// Output died (id already stale).
    OutputGone {
        output: OutputId,
        name: String,
    },
    /// Output geometry changed (frontend resize path).
    OutputResized {
        output: OutputId,
    },
    /// Output moved by (dx, dy); views on it follow.
    OutputMoved {
        output: OutputId,
        dx: f64,
        dy: f64,
    },
    /// Seat died (id already stale).
    SeatGone {
        seat: SeatId,
    },
    /// A move/resize/busy grab the wrapper ran has ended; `surface` may
    /// already be stale.
    GrabEnded {
        surface: DesktopSurfaceId,
    },

    // ---- session / lifecycle ----
    /// VT switch back in (sync; re-issue input activation).
    SessionActivated,
    /// Compositor teardown began (sync, from the destroy listener); the
    /// app must release its per-object state now.
    Shutdown,
    // R0 leftovers used by the frontend smoke:
    HeadsChanged,
    HeadGone {
        head: crate::ids::HeadId,
        name: String,
    },
    CompositorShutdown,
}
```

### A.15 `crates/weston/src/host.rs` — copy as-is

```rust
//! `ShellHost`: the trait boundary the shell is written against
//! (plan §3e D20, amendment A5).
//!
//! Queries return `Option<plain data>`; commands take ids and are
//! no-ops on stale ids (D13: `Option` everywhere, skipped work — never
//! a panic).  No wrapper type, pointer, or `weston_sys` name crosses
//! this trait: `westonite-shell` stays `#![forbid(unsafe_code)]` and
//! unit-tests against a mock implementation.

use std::ptr::NonNull;

use crate::ctx::Ctx;
use crate::ids::{DesktopSurfaceId, OutputId, SeatId, SurfaceId};

/// Plain geometry (POD re-declared at the fence — §3h).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputInfo {
    /// Owned at the fence (§3h): copied, lossy-UTF-8, never borrowed.
    pub name: String,
    pub geometry: Rect,
}

bitflags::bitflags! {
    /// Mirror of `enum weston_desktop_surface_edge` (§3h: no raw u32
    /// reaches the shell).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct ResizeEdges: u32 {
        const TOP = 1;
        const BOTTOM = 2;
        const LEFT = 4;
        const RIGHT = 8;
    }
}

bitflags::bitflags! {
    /// Mirror of `enum weston_activate_flag`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct ActivateFlags: u32 {
        const CONFIGURE = 1;
        const CLICKED = 2;
    }
}

/// What view input activation lands on.  `PointerFocusView` re-reads the
/// seat's pointer focus inside the (sync) input frame — the C binding
/// path activates the exact clicked view, which may be a subsurface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivateTarget {
    SurfaceView(DesktopSurfaceId),
    PointerFocusView(SeatId),
    TouchFocusView(SeatId),
}

pub trait ShellHost {
    // ---- queries ----
    /// Live outputs in the compositor's own list order (C iterates
    /// `ec->output_list`; "first output" fallbacks depend on it).
    fn outputs(&self) -> Vec<OutputId>;
    fn output_info(&self, id: OutputId) -> Option<OutputInfo>;
    fn seats(&self) -> Vec<SeatId>;
    fn pointer_pos(&self, seat: SeatId) -> Option<(f64, f64)>;
    /// Main desktop surface under the seat's pointer focus, if any.
    fn pointer_focus(&self, seat: SeatId) -> Option<DesktopSurfaceId>;
    /// Workspace-layer views, topmost first, mapped to their desktop
    /// surfaces (§3l snapshot — the focus-replacement hunt).
    fn workspace_views_top_down(&self) -> Vec<DesktopSurfaceId>;
    fn is_view_mapped(&self, id: DesktopSurfaceId) -> bool;
    fn view_pos(&self, id: DesktopSurfaceId) -> Option<(f64, f64)>;
    fn view_output(&self, id: DesktopSurfaceId) -> Option<OutputId>;
    fn surface_geometry(&self, id: DesktopSurfaceId) -> Option<Rect>;

    // ---- commands (stale id ⇒ logged no-op) ----
    /// `weston_view_activate_input` on the target; returns the id of the
    /// `weston_surface` that received focus, registered with a destroy
    /// listener that will emit `TrackedSurfaceGone` (+ its main-surface
    /// payload).  This is the C `focus_state` listener, centralized.
    /// Tracking is ref-counted: every returned id counts as one
    /// acquisition and must be balanced by one [`untrack_surface`] call
    /// (several seats focusing the same surface share one listener).
    fn activate_input(
        &self,
        target: ActivateTarget,
        seat: SeatId,
        flags: ActivateFlags,
    ) -> Option<SurfaceId>;
    /// Release one acquisition of a previously returned focus surface;
    /// watching stops only when every acquisition has been released.
    fn untrack_surface(&self, id: SurfaceId);
    fn set_activated(&self, id: DesktopSurfaceId, active: bool);
    /// Move the surface's view to the top of the workspace layer and
    /// propagate to libweston-desktop-managed child views.
    fn raise_to_workspace_top(&self, id: DesktopSurfaceId);
    fn map_surface(&self, id: DesktopSurfaceId);
    fn set_view_position(&self, id: DesktopSurfaceId, x: f64, y: f64);
    /// Position with a surface-coordinate offset (the xwayland /
    /// resize-commit path).
    fn set_view_position_with_offset(
        &self,
        id: DesktopSurfaceId,
        x: f64,
        y: f64,
        offs_x: f64,
        offs_y: f64,
    );
    /// `weston_view_update_transform` on every view of the surface.
    fn update_transforms(&self, id: DesktopSurfaceId);
    fn mark_view_dirty(&self, id: DesktopSurfaceId);
    /// The C `desktop_shell_destroy_surface` tail: unlink + destroy the
    /// shell view, drop wrapper state, invalidate registries.
    fn destroy_surface_state(&self, id: DesktopSurfaceId);
    /// Busy-cursor grab on the seat's pointer (C `set_busy_cursor`).
    fn set_busy_cursor(&self, id: DesktopSurfaceId, seat: SeatId);
    /// End busy grabs held on any seat whose grabbed surface belongs to
    /// the same desktop client as `id` (C `end_busy_cursor`).
    fn end_busy_grabs_for_client_of(&self, id: DesktopSurfaceId);
    /// (Re)create the per-output solid background curtain.
    fn recreate_background(&self, output: OutputId, argb: u32);
}

// ---------------------------------------------------------------------
// Implementation on the real wrapper.
// ---------------------------------------------------------------------

impl Ctx {
    pub(crate) fn resolve_rec(
        &self,
        id: DesktopSurfaceId,
    ) -> Option<std::rc::Rc<crate::ctx::SurfaceRec>> {
        // Rec presence implies the desktop surface is still live: the
        // removal path drops the rec before the C object dies.
        self.inner.surface_recs.borrow().get(&id).cloned()
    }

    pub(crate) fn resolve_output(
        &self,
        id: OutputId,
    ) -> Option<NonNull<weston_sys::weston_output>> {
        self.inner.outputs.borrow().resolve(id.0)
    }

    pub(crate) fn resolve_seat(&self, id: SeatId) -> Option<NonNull<weston_sys::weston_seat>> {
        self.inner.seats.borrow().resolve(id.0)
    }

    /// pointer for a seat — scoped use only, never stored (§3a).
    pub(crate) fn seat_pointer(&self, id: SeatId) -> Option<NonNull<weston_sys::weston_pointer>> {
        let seat = self.resolve_seat(id)?;
        // SAFETY: live seat; get_pointer returns NULL when no pointer
        // capability exists (§3h nullable-return rule).
        NonNull::new(unsafe { weston_sys::weston_seat_get_pointer(seat.as_ptr()) })
    }

    pub(crate) fn ds_id_of(
        &self,
        ds: NonNull<weston_sys::weston_desktop_surface>,
    ) -> Option<DesktopSurfaceId> {
        self.inner
            .desktop_surfaces
            .borrow()
            .id_of(ds)
            .map(DesktopSurfaceId)
    }

    /// weston_surface → its desktop surface id (the C
    /// `get_shell_surface`, minus the user-data round-trip).
    pub(crate) fn ds_id_of_wsurf(
        &self,
        surf: NonNull<weston_sys::weston_surface>,
    ) -> Option<DesktopSurfaceId> {
        // SAFETY: live surface pointer handed to us by C inside a
        // callback frame; both calls are pure queries.
        unsafe {
            if !weston_sys::weston_surface_is_desktop_surface(surf.as_ptr()) {
                return None;
            }
            let ds = weston_sys::weston_surface_get_desktop_surface(surf.as_ptr());
            NonNull::new(ds).and_then(|ds| self.ds_id_of(ds))
        }
    }

    /// Main surface of a weston_surface → desktop surface id.
    pub(crate) fn main_ds_id_of_wsurf(
        &self,
        surf: NonNull<weston_sys::weston_surface>,
    ) -> Option<DesktopSurfaceId> {
        // SAFETY: live surface; get_main_surface is a pure query.
        let main = unsafe { weston_sys::weston_surface_get_main_surface(surf.as_ptr()) };
        NonNull::new(main).and_then(|m| self.ds_id_of_wsurf(m))
    }
}

impl ShellHost for Ctx {
    fn outputs(&self) -> Vec<OutputId> {
        let ec = self.compositor_ptr();
        if ec.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        // SAFETY: compositor live; iterating the intrusive output_list
        // via bound POD fields, snapshotting ids (§3l).
        unsafe {
            let head = &raw mut (*ec).output_list;
            let mut node = (*head).next;
            while node != head {
                let o = crate::container_of!(node, weston_sys::weston_output, link);
                if let Some(id) = NonNull::new(o).and_then(|p| self.inner.outputs.borrow().id_of(p))
                {
                    out.push(OutputId(id));
                }
                node = (*node).next;
            }
        }
        out
    }

    fn output_info(&self, id: OutputId) -> Option<OutputInfo> {
        let ptr = self.resolve_output(id)?;
        // SAFETY: registry-live output; POD reads, name copied (§3h).
        unsafe {
            let o = ptr.as_ref();
            let name = if o.name.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(o.name)
                    .to_string_lossy()
                    .into_owned()
            };
            Some(OutputInfo {
                name,
                geometry: Rect {
                    x: o.pos.c.x as i32,
                    y: o.pos.c.y as i32,
                    width: o.width,
                    height: o.height,
                },
            })
        }
    }

    fn seats(&self) -> Vec<SeatId> {
        self.inner
            .seats
            .borrow()
            .live_ids()
            .into_iter()
            .map(SeatId)
            .collect()
    }

    fn pointer_pos(&self, seat: SeatId) -> Option<(f64, f64)> {
        let p = self.seat_pointer(seat)?;
        // SAFETY: pointer valid within this call (scoped access, §3a).
        let pos = unsafe { p.as_ref().pos };
        Some((pos.c.x, pos.c.y))
    }

    fn pointer_focus(&self, seat: SeatId) -> Option<DesktopSurfaceId> {
        let p = self.seat_pointer(seat)?;
        // SAFETY: pointer valid within this call; focus may be NULL.
        let view = NonNull::new(unsafe { p.as_ref().focus })?;
        // SAFETY: focus view is live while the pointer holds it.
        let surf = NonNull::new(unsafe { view.as_ref().surface })?;
        self.main_ds_id_of_wsurf(surf)
    }

    fn workspace_views_top_down(&self) -> Vec<DesktopSurfaceId> {
        let Some(layer) = crate::layer::workspace_layer_ptr(self) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // SAFETY: the workspace layer is a live kind-4 allocation owned
        // by us; view_list iteration mirrors wl_list_for_each over
        // layer_link.link, snapshotted before any mutation (§3l).
        unsafe {
            let head = &raw mut (*layer).view_list.link;
            let mut node = (*head).next;
            while node != head {
                let entry = crate::container_of!(node, weston_sys::weston_layer_entry, link);
                let view = crate::container_of!(entry, weston_sys::weston_view, layer_link);
                if let Some(id) = NonNull::new((*view).surface).and_then(|s| self.ds_id_of_wsurf(s))
                {
                    out.push(id);
                }
                node = (*node).next;
            }
        }
        out
    }

    fn is_view_mapped(&self, id: DesktopSurfaceId) -> bool {
        let Some(rec) = self.resolve_rec(id) else {
            return false;
        };
        // SAFETY: rec implies live view; pure query.
        unsafe { weston_sys::weston_view_is_mapped(rec.view.as_ptr()) }
    }

    fn view_pos(&self, id: DesktopSurfaceId) -> Option<(f64, f64)> {
        let rec = self.resolve_rec(id)?;
        // SAFETY: live view; pure query returning POD.
        let pos = unsafe { weston_sys::weston_view_get_pos_offset_global(rec.view.as_ptr()) };
        Some((pos.c.x, pos.c.y))
    }

    fn view_output(&self, id: DesktopSurfaceId) -> Option<OutputId> {
        let rec = self.resolve_rec(id)?;
        // SAFETY: live view; output may be NULL.
        let out = NonNull::new(unsafe { rec.view.as_ref().output })?;
        self.inner.outputs.borrow().id_of(out).map(OutputId)
    }

    fn surface_geometry(&self, id: DesktopSurfaceId) -> Option<Rect> {
        let ds = self.inner.desktop_surfaces.borrow().resolve(id.0)?;
        // SAFETY: live desktop surface; pure query returning POD.
        let g = unsafe { weston_sys::weston_desktop_surface_get_geometry(ds.as_ptr()) };
        Some(Rect {
            x: g.x,
            y: g.y,
            width: g.width,
            height: g.height,
        })
    }

    fn activate_input(
        &self,
        target: ActivateTarget,
        seat: SeatId,
        flags: ActivateFlags,
    ) -> Option<SurfaceId> {
        let seat_ptr = self.resolve_seat(seat)?;
        let view: NonNull<weston_sys::weston_view> = match target {
            ActivateTarget::SurfaceView(id) => self.resolve_rec(id)?.view,
            ActivateTarget::PointerFocusView(s) => {
                let p = self.seat_pointer(s)?;
                // SAFETY: scoped pointer access; focus may be NULL.
                NonNull::new(unsafe { p.as_ref().focus })?
            }
            ActivateTarget::TouchFocusView(s) => {
                let sp = self.resolve_seat(s)?;
                // SAFETY: live seat; touch/focus may be NULL.
                let t = NonNull::new(unsafe { weston_sys::weston_seat_get_touch(sp.as_ptr()) })?;
                // SAFETY: touch valid within this call; focus may be NULL.
                NonNull::new(unsafe { t.as_ref().focus })?
            }
        };
        // SAFETY: view + seat live; activate_input is the C call at
        // shell.c:1655.
        self.with_depth(|| unsafe {
            weston_sys::weston_view_activate_input(view.as_ptr(), seat_ptr.as_ptr(), flags.bits());
        });
        // Track the focused weston_surface (C focus_state_set_focus).
        // SAFETY: the view is live inside this activation frame.
        let es = NonNull::new(unsafe { view.as_ref().surface })?;
        Some(crate::compositor::track_surface(self, es))
    }

    fn untrack_surface(&self, id: SurfaceId) {
        crate::compositor::untrack_surface(self, id);
    }

    fn set_activated(&self, id: DesktopSurfaceId, active: bool) {
        let Some(ds) = self.inner.desktop_surfaces.borrow().resolve(id.0) else {
            return;
        };
        // SAFETY: live desktop surface.
        self.with_depth(|| unsafe {
            weston_sys::weston_desktop_surface_set_activated(ds.as_ptr(), active);
        });
    }

    fn raise_to_workspace_top(&self, id: DesktopSurfaceId) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        let Some(ds) = self.inner.desktop_surfaces.borrow().resolve(id.0) else {
            return;
        };
        let Some(layer) = crate::layer::workspace_layer_ptr(self) else {
            return;
        };
        // SAFETY: live view/layer; mirrors shell_surface_update_layer
        // (move_to_layer to the list head = top, then propagate).
        self.with_depth(|| unsafe {
            weston_sys::weston_view_move_to_layer(rec.view.as_ptr(), &raw mut (*layer).view_list);
            weston_sys::weston_desktop_surface_propagate_layer(ds.as_ptr());
        });
    }

    fn map_surface(&self, id: DesktopSurfaceId) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        self.with_depth(||
            // SAFETY: rec implies live surface; map is idempotent.
            unsafe {
                weston_sys::weston_surface_map(rec.wsurf.as_ptr());
            });
    }

    fn set_view_position(&self, id: DesktopSurfaceId, x: f64, y: f64) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        let pos = weston_sys::weston_coord_global {
            c: weston_sys::weston_coord { x, y },
        };
        // SAFETY: live view; POD argument.
        self.with_depth(|| unsafe {
            weston_sys::weston_view_set_position(rec.view.as_ptr(), pos);
        });
    }

    fn set_view_position_with_offset(
        &self,
        id: DesktopSurfaceId,
        x: f64,
        y: f64,
        offs_x: f64,
        offs_y: f64,
    ) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        let pos = weston_sys::weston_coord_global {
            c: weston_sys::weston_coord { x, y },
        };
        // SAFETY: live view; the offset must name the view's surface
        // (weston_coord_surface invariant) — rec.wsurf is exactly that.
        let offs = weston_sys::weston_coord_surface {
            c: weston_sys::weston_coord {
                x: offs_x,
                y: offs_y,
            },
            coordinate_space_id: rec.wsurf.as_ptr().cast(),
        };
        self.with_depth(||
            // SAFETY: rec implies live view; pos/offs are POD.
            unsafe {
                weston_sys::weston_view_set_position_with_offset(rec.view.as_ptr(), pos, offs);
            });
    }

    fn update_transforms(&self, id: DesktopSurfaceId) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        // SAFETY: live view/surface; iterate surface->views like the C
        // commit path, snapshot first (§3l).
        self.with_depth(|| unsafe {
            weston_sys::weston_view_update_transform(rec.view.as_ptr());
            let surf = rec.wsurf.as_ptr();
            if !(*surf).output.is_null() {
                let head = &raw mut (*surf).views;
                let mut views = Vec::new();
                let mut node = (*head).next;
                while node != head {
                    views.push(crate::container_of!(
                        node,
                        weston_sys::weston_view,
                        surface_link
                    ));
                    node = (*node).next;
                }
                for v in views {
                    weston_sys::weston_view_update_transform(v);
                }
            }
        });
    }

    fn mark_view_dirty(&self, id: DesktopSurfaceId) {
        let Some(rec) = self.resolve_rec(id) else {
            return;
        };
        // SAFETY: live view.
        self.with_depth(|| unsafe {
            weston_sys::weston_view_geometry_dirty(rec.view.as_ptr());
        });
    }

    fn destroy_surface_state(&self, id: DesktopSurfaceId) {
        crate::desktop::destroy_surface_state(self, id);
    }

    fn set_busy_cursor(&self, id: DesktopSurfaceId, seat: SeatId) {
        crate::grab::set_busy_cursor(self, id, seat);
    }

    fn end_busy_grabs_for_client_of(&self, id: DesktopSurfaceId) {
        crate::grab::end_busy_grabs_for_client_of(self, id);
    }

    fn recreate_background(&self, output: OutputId, argb: u32) {
        crate::curtain::recreate_background(self, output, argb);
    }
}
```

### A.16 `crates/weston/src/log.rs` — copy as-is

```rust
//! weston_log plumbing for the wrapper (plan §3k, R2f).
//!
//! Outbound logging always formats in Rust and passes the result as a
//! `%s` argument — client-controlled text must never reach C as a format
//! string (§3i).
//!
//! Inbound is libweston's own machinery, as C wires it (main.c:4499):
//! the shim's `vlog`/`vlog_continue` timestamp each line and print it
//! into the **"log" scope**, and libweston's *subscribers* decide where
//! it goes — the log file, the in-memory flight recorder, or both.
//! [`LogContext`] owns that whole assembly and must be built by the
//! frontend before anything logs.
//!
//! Outside a `LogContext` — the R0 smoke binary, the unit harness, a
//! panic-barrier line after teardown — the shim falls back to writing
//! straight to [`wsys_rust_log_sink`], which appends to the same file
//! (or stderr).  C has no such fallback and would drop those lines.

use std::cell::RefCell;
use std::ffi::CString;
use std::fs::File;
use std::io::Write;
use std::os::unix::io::IntoRawFd;

/// C main.c DEFAULT_FLIGHT_REC_SIZE (main.c:76).
const DEFAULT_FLIGHT_REC_SIZE: usize = 5 * 1024 * 1024;
/// C main.c DEFAULT_FLIGHT_REC_SCOPES (main.c:77).
pub const DEFAULT_FLIGHT_REC_SCOPES: &str = "log,drm-backend";

thread_local! {
    /// Fallback destination for the shim handlers when no scope is
    /// installed: the --log file when one is open, stderr otherwise.
    /// Thread-local per §3j (all logging happens on the libweston
    /// thread).
    static LOG_FILE: RefCell<Option<File>> = const { RefCell::new(None) };
}

/// How the frontend wants its logging wired: C's `--log`,
/// `--logger-scopes` and `--flight-rec-scopes`, resolved.
#[derive(Debug, Clone, Default)]
pub struct LogSetup {
    /// `--log`: append here instead of stderr.
    pub file: Option<std::path::PathBuf>,
    /// `--logger-scopes` / `-l`.  Empty means C's default: subscribe
    /// the logger to "log" alone.
    pub logger_scopes: Vec<String>,
    /// `--flight-rec-scopes` / `-f`.  `None` means C's default list;
    /// an explicitly empty value disables the recorder entirely.
    pub flight_rec_scopes: Option<Vec<String>>,
}

/// C main.c's log context, log scope, log file and the two subscribers,
/// as one RAII owner.
///
/// Lifetime matters and mirrors main.c: created before the first log
/// line, handed to `weston_compositor_create` (borrowed — libweston
/// keeps the pointer), and destroyed *after* the compositor and the
/// display (main.c:4819).  Holding it in `main` and scoping the
/// compositor inside it gives that order for free.
pub struct LogContext {
    ctx: *mut weston_sys::weston_log_context,
    /// C main.c's `log_scope`.  Kept because libweston requires it be
    /// destroyed explicitly, before the context that owns it -- skip
    /// that and it warns "debug scope 'log' has not been destroyed"
    /// and leaks the scope (caught by the valgrind smoke).
    scope: *mut weston_sys::weston_log_scope,
    logger: *mut weston_sys::weston_log_subscriber,
    flight_rec: *mut weston_sys::weston_log_subscriber,
    /// The `FILE *` the log subscriber writes to.  Null when logging to
    /// stderr, which must never be fclosed.
    file: *mut weston_sys::FILE,
}

/// Why a [`LogContext`] could not be built.
#[derive(Debug)]
pub enum LogError {
    CtxCreate,
    ScopeCreate,
    FileOpen(std::path::PathBuf, std::io::Error),
}

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogError::CtxCreate => f.write_str("failed to initialize weston debug framework"),
            LogError::ScopeCreate => f.write_str("failed to create the \"log\" log scope"),
            LogError::FileOpen(p, e) => write!(f, "cannot open log file '{}': {e}", p.display()),
        }
    }
}

impl LogContext {
    /// C main.c:4499-4522, in order: create the context, add the "log"
    /// scope, open the log file, create the subscribers, subscribe.
    pub fn new(setup: &LogSetup) -> Result<LogContext, LogError> {
        // SAFETY: plain constructor; null-checked immediately.
        let ctx = unsafe { weston_sys::weston_log_ctx_create() };
        if ctx.is_null() {
            return Err(LogError::CtxCreate);
        }
        let mut lc = LogContext {
            ctx,
            scope: std::ptr::null_mut(),
            logger: std::ptr::null_mut(),
            flight_rec: std::ptr::null_mut(),
            file: std::ptr::null_mut(),
        };

        // SAFETY: ctx live; the name/description are static C strings
        // and libweston copies what it needs.  The three null callbacks
        // are what C passes (main.c:4505).
        let scope = unsafe {
            weston_sys::weston_log_ctx_add_log_scope(
                ctx,
                c"log".as_ptr(),
                c"Weston and Wayland log\n".as_ptr(),
                None,
                None,
                std::ptr::null_mut(),
            )
        };
        if scope.is_null() {
            return Err(LogError::ScopeCreate);
        }
        lc.scope = scope;

        // C weston_log_file_open (main.c:183): append, cloexec, line
        // buffered; stderr when there is no --log.  Opened through Rust
        // when there is a path, so a bad --log is a typed error rather
        // than a null FILE*, then handed to C as the FILE* the log
        // subscriber insists on.
        let dump_to = match &setup.file {
            Some(path) => {
                let f = File::options()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|e| LogError::FileOpen(path.clone(), e))?;
                // The fallback sink keeps its own handle on the same
                // file: it is used before this context exists and after
                // it is gone, and two appending descriptors on one file
                // interleave correctly.
                let dup = f
                    .try_clone()
                    .map_err(|e| LogError::FileOpen(path.clone(), e))?;
                LOG_FILE.with(|l| *l.borrow_mut() = Some(dup));
                // fdopen the descriptor we already validated rather
                // than fopen the path a second time: one open, one
                // error to report, and no window for the path to change
                // underneath us.  `into_raw_fd` hands ownership to the
                // stream, which Drop fcloses.
                lc.open_stream(f.into_raw_fd(), path)?
            }
            // A dup of stderr, not stderr itself: Drop fcloses whatever
            // it owns, and closing the real stderr would take every
            // later diagnostic with it.  C dodges this by never
            // fclosing when weston_logfile == stderr (main.c:208).
            None => {
                // SAFETY: fd 2 is open for the process lifetime.
                let fd = unsafe { libc::dup(2) };
                if fd < 0 {
                    return Err(LogError::FileOpen(
                        std::path::PathBuf::from("<stderr>"),
                        std::io::Error::last_os_error(),
                    ));
                }
                lc.open_stream(fd, std::path::Path::new("<stderr>"))?
            }
        };

        // SAFETY: dump_to is a live stream that outlives the
        // subscriber (Drop destroys the subscriber before the file).
        lc.logger = unsafe { weston_sys::weston_log_subscriber_create_log(dump_to) };

        // C: an absent --flight-rec-scopes means the default list; an
        // explicitly empty one disables the recorder (main.c:4515).
        let flight_scopes: Vec<String> = match &setup.flight_rec_scopes {
            Some(v) => v.clone(),
            None => DEFAULT_FLIGHT_REC_SCOPES
                .split(',')
                .map(str::to_string)
                .collect(),
        };
        if !flight_scopes.is_empty() {
            // SAFETY: constructor; owned by us, destroyed in Drop.
            lc.flight_rec = unsafe {
                weston_sys::weston_log_subscriber_create_flight_rec(DEFAULT_FLIGHT_REC_SIZE)
            };
        }

        // C weston_log_subscribe_to_scopes (main.c:4380).
        if setup.logger_scopes.is_empty() {
            lc.subscribe(lc.logger, "log");
        } else {
            for name in &setup.logger_scopes {
                lc.subscribe(lc.logger, name);
            }
        }
        if !lc.flight_rec.is_null() {
            for name in &flight_scopes {
                lc.subscribe(lc.flight_rec, name);
            }
        }

        // Only now do log lines have anywhere to go.
        // SAFETY: the scope outlives the handlers -- it belongs to
        // `ctx`, which Drop destroys last, and teardown re-installs the
        // fallback pair before that.
        unsafe { weston_sys::wsys_install_log_handlers(scope) };
        Ok(lc)
    }

    /// `fdopen` a descriptor we own into the `FILE *` the log
    /// subscriber requires, line-buffered as C's setvbuf does so a
    /// compositor that dies mid-run still leaves whole lines behind.
    /// The stream takes the descriptor over and Drop fcloses it.
    fn open_stream(
        &mut self,
        fd: std::os::unix::io::RawFd,
        path: &std::path::Path,
    ) -> Result<*mut weston_sys::FILE, LogError> {
        // SAFETY: `fd` is open and no longer owned by anything else;
        // fdopen takes it over.
        let fp = unsafe { libc::fdopen(fd, c"a".as_ptr()) };
        if fp.is_null() {
            let e = std::io::Error::last_os_error();
            // SAFETY: fdopen failed, so the descriptor is still ours.
            unsafe { libc::close(fd) };
            return Err(LogError::FileOpen(path.to_path_buf(), e));
        }
        // SAFETY: fp is a live stream we just created.
        unsafe { libc::setvbuf(fp, std::ptr::null_mut(), libc::_IOLBF, 256) };
        self.file = fp.cast();
        Ok(fp.cast())
    }

    fn subscribe(&self, subscriber: *mut weston_sys::weston_log_subscriber, name: &str) {
        if subscriber.is_null() {
            return;
        }
        let Ok(c) = CString::new(name) else { return };
        // SAFETY: ctx and subscriber live; libweston copies the name.
        unsafe { weston_sys::weston_log_subscribe(self.ctx, subscriber, c.as_ptr()) };
    }

    /// The raw context, for `weston_compositor_create`.  Borrowed:
    /// libweston keeps the pointer for the compositor's lifetime, which
    /// is why this owner has to outlive it.
    pub(crate) fn as_ptr(&self) -> *mut weston_sys::weston_log_context {
        self.ctx
    }

    /// Whether the flight recorder is running — C logs this at startup
    /// and gates the Super+D debug binding on it.
    pub fn flight_rec_enabled(&self) -> bool {
        !self.flight_rec.is_null()
    }

    pub(crate) fn flight_rec_ptr(&self) -> *mut weston_sys::weston_log_subscriber {
        self.flight_rec
    }
}

impl Drop for LogContext {
    fn drop(&mut self) {
        // Put the handlers back on the scope-free fallback first: the
        // teardown below destroys the scope this context installed, and
        // a log line in between (a panic-barrier report, say) would
        // otherwise print into freed memory.
        // SAFETY: passing null selects the fallback path in the shim.
        unsafe { weston_sys::wsys_install_log_handlers(std::ptr::null_mut()) };
        // SAFETY: destruction order is main.c:4817-4823 exactly — the
        // scope first, then the subscribers, then the context, then the
        // file.  The file is last because the log subscriber writes to
        // it right up until it is destroyed.
        unsafe {
            if !self.scope.is_null() {
                weston_sys::weston_log_scope_destroy(self.scope);
            }
            if !self.logger.is_null() {
                weston_sys::weston_log_subscriber_destroy(self.logger);
            }
            if !self.flight_rec.is_null() {
                weston_sys::weston_log_subscriber_destroy(self.flight_rec);
            }
            if !self.ctx.is_null() {
                weston_sys::weston_log_ctx_destroy(self.ctx);
            }
            if !self.file.is_null() {
                libc::fclose(self.file.cast());
            }
        }
    }
}

/// Public logging entry for the safe frontend crates: one line through
/// weston_log (reaches the file/stderr sink installed above).  No
/// trailing newline in `msg` — it is appended here (see [`log_line`]).
pub fn message(msg: &str) {
    log_line(msg);
}

/// Emit one line through weston_log (goes to the handlers installed via
/// the shim; before a compositor/log context exists it still reaches the
/// handler pair, which is why the panic barrier can use it early).
///
/// No trailing newline — the `%s\n` below appends it.  This is the
/// OPPOSITE of the C convention, where every weston_log call site
/// writes its own `\n`: porting a C log line verbatim produces a blank
/// line (PR16-S6).
pub(crate) fn log_line(msg: &str) {
    // Sanitize interior NULs rather than fail: this is the logging path
    // the panic barrier depends on.
    let c = CString::new(msg.replace('\0', "\u{fffd}"))
        .unwrap_or_else(|_| CString::new("westonite: <unloggable>").expect("static"));
    // SAFETY: weston_log is a plain variadic; "%s\n" consumes exactly the
    // one pointer argument we pass, which outlives the call.
    unsafe {
        weston_sys::weston_log(c"%s\n".as_ptr(), c.as_ptr());
    }
}

/// C main.c:4589: announce the pid and `raise(SIGSTOP)`, so a debugger
/// can attach to a compositor that has created nothing yet.  Resumes on
/// SIGCONT; if no one ever sends it, the process simply stays stopped,
/// which is the point.
///
/// Lives here rather than in the frontend because `raise` is unsafe and
/// the frontend crates are `forbid(unsafe_code)`.
pub fn wait_for_debugger() {
    log_line(&format!(
        "Weston PID is {} - waiting for debugger, send SIGCONT to continue...",
        std::process::id()
    ));
    // SAFETY: raise(SIGSTOP) on our own process; the default action
    // stops us until SIGCONT arrives.  No handler state is involved.
    unsafe { libc::raise(libc::SIGSTOP) };
}

/// C sigchld_handler's per-child exit lines (main.c:401-409), shared by
/// every frontend-tracked child (xwayland, screenshooter).
pub(crate) fn log_child_exit(path: &str, status: i32) {
    if libc::WIFEXITED(status) {
        log_line(&format!(
            "{path} exited with status {}",
            libc::WEXITSTATUS(status)
        ));
    } else if libc::WIFSIGNALED(status) {
        log_line(&format!("{path} died on signal {}", libc::WTERMSIG(status)));
    } else {
        log_line(&format!("{path} disappeared"));
    }
}

/// Install the shim's vlog/vlog_continue handlers with **no scope**, so
/// weston_log reaches [`wsys_rust_log_sink`] directly.
///
/// This is the pre-[`LogContext`] path: the R0 smoke binary, the unit
/// harness, and the frontend's own earliest failures (a bad `--log`
/// path has to be reportable before the log file exists).  A
/// `LogContext` replaces them with the scope-aware pair and Drop puts
/// these back.
pub fn install_stderr_handlers() {
    // SAFETY: registers two C functions defined in the shim; they stay
    // valid for the process lifetime.  Null selects the fallback path.
    unsafe { weston_sys::wsys_install_log_handlers(std::ptr::null_mut()) }
}

/// Rust sink for the shim's handlers: write to stderr, return the char
/// count as weston_log expects.  `extern "C"`: called from the shim with
/// a borrowed buffer valid only for this call (§3a kind 5).
#[unsafe(no_mangle)]
extern "C" fn wsys_rust_log_sink(buf: *const libc::c_char, len: usize, _cont: bool) -> libc::c_int {
    crate::panic_barrier::guard("wsys_rust_log_sink", || {
        if buf.is_null() {
            return 0;
        }
        // SAFETY: the shim passes a buffer of exactly `len` initialized
        // bytes, valid for the duration of this call.
        let bytes = unsafe { std::slice::from_raw_parts(buf.cast::<u8>(), len) };
        LOG_FILE.with(|l| {
            let mut slot = l.borrow_mut();
            match slot.as_mut() {
                Some(f) => {
                    let _ = f.write_all(bytes);
                    let _ = f.flush();
                }
                None => {
                    let _ = std::io::stderr().lock().write_all(bytes);
                }
            }
        });
        len as libc::c_int
    })
}

/// C `flight_rec_key_binding_handler` (main.c:4371): dump the recorder's
/// ring buffer.  Registered as a *debug* binding (Super+Shift+Space
/// then D), so it costs nothing until someone asks for it.
///
/// The subscriber arrives as the binding's `data` — the one callback in
/// the tree that carries a payload rather than resolving through
/// `Ctx::current`, because C hands it the same way and the pointer is
/// owned by the frontend's [`LogContext`], which outlives every
/// binding.
pub(crate) unsafe extern "C" fn flight_rec_binding(
    _keyboard: *mut weston_sys::weston_keyboard,
    _time: *const weston_sys::timespec,
    _key: u32,
    data: *mut std::ffi::c_void,
) {
    crate::panic_barrier::guard("binding.flight_rec", || {
        if data.is_null() {
            return;
        }
        let sub = data.cast::<weston_sys::weston_log_subscriber>();
        let dump = || {
            // SAFETY: `data` is the flight-recorder subscriber the
            // frontend registered; it outlives the compositor, so it is
            // live for any binding that can still fire.
            unsafe { weston_sys::weston_log_subscriber_display_flight_rec(sub) };
        };
        // The dump goes out through weston_log, which re-enters our
        // handlers -- so it is an outbound call like any other and takes
        // the A4 wrap when there is a context to wrap it in.
        match crate::ctx::Ctx::current() {
            Some(ctx) => ctx.with_depth(dump),
            None => dump(),
        }
    });
}
```

### A.17 `crates/weston/src/compositor.rs` — the load-bearing parts

The file is about 2700 lines; the parts below fix the bring-up order, the signal shape, the run-loop rule and the teardown order. The backend loaders (`load_headless`, `load_vnc`, `load_x11`, `load_wayland`, `load_pipewire`, `load_drm`), `heads_changed`, `enable_head`, `register_head`/`register_output`, `track_surface`/`untrack_surface`, `lazy_align`, `apply_color`, `mirror_on_output_created`/`mirror_on_output_resized` and `create_windowed_heads` are written per §4.12–§4.13 and §9 against the C `main.c` functions the callback inventory names.


**A.17a Types and the builder.**

```rust
//! Compositor bring-up, run loop, and teardown (plan §7 R0).
//!
//! Mirrors the C frontend's headless path (`main.c`
//! `load_headless_backend` → `simple_heads_changed` →
//! `simple_head_enable`) closely enough that the Phase-1 smoke test is
//! reproducible from Rust: create display + log context + compositor,
//! load the headless backend with the noop renderer, create a head,
//! configure+enable an output per connected head, run, exit 0 on
//! SIGTERM.  The real frontend port replaces the canned output
//! configurator at R2; the primitives this file exercises are the R0
//! deliverable.

/// evdev KEY_D — the flight recorder's dump key (C main.c:4634).
const KEY_D: u32 = 32;

use std::ffi::{CString, c_int, c_void};
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ctx::Ctx;
use crate::events::Event;
use crate::listener::Listener;
use crate::log;
use crate::output_policy::{OutputPolicy, OutputSetup};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Headless,
    Vnc,
    X11,
    Wayland,
    Pipewire,
    Drm,
}

/// Headless backend options (C load_headless_backend's CLI slice).
#[derive(Debug, Clone, Default)]
pub struct HeadlessOptions {
    /// C --no-outputs: create no head at all.
    pub no_outputs: bool,
    /// C --refresh-rate, mHz; backend default -1.
    pub refresh_mhz: Option<i32>,
    /// C `[core] output-decorations` (main.c:3494): draw a decoration
    /// border around each headless output.  Headless-only in C too --
    /// it is a field of weston_headless_backend_config.
    pub decorate: bool,
}

/// VNC backend options (C load_vnc_backend: CLI + `[vnc]` section,
/// already merged by the frontend's resolution).
#[derive(Debug, Clone, Default)]
pub struct VncOptions {
    /// C --address; backend default: all interfaces.
    pub bind_address: Option<String>,
    /// C --port / `[vnc] port`; backend default 5900.
    pub port: Option<i32>,
    /// C `[vnc] refresh-rate`, Hz; VNC_DEFAULT_FREQ (60) otherwise.
    pub refresh_rate_hz: Option<i32>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub disable_tls: bool,
}

#[derive(Debug, Clone)]
enum BackendSpec {
    Headless(HeadlessOptions),
    Vnc(VncOptions),
    X11(X11Options),
    Wayland(WaylandOptions),
    Pipewire(PipewireOptions),
    Drm(DrmOptions),
}

/// The two backend-level settings the DRM heads-changed branch needs
/// that are not per-output policy.
#[derive(Debug, Clone, Copy, Default)]
struct DrmRuntime {
    /// C --current-mode: every output takes its current mode.
    current_mode: bool,
    /// C `[core] require-outputs` — how hard a failed layoutput is.
    require_outputs: crate::layoutput::RequireOutputs,
}

/// DRM/KMS backend options (C load_drm_backend, main.c:3375).
///
/// Unlike every other backend this one creates no heads of its own on
/// load: the backend discovers connectors and the heads-changed
/// listener does the work, through the layoutput machinery rather than
/// `simple_heads_changed` (see [`crate::layoutput`]).
#[derive(Debug, Clone, Default)]
pub struct DrmOptions {
    /// C --seat / the libinput seat id; backend default "seat0".
    pub seat_id: Option<String>,
    /// C --drm-device, e.g. "card0"; backend default: pick one.
    pub specific_device: Option<String>,
    /// C --additional-devices.
    pub additional_devices: Option<String>,
    /// C `[core] gbm-format` — the backend-wide default, distinct from
    /// the per-output `gbm-format=`.
    pub gbm_format: Option<String>,
    /// C `[core] pageflip-timeout`, ms; 0 disables.
    pub pageflip_timeout: u32,
    /// C `[core] pixman-shadow`, default true.
    pub use_pixman_shadow: bool,
    /// C --current-mode: force every output to its current mode.
    pub current_mode: bool,
    /// C --continue-without-input: clears compositor->require_input, so
    /// a machine with no keyboard or mouse still starts.
    pub continue_without_input: bool,
    /// C weston.ini `[libinput]`, applied per device through the
    /// backend's `configure_device` hook (crate::libinput).  DRM is the
    /// only backend that has that hook, which is why the section rides
    /// along here rather than on the builder.
    pub input: crate::libinput::InputConfig,
}

/// X11 backend options (C load_x11_backend's CLI slice, main.c:3947).
#[derive(Debug, Clone, Default)]
pub struct X11Options {
    pub fullscreen: bool,
    pub no_input: bool,
    /// C --output-count, default 1: how many `screenN` heads to create
    /// beyond the `[[output]]` sections whose name starts with 'X'.
    pub output_count: i32,
}

/// Wayland (nested) backend options (C load_wayland_backend, 4070).
#[derive(Debug, Clone, Default)]
pub struct WaylandOptions {
    /// C --display: parent compositor socket (None = WAYLAND_DISPLAY).
    pub display_name: Option<String>,
    pub fullscreen: bool,
    /// C --sprawl: one output spanning the parent's outputs; the
    /// windowed-output API is then absent and nothing is configurable.
    pub sprawl: bool,
    pub output_count: i32,
    /// C `[shell] cursor-theme` / `cursor-size` (default 32).
    pub cursor_theme: Option<String>,
    pub cursor_size: i32,
}

/// PipeWire backend options (C load_pipewire_backend, 3625).
#[derive(Debug, Clone, Default)]
pub struct PipewireOptions {
    /// C `[core] gbm-format` (the backend-wide one).
    pub gbm_format: Option<String>,
    /// C `[pipewire] num-outputs`, default 1.
    pub num_outputs: i32,
}

/// `[keyboard]` slice of C weston_compositor_init_config, applied
/// between compositor creation and backend load.  Mandatory before any
/// backend that creates a keyboard: the VNC backend strdup's
/// `compositor->xkb_names` unconditionally (vnc.c:1260), so an
/// uninitialized set segfaults it.  `None` fields let libweston fill
/// its own defaults (evdev/pc105/us).
#[derive(Debug, Clone, Default)]
pub struct KeyboardConfig {
    pub rules: Option<String>,
    pub model: Option<String>,
    pub layout: Option<String>,
    pub variant: Option<String>,
    pub options: Option<String>,
    /// C default 40.
    pub repeat_rate: Option<i32>,
    /// C default 400.
    pub repeat_delay: Option<i32>,
    /// C default true.
    pub vt_switching: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererKind {
    Auto,
    Noop,
    Pixman,
    Gl,
}

impl RendererKind {
    fn to_c(self) -> weston_sys::weston_renderer_type::Type {
        match self {
            RendererKind::Auto => weston_sys::weston_renderer_type::WESTON_RENDERER_AUTO,
            RendererKind::Noop => weston_sys::weston_renderer_type::WESTON_RENDERER_NOOP,
            RendererKind::Pixman => weston_sys::weston_renderer_type::WESTON_RENDERER_PIXMAN,
            RendererKind::Gl => weston_sys::weston_renderer_type::WESTON_RENDERER_GL,
        }
    }
}

#[derive(Debug)]
pub enum CompositorError {
    DisplayCreate,
    LogCtxCreate,
    CompositorCreate,
    BackendLoad,
    WindowedOutputApi,
    HeadCreate,
    SocketAdd,
    SignalSource,
    ShellInit,
    OutputInit,
    Xwayland,
    ColorManager,
}

impl std::fmt::Display for CompositorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CompositorError::DisplayCreate => "wl_display_create failed",
            CompositorError::LogCtxCreate => "weston_log_ctx_create failed",
            CompositorError::CompositorCreate => "weston_compositor_create failed",
            CompositorError::BackendLoad => "loading the backend failed",
            CompositorError::ColorManager => "loading the color manager failed",
            CompositorError::WindowedOutputApi => "windowed output API unavailable",
            CompositorError::HeadCreate => "creating the head failed",
            CompositorError::SocketAdd => "binding the wayland socket failed",
            CompositorError::SignalSource => "installing signal sources failed",
            CompositorError::ShellInit => "shell initialization failed",
            CompositorError::OutputInit => "configuring the outputs failed",
            CompositorError::Xwayland => "loading the xwayland module failed",
        };
        f.write_str(s)
    }
}

impl std::error::Error for CompositorError {}

type ShellFactory = Box<dyn FnOnce(u32) -> Box<dyn crate::ctx::ShellApp>>;

pub struct CompositorBuilder {
    backends: Vec<BackendSpec>,
    renderer: RendererKind,
    policy: OutputPolicy,
    keyboard: KeyboardConfig,
    repaint_window_msec: Option<i32>,
    socket: bool,
    socket_name: Option<String>,
    shell: Option<(u32, ShellFactory)>,
    /// Some(xserver path) loads xwayland.so in build() (C main.c:4725).
    xwayland: Option<std::path::PathBuf>,
    /// C `[core] require-outputs` — DRM only (it is the only backend
    /// whose outputs can fail to come up).
    require_outputs: crate::layoutput::RequireOutputs,
    /// C `[core] color-management` (main.c:1203).
    color_management: bool,
    /// C `--debug`: the weston-debug protocol plus an allow-all
    /// screenshot authority.  A privilege change, so it is opt-in and
    /// never inferred.
    debug_protocol: bool,
    /// The frontend's log context, borrowed for
    /// `weston_compositor_create`.  Required: `build()` fails without
    /// one, because C has no path that reaches a compositor without a
    /// log context either.
    log_ctx: Option<*mut weston_sys::weston_log_context>,
    /// The flight-recorder subscriber, when one is running: C hangs the
    /// Super+D dump binding off it (main.c:4633).
    flight_rec: *mut weston_sys::weston_log_subscriber,
}

impl Default for CompositorBuilder {
    fn default() -> Self {
        CompositorBuilder::new()
    }
}

impl CompositorBuilder {
    /// No backends yet — add them in load order (C load_backends walks
    /// the comma list in order, primary first).  build() fails on an
    /// empty list.
    pub fn new() -> CompositorBuilder {
        CompositorBuilder {
            backends: Vec::new(),
            renderer: RendererKind::Noop,
            // C headless_backend_output_configure defaults.
            policy: OutputPolicy::defaults(1024, 640),
            keyboard: KeyboardConfig::default(),
            repaint_window_msec: None,
            socket: false,
            socket_name: None,
            shell: None,
            xwayland: None,
            require_outputs: crate::layoutput::RequireOutputs::default(),
            color_management: false,
            debug_protocol: false,
            log_ctx: None,
            flight_rec: std::ptr::null_mut(),
        }
    }

    /// C `--debug` (main.c:4626): enable the weston-debug protocol and
    /// let any client capture any output.
    pub fn debug_protocol(mut self, on: bool) -> CompositorBuilder {
        self.debug_protocol = on;
        self
    }

    /// Hand the builder the frontend's [`crate::log::LogContext`].
    /// Borrowed, not consumed: it must outlive the compositor (main.c
    /// destroys it after the display), so the frontend keeps it.
    pub fn with_log_context(mut self, log: &crate::log::LogContext) -> CompositorBuilder {
        self.log_ctx = Some(log.as_ptr());
        self.flight_rec = log.flight_rec_ptr();
        self
    }

    /// Load the xwayland module at build() and lazily spawn `xserver`
    /// on the first X connection (C --xwayland / [core] xwayland).
    pub fn with_xwayland(mut self, xserver: std::path::PathBuf) -> Self {
        self.xwayland = Some(xserver);
        self
    }

    /// `[keyboard]` configuration (C weston_compositor_init_config).
    pub fn with_keyboard(mut self, kb: KeyboardConfig) -> Self {
        self.keyboard = kb;
        self
    }

    /// `[core] repaint-window` (ms; C validates the -10..=1000 range
    /// with a warning and keeps libweston's default otherwise).
    pub fn with_repaint_window_msec(mut self, msec: i32) -> Self {
        self.repaint_window_msec = Some(msec);
        self
    }

    /// One default headless backend (the R0 smoke path).
    pub fn headless() -> CompositorBuilder {
        CompositorBuilder::new().add_headless(HeadlessOptions::default())
    }

    pub fn add_headless(mut self, opts: HeadlessOptions) -> Self {
        self.backends.push(BackendSpec::Headless(opts));
        self
    }

    pub fn add_x11(mut self, opts: X11Options) -> Self {
        self.backends.push(BackendSpec::X11(opts));
        self
    }

    pub fn add_wayland(mut self, opts: WaylandOptions) -> Self {
        self.backends.push(BackendSpec::Wayland(opts));
        self
    }

    pub fn add_pipewire(mut self, opts: PipewireOptions) -> Self {
        self.backends.push(BackendSpec::Pipewire(opts));
        self
    }

    pub fn add_vnc(mut self, opts: VncOptions) -> Self {
        self.backends.push(BackendSpec::Vnc(opts));
        self
    }

    pub fn add_drm(mut self, opts: DrmOptions) -> Self {
        self.backends.push(BackendSpec::Drm(opts));
        self
    }

    /// C `[core] require-outputs` (main.c:4644, default "any").
    pub fn require_outputs(mut self, r: crate::layoutput::RequireOutputs) -> Self {
        self.require_outputs = r;
        self
    }

    /// C `[core] color-management=true`: load the colour manager.
    /// Without it every colour key is inert in C, so the frontend
    /// refuses the combination rather than reproducing that silence.
    pub fn color_management(mut self, on: bool) -> Self {
        self.color_management = on;
        self
    }

    /// Renderer selection, applied to every backend loaded (C passes
    /// the one CLI/global choice into each loader's config.renderer).
    pub fn renderer(mut self, r: RendererKind) -> Self {
        self.renderer = r;
        self
    }

    /// Default output geometry only (R0 smoke sugar); the frontend
    /// passes a full [`OutputPolicy`] instead.
    pub fn output_size(mut self, width: i32, height: i32) -> Self {
        self.policy.default_size = (width, height);
        self
    }

    /// The resolved output configuration (R2b): per-head `[[output]]`
    /// rules + CLI overrides, consulted by the heads-changed handler.
    pub fn with_output_policy(mut self, policy: OutputPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Also bind a wayland socket (auto name) so clients can connect.
    pub fn with_socket(mut self) -> Self {
        self.socket = true;
        self
    }

    /// Bind a wayland socket with an explicit name (C --socket).
    pub fn with_socket_name(mut self, name: &str) -> Self {
        self.socket = true;
        self.socket_name = Some(name.to_string());
        self
    }

    /// Attach the built-in Rust shell (R2+: statically linked policy
    /// crate) after backend setup, before the heads flush — so the
    /// shell's output listeners see every output creation.
    #[cfg(feature = "hybrid-r1")]
    pub fn with_shell(
        mut self,
        background_color: u32,
        factory: impl FnOnce(u32) -> Box<dyn crate::ctx::ShellApp> + 'static,
    ) -> Self {
        self.shell = Some((background_color, Box::new(factory)));
        self
    }

```

**A.17b `CompositorBuilder::build()`.**

```rust
    pub fn build(self) -> Result<Compositor, CompositorError> {
        // NOT install_stderr_handlers() any more: by the time build()
        // runs, the frontend's LogContext has installed the
        // scope-aware pair, and re-installing here would silently
        // disconnect the log file and the flight recorder.  The
        // fallback pair is installed by the frontend before its first
        // line instead (and by the unit harness for tests).
        //
        // Checked before anything is created: a builder with no backend
        // can never produce a running compositor, and failing here
        // keeps the error off the teardown paths below.
        if self.backends.is_empty() {
            return Err(CompositorError::BackendLoad);
        }
        let ctx = Ctx::new();

        // SAFETY: plain constructor calls; null-checked before use.
        let display = unsafe { weston_sys::wl_display_create() };
        let Some(display) = NonNull::new(display) else {
            ctx.teardown();
            return Err(CompositorError::DisplayCreate);
        };
        ctx.inner.display.set(display.as_ptr());

        // Borrowed, not created here: the frontend owns the log context
        // because C does (main.c:4499 -- it must exist before the very
        // first log line, which is long before any compositor).  It
        // outlives this Compositor, so Drop no longer destroys it.
        let log_ctx = self.log_ctx.map_or(std::ptr::null_mut(), |c| c);
        if log_ctx.is_null() {
            // SAFETY: display was just created and has no clients.
            unsafe { weston_sys::wl_display_destroy(display.as_ptr()) };
            ctx.teardown();
            return Err(CompositorError::LogCtxCreate);
        }

        // SAFETY: display and log_ctx are live; user_data stays null in
        // the pure-Rust binary (the C frontend owns that slot only
        // during the hybrid phases — plan §7 R1 hazard note).
        let compositor = unsafe {
            weston_sys::weston_compositor_create(
                display.as_ptr(),
                log_ctx,
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if compositor.is_null() {
            // SAFETY: reverse creation order; nothing else refers to
            // the display.  The log context belongs to the frontend.
            unsafe {
                weston_sys::wl_display_destroy(display.as_ptr());
            }
            ctx.teardown();
            return Err(CompositorError::CompositorCreate);
        }
        ctx.inner.compositor.set(compositor);

        // C weston_compositor_init_config, between compositor create
        // and backend load: [keyboard] xkb names (mandatory for
        // keyboard-creating backends — VNC strdup's them, vnc.c:1260),
        // repeat rates, vt-switching, [core] repaint-window.
        // SAFETY: compositor live; xkb_rule_names is read by the call.
        // libweston STORES the passed pointers and frees them at
        // compositor destroy, so it gets C-owned strdup copies; nulls
        // let it fill its own defaults (evdev/pc105/us, input.c:3954).
        let xkb_ok = unsafe {
            let mut names: weston_sys::xkb_rule_names = std::mem::zeroed();
            names.rules = c_strdup_opt(self.keyboard.rules.as_deref());
            names.model = c_strdup_opt(self.keyboard.model.as_deref());
            names.layout = c_strdup_opt(self.keyboard.layout.as_deref());
            names.variant = c_strdup_opt(self.keyboard.variant.as_deref());
            names.options = c_strdup_opt(self.keyboard.options.as_deref());
            let r = weston_sys::weston_compositor_set_xkb_rule_names(compositor, &mut names);
            if r < 0 {
                // libweston bails before `ec->xkb_names = *names`
                // (input.c: only xkb_context_new can fail there), so the
                // strdup'd copies never changed owner — free them or the
                // C-owned allocations leak on this path.
                for p in [
                    names.rules,
                    names.model,
                    names.layout,
                    names.variant,
                    names.options,
                ] {
                    libc::free(p.cast_mut().cast());
                }
            }
            r >= 0
        };
        if !xkb_ok {
            // C weston_compositor_init_config returns immediately here,
            // so none of the settings below are applied or logged.
            // SAFETY: reverse creation order, same as the paths above.
            unsafe {
                weston_sys::weston_compositor_destroy(compositor);
                weston_sys::wl_display_destroy(display.as_ptr());
            }
            ctx.teardown();
            return Err(CompositorError::CompositorCreate);
        }
        // SAFETY: compositor live; plain POD field writes on it.
        unsafe {
            (*compositor).kb_repeat_rate = self.keyboard.repeat_rate.unwrap_or(40);
            (*compositor).kb_repeat_delay = self.keyboard.repeat_delay.unwrap_or(400);
            (*compositor).vt_switching = self.keyboard.vt_switching.unwrap_or(true);
            if let Some(msec) = self.repaint_window_msec {
                if !(-10..=1000).contains(&msec) {
                    log::log_line(&format!("Invalid repaint_window value in config: {msec}"));
                } else {
                    (*compositor).repaint_msec = msec;
                }
            }
            log::log_line(&format!(
                "Output repaint window is {} ms maximum.",
                (*compositor).repaint_msec
            ));
            // C main.c:1203, immediately after the repaint window and
            // before the backends load — the colour manager must exist
            // before any output is configured, since that is when
            // profiles and EOTF modes are applied.
            if self.color_management {
                if weston_sys::weston_compositor_load_color_manager(compositor) < 0 {
                    return Err(CompositorError::ColorManager);
                }
                ctx.inner.use_color_manager.set(true);
            }
            // main.c:4653 `wet.compositor->multi_backend = backends &&
            // strchr(backends, ',')`, set before load_backends.  Load
            // bearing in libweston: output_accumulate_damage drops the
            // core buffer reference early ("the backend has seen it")
            // only when a single backend is in play — with two, that
            // optimization releases buffers the second backend still
            // needs (compositor.c:3405).
            (*compositor).multi_backend = self.backends.len() > 1;
        }

        let mut comp = Compositor {
            ctx: ctx.clone(),
            policy_names: self.policy.rules.iter().map(|r| r.name.clone()).collect(),
            heads_changed: None,
            protocol_scope: std::ptr::null_mut(),
            protologger: std::ptr::null_mut(),
            frontend_listeners: Vec::new(),
            signal_sources: Vec::new(),
            socket_bound: None,
            exit_code: 0,
        };

        // C main.c:4618, in the same place -- right after the
        // compositor exists and before anything can talk protocol.
        // Unconditional, as in C: the handler bails on an unsubscribed
        // scope, so an idle "proto" dump costs one branch per message.
        // SAFETY: log_ctx live (the frontend owns it and outlives us);
        // the strings are static and libweston copies what it keeps.
        comp.protocol_scope = unsafe {
            weston_sys::weston_log_ctx_add_log_scope(
                log_ctx,
                c"proto".as_ptr(),
                c"Wayland protocol dump for all clients.\n".as_ptr(),
                None,
                None,
                std::ptr::null_mut(),
            )
        };
        if !comp.protocol_scope.is_null() {
            // The scope rides in as user_data: this is the one
            // trampoline that needs no Ctx at all, and passing the
            // pointer keeps it working even during teardown ordering.
            // SAFETY: display live; the logger is destroyed in Drop
            // before the scope it points at.
            comp.protologger = unsafe {
                weston_sys::wl_display_add_protocol_logger(
                    display.as_ptr(),
                    Some(crate::debug::protocol_log_fn),
                    comp.protocol_scope.cast(),
                )
            };
        }

        // C main.c:4626: --debug.  After the protocol logger, as C has
        // it, and before backends load so an early capture attempt is
        // already covered.
        if self.debug_protocol {
            comp.frontend_listeners.push(crate::debug::enable(&ctx));
        }

        // Signal sources BEFORE backend load, as C installs signals[]
        // right after display creation: wl_event_loop_add_signal blocks
        // the signal via sigprocmask on THIS thread, and threads spawned
        // later inherit that mask.  The VNC backend spawns neatvnc/aml
        // worker threads during load — install after it, and a
        // process-directed SIGTERM can be delivered to a worker (which
        // never blocked it) and kill the process with the default action
        // instead of reaching the signalfd (seen live: CI SIGTERM death
        // in the vnc valgrind leg while the same run passed locally —
        // it is a per-delivery race).
        comp.install_signal_sources()?;

        // Sync-tier heads-changed listener (§3e names this tier: outputs
        // must be configured inside the flush).  It touches only wrapper
        // state + the output policy — a pure data lookup, no app
        // borrow (A3).
        let policy = self.policy.clone();
        // The DRM branch needs two settings the policy does not carry
        // (they are backend-level, not per-output): --current-mode and
        // [core] require-outputs.  Captured here rather than looked up
        // in the handler, so the sync-tier A3 proof stays "wrapper
        // state only, no app borrow".
        let drm_runtime = DrmRuntime {
            current_mode: self
                .backends
                .iter()
                .any(|b| matches!(b, BackendSpec::Drm(o) if o.current_mode)),
            require_outputs: self.require_outputs,
        };
        let listener = Listener::new(
            "heads_changed",
            false,
            Box::new(move |ctx, _data| heads_changed(ctx, &policy, drm_runtime)),
        );
        // SAFETY: the compositor outlives the listener (Compositor owns
        // both; drop detaches before destroy).
        unsafe {
            weston_sys::weston_compositor_add_heads_changed_listener(
                compositor,
                listener.raw_ptr(),
            );
        }
        listener.mark_attached();
        comp.heads_changed = Some(listener);

        // Frontend mirror listeners (C wet_compositor.output_created_
        // listener at main.c:4245 — L6 — and the wet_head_tracker
        // resized listener — L2).  Installed unconditionally like C;
        // both no-op unless an [[output]] rule carries mirror-of=.
        // Sync tier: pure wrapper/policy work, no app borrow (A3) —
        // enabling the mirror nests inside the source's
        // output_created emission exactly as C's handler does.
        let policy = self.policy.clone();
        let out_created = Listener::new(
            "frontend.output_created",
            false,
            Box::new(move |ctx, data| {
                if let Some(o) = NonNull::new(data.cast::<weston_sys::weston_output>()) {
                    mirror_on_output_created(ctx, &policy, o);
                }
            }),
        );
        let policy = self.policy.clone();
        let out_resized = Listener::new(
            "frontend.output_resized",
            false,
            Box::new(move |ctx, data| {
                if let Some(o) = NonNull::new(data.cast::<weston_sys::weston_output>()) {
                    mirror_on_output_resized(ctx, &policy, o);
                }
            }),
        );
        // SAFETY: compositor live; both signals are fields on it.
        unsafe {
            weston_sys::wsys_wl_signal_add(
                &raw mut (*compositor).output_created_signal,
                out_created.raw_ptr(),
            );
            weston_sys::wsys_wl_signal_add(
                &raw mut (*compositor).output_resized_signal,
                out_resized.raw_ptr(),
            );
        }
        for l in [out_created, out_resized] {
            l.mark_attached();
            // Owned by the Compositor, NOT ctx.own_listener: these
            // nodes sit on signal lists INSIDE the weston_compositor
            // struct, and without a shell attached ctx.teardown() only
            // runs after weston_compositor_destroy has freed it —
            // detaching then is a write into freed memory (caught by
            // the r0 valgrind gate).  Drop detaches them next to
            // heads_changed, before the destroy.
            comp.frontend_listeners.push(l);
        }

        for spec in &self.backends {
            match spec {
                BackendSpec::Headless(opts) => comp.load_headless(self.renderer, opts)?,
                BackendSpec::Vnc(opts) => comp.load_vnc(self.renderer, opts)?,
                BackendSpec::X11(opts) => comp.load_x11(self.renderer, opts)?,
                BackendSpec::Wayland(opts) => comp.load_wayland(self.renderer, opts)?,
                BackendSpec::Pipewire(opts) => comp.load_pipewire(self.renderer, opts)?,
                BackendSpec::Drm(opts) => comp.load_drm(self.renderer, opts)?,
            }
        }

        // main.c:4660 — installs the (no-op) color manager and finalizes
        // backend setup; weston_output_init dereferences the color
        // manager, so this MUST precede the heads flush.
        // SAFETY: compositor live, backends just loaded.
        let ok = unsafe { weston_sys::weston_compositor_backends_loaded(compositor) };
        if ok < 0 {
            return Err(CompositorError::BackendLoad);
        }

        // C main.c:4633: the recorder's dump binding, only when a
        // recorder exists.  A *debug* binding, so it sits behind
        // libweston's debug-binding modifier and cannot collide with
        // anything a client or the shell binds.
        if !self.flight_rec.is_null() {
            // SAFETY: compositor live; the subscriber belongs to the
            // frontend's LogContext, which outlives the compositor, so
            // the data pointer stays valid for every firing.
            unsafe {
                weston_sys::weston_compositor_add_debug_binding(
                    compositor,
                    KEY_D,
                    Some(crate::log::flight_rec_binding),
                    self.flight_rec.cast(),
                );
            }
        }

        let has_shell = self.shell.is_some();
        #[cfg(feature = "hybrid-r1")]
        if let Some((bg, factory)) = self.shell
            && !crate::shell_init::attach_shell_native(&ctx, bg, factory)
        {
            return Err(CompositorError::ShellInit);
        }
        // C wet_shell_init ends by calling screenshooter_create
        // (shell.c:2232); at R2e the frontend owns that call — same
        // registrations at the same point in startup (plan §4).
        if has_shell {
            let auth = crate::screenshooter::create(&ctx);
            // Its node lives on a signal inside the compositor struct:
            // Compositor-owned, detached in Drop before destroy.
            comp.frontend_listeners.push(auth);
        }

        // Bring-up order from here matches C (main.c:4671-4767): flush
        // → init_failed check → socket → xwayland → wake.  The one
        // deliberate divergence is above, documented at `with_shell`:
        // the shell attaches BEFORE the flush (C: after the socket) so
        // its output listeners see every output creation.  The socket
        // deliberately binds AFTER the flush, as in C (main.c:4706) —
        // PR16-C6 had it the other way around, which made the socket's
        // on-disk existence a false proxy for "outputs are configured"
        // (exactly the inference test harnesses want to draw).

        // The C frontend flushes heads once after backend setup
        // (main.c:4671); our sync-tier listener enables outputs here.
        ctx.with_depth(|| {
            // SAFETY: compositor is live.
            unsafe { weston_sys::weston_compositor_flush_heads_changed(compositor) };
        });

        // C wet_main: `if (wet.init_failed) goto out` immediately after
        // the flush — an output that could not be created, configured
        // or enabled is a startup failure, never a compositor that
        // comes up quietly short of outputs.
        if ctx.inner.init_failed.get() {
            return Err(CompositorError::OutputInit);
        }

        if self.socket {
            let bound = match &self.socket_name {
                Some(name) => {
                    let cname =
                        CString::new(name.as_str()).map_err(|_| CompositorError::SocketAdd)?;
                    // SAFETY: display live; name is a valid C string for
                    // the call (libwayland copies it).
                    let r = unsafe {
                        weston_sys::wl_display_add_socket(display.as_ptr(), cname.as_ptr())
                    };
                    if r != 0 {
                        return Err(CompositorError::SocketAdd);
                    }
                    name.clone()
                }
                None => {
                    // SAFETY: display is live; the returned name is a
                    // borrowed C string, copied at the fence (§3h).
                    let name = unsafe { weston_sys::wl_display_add_socket_auto(display.as_ptr()) };
                    if name.is_null() {
                        return Err(CompositorError::SocketAdd);
                    }
                    // SAFETY: non-null return is a valid NUL-terminated
                    // string owned by the display.
                    unsafe { std::ffi::CStr::from_ptr(name) }
                        .to_string_lossy()
                        .into_owned()
                }
            };
            crate::log::log_line(&format!("westonite: wayland socket {bound}"));
            comp.socket_bound = Some(bound);
        }

        // C wet_main loads xwayland after the socket and shell, before
        // wake ("Load xwayland before other modules", main.c:4717);
        // failure is fatal (goto out).
        if let Some(xserver) = self.xwayland {
            crate::xwayland::load(&ctx, xserver)?;
        }

        // SAFETY: compositor is live.
        unsafe { weston_sys::weston_compositor_wake(compositor) };

        Ok(comp)
    }
}

/// How `create_windowed_heads` numbers the default-named heads once
/// named `[output]` sections have consumed some of `count` — the one
/// place C's two windowed create_head loops disagree (PR27-C1):
///
/// * x11 (main.c:4013): `for (i = output_count; i < option_count; …)`
///   — defaults CONTINUE after the named count (`X-1` + `screen1`);
/// * wayland (main.c:4152): `count` is decremented per named head and
///   the default loop runs `for (i = 0; i < count; …)` — defaults
///   number FROM ZERO (`WL-1` + `wayland0`).
enum DefaultHeadNumbering {
    AfterNamed,
    FromZero,
}
```

**A.17c `Compositor`, `install_signal_sources`, `run`, `Drop`, the signal callbacks (two excerpts; the backend loaders sit between them).**

```rust
pub struct Compositor {
    ctx: Ctx,
    /// `[[output]]` section names in file order — the windowed loaders
    /// filter them by prefix to decide which heads to create, as C
    /// walks the config sections (load_x11_backend / load_wayland_backend).
    policy_names: Vec<String>,
    heads_changed: Option<Listener>,
    /// C `protocol_scope` + `protologger` (main.c:4618): the "proto"
    /// dump.  Installed unconditionally, as C does — it costs nothing
    /// until someone subscribes to the scope.  Owned here because the
    /// logger belongs to the display and the scope must die before the
    /// frontend's log context does.
    protocol_scope: *mut weston_sys::weston_log_scope,
    protologger: *mut weston_sys::wl_protocol_logger,
    /// Frontend listeners on compositor-embedded signals (mirror L6 +
    /// L2): detached in Drop before weston_compositor_destroy.
    frontend_listeners: Vec<Listener>,
    signal_sources: Vec<*mut weston_sys::wl_event_source>,
    socket_bound: Option<String>,
    exit_code: i32,
}

impl Compositor {
    pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// The wayland socket name bound in build(), if any.
    pub fn socket_name(&self) -> Option<&str> {
        self.socket_bound.as_deref()
    }

    /// Watch an autolaunched client: when `watch` and the pid exits,
    /// the compositor terminates (C execute_autolaunch + sigchld).
    pub fn set_autolaunch(&self, pid: i32, watch: bool) {
        self.ctx.inner.autolaunch.set(Some((pid, watch)));
    }

```
```rust
    /// Install the SIGTERM/SIGUSR2/SIGCHLD event-loop sources (main.c
    /// signals[]) and the sigaction-routed SIGINT (main.c:4552-4567).
    /// Called from build() BEFORE backend load — the sigprocmask that
    /// backs the signalfd must be in place before any backend spawns
    /// threads (they inherit the mask; see the call site) — which also
    /// puts it before any client spawn (SIGCHLD reaping/watch never
    /// misses an early exit).
    fn install_signal_sources(&mut self) -> Result<(), CompositorError> {
        let display = self.ctx.inner.display.get();
        // SAFETY: display live; loop borrowed for source installation.
        let ev_loop = unsafe { weston_sys::wl_display_get_event_loop(display) };

        // Signal-driven termination, as main.c installs for
        // SIGTERM/SIGUSR2.  SIGINT is deliberately NOT a loop source:
        // C's comment (main.c:4552) — a signalfd-caught SIGINT is
        // invisible to gdb, so Ctrl+C in a debugger would "stop"
        // weston by exiting it cleanly.  Instead a plain sigaction
        // handler re-raises SIGUSR2 (below), which IS on the loop.
        // SAFETY: display stays valid while the loop runs; the sources
        // are destroyed with the display.
        let s1 = unsafe {
            weston_sys::wl_event_loop_add_signal(
                ev_loop,
                libc::SIGTERM,
                Some(on_term_signal),
                display.cast(),
            )
        };
        // SAFETY: as s1 — same display, same loop.
        let s2 = unsafe {
            weston_sys::wl_event_loop_add_signal(
                ev_loop,
                libc::SIGUSR2,
                Some(on_term_signal),
                display.cast(),
            )
        };
        // SIGCHLD: reap clients; terminate on watched-autolaunch exit
        // (C main.c sigchld_handler + autolaunch watch).
        // SAFETY: as s1 — same display, same loop.
        let s3 = unsafe {
            weston_sys::wl_event_loop_add_signal(
                ev_loop,
                libc::SIGCHLD,
                Some(on_sigchld),
                display.cast(),
            )
        };
        if s1.is_null() || s2.is_null() || s3.is_null() {
            // Remove whichever source did install, or it would stay
            // registered (and leak) past this failed attempt.
            for s in [s1, s2, s3] {
                if !s.is_null() {
                    // SAFETY: source just created on this loop, not yet
                    // tracked anywhere else.
                    unsafe { weston_sys::wl_event_source_remove(s) };
                }
            }
            return Err(CompositorError::SignalSource);
        }
        // Removed in Drop, as main.c removes its signals[] sources.
        self.signal_sources.push(s1);
        self.signal_sources.push(s2);
        self.signal_sources.push(s3);

        // C main.c:4562-4567: SIGINT through plain sigaction, handler
        // re-raises SIGUSR2 ("xwayland uses SIGUSR1") — the one signal
        // gdb can still catch.  C ignores sigaction's return; so do
        // we.  Never restored, as in C: after teardown a SIGINT still
        // raises SIGUSR2, whose disposition is then the default again.
        // SAFETY: installs an async-signal-safe handler (bare raise);
        // the zeroed struct + explicit fields match C's
        // sa_handler/empty-mask/flags-0 setup exactly.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = sigint_helper as *const () as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            action.sa_flags = 0;
            libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
        }

        // C main.c:4570: block SIGUSR1 up front, unconditionally —
        // "Xwayland uses SIGUSR1 for communicating with weston" — so
        // that (a) a stray SIGUSR1 can't kill the process with its
        // default action, and (b) backend worker threads spawned later
        // inherit the block.  Same placement rationale as the signalfd
        // masks above.
        // SAFETY: plain sigset syscalls on a stack-local set.
        unsafe {
            let mut mask: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGUSR1);
            libc::pthread_sigmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut());
        }
        Ok(())
    }

    /// Run until terminated (SIGTERM/SIGUSR2, or SIGINT via the
    /// sigaction reroute).  Returns the process exit
    /// code (0 on clean signal-driven shutdown — the Phase-1 contract).
    pub fn run(&mut self) -> i32 {
        let display = self.ctx.inner.display.get();
        // Deliberately NOT wrapped in `with_depth`, unlike every other
        // outbound call: this one *is* the event loop, so holding the
        // counter at 1 for its whole lifetime would mean the counter
        // never returns to zero while the compositor runs, and A4's
        // drain-at-the-edge could only fire after the loop exits.  That
        // was the R0 shape, and it made every deferred-tier event
        // (focus-after-close, busy-cursor end, GrabEnded, the Gone
        // policies) land in one batch at shutdown, while retired boxes
        // piled up in the pending-drop list for the whole session.
        // With the base depth at zero, each trampoline's own wrap goes
        // 0→1→0 and drains at its edge — which is exactly where the C
        // code ran the same handler.
        //
        // SAFETY: the run loop dispatches into our trampolines; every
        // one re-enters through Ctx::current + with_depth.  Full audit
        // of the entry points that deliberately do NOT wrap:
        //   - the two event-loop signal callbacks below — they wrap
        //     their own bodies instead, so one handler is one drain;
        //   - the label / no-op vtable entries (curtain_get_label,
        //     curtain_committed, and the empty grab entries — the
        //     noop_* set plus touch_down/touch_frame) and the
        //     weston_log sink, which must stay unwrapped: draining from
        //     inside the log sink would run app handlers on a log call;
        //   - desktop::tramp_get_position, which answers from
        //     wrapper-held view state into out-params and makes no
        //     outbound call at all, so it has nothing to drain;
        //   - desktop::client_surfaces' nested `collect`, which is not
        //     an entry point on C's schedule at all: we hand it to
        //     weston_desktop_client_for_each_surface from inside an
        //     already-wrapped trampoline, and it only appends to a
        //     caller-owned Vec;
        //   - sigint_helper, which is not a loop entry point at all
        //     but a REAL async-signal handler (sigaction): nothing is
        //     allowed in its body but the one raise().
        // (Re-derive with a body-scoped scan for extern "C" fns whose
        // bodies contain no with_depth/with_ctx/guard_ctx, not a
        // fixed-size window — the window version missed two of these.)
        unsafe { weston_sys::wl_display_run(display) };
        self.exit_code
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        // Teardown order mirrors main.c's `out:` path; shutting_down
        // first so destroy-storm policy events are discarded (§3e).
        self.ctx.inner.shutting_down.set(true);
        // C wet_xwayland_destroy runs before weston_compositor_destroy
        // (the module must still be live for xserver_exited).
        crate::xwayland::teardown(&self.ctx);
        // Screenshooter: detach the per-client listener before the
        // display (and its clients) dies; the authority listener is in
        // frontend_listeners below (C screenshooter_destroy removes
        // both from the compositor's destroy emission).
        crate::screenshooter::teardown(&self.ctx);
        if let Some(l) = self.heads_changed.take() {
            l.detach();
        }
        for l in self.frontend_listeners.drain(..) {
            // Invariant: everything in this set was attached at creation
            // and stays attached until this very detach.  A listener
            // reporting otherwise was hand-attached via raw_ptr()
            // without mark_attached() (the PR36-C1 bug shape): detach()
            // would no-op and the pinned box below would be freed while
            // its node is still linked inside the live compositor.
            debug_assert!(
                l.is_attached(),
                "frontend listener dropped while not marked attached"
            );
            // SAFETY-relevant ordering: the signal lists these sit on
            // live inside the compositor struct, freed just below.
            l.detach();
        }
        // C main.c:4793/4803: the protocol logger first (it points at
        // the scope), then the scope -- both before the display, and
        // both before the frontend's log context destroys the log ctx
        // that owns the scope.
        // SAFETY: each is ours, created in build(), destroyed once.
        unsafe {
            if !self.protologger.is_null() {
                weston_sys::wl_protocol_logger_destroy(self.protologger);
                self.protologger = std::ptr::null_mut();
            }
            if !self.protocol_scope.is_null() {
                weston_sys::weston_log_scope_destroy(self.protocol_scope);
                self.protocol_scope = std::ptr::null_mut();
            }
        }
        for src in self.signal_sources.drain(..) {
            // SAFETY: sources created on this display's loop and still
            // owned by us; removed before the display dies (main.c order).
            unsafe { weston_sys::wl_event_source_remove(src) };
        }
        let compositor = self.ctx.inner.compositor.get();
        let display = self.ctx.inner.display.get();
        if !compositor.is_null() {
            self.ctx.with_depth(|| {
                // SAFETY: created in build(); destroy emits the
                // destroy storms our trampolines handle above.
                unsafe { weston_sys::weston_compositor_destroy(compositor) };
            });
        }
        self.ctx.teardown();
        // SAFETY: after compositor destroy; order per main.c.  The log
        // context is NOT destroyed here -- the frontend owns it and
        // outlives us, exactly as main.c destroys it after the display
        // (main.c:4819).
        unsafe {
            if !display.is_null() {
                weston_sys::wl_display_destroy(display);
            }
        }
    }
}

extern "C" fn on_sigchld(_signal: c_int, data: *mut c_void) -> c_int {
    crate::panic_barrier::guard("on_sigchld", || {
        // Event-loop callbacks run at the loop's base depth, which is
        // zero (see `run`), so the whole body takes the +1 itself: the
        // outbound calls below (xserver_exited, weston_log,
        // wl_display_terminate) would otherwise each return the counter
        // to zero and drain mid-reap, running shell handlers between
        // two waitpid iterations.  One wrap ⇒ one drain, at the edge.
        //
        // Resolved once, up front — but the reap must NOT depend on it.
        // A missing Ctx here would be a teardown-ordering bug (§3j; not
        // reachable today, since Drop removes the signal sources before
        // Ctx::teardown), and the one thing a SIGCHLD handler may never
        // skip is waitpid: bailing out would leave real zombies behind
        // for a wrapper-state problem.  Reap either way; only the
        // frontend bookkeeping and the depth wrap need the Ctx.
        let ctx = Ctx::current();
        debug_assert!(ctx.is_some(), "on_sigchld with no live Ctx");
        // Reap every exited child (C sigchld_handler's waitpid loop).
        let reap = || {
            loop {
                let mut status: c_int = 0;
                // SAFETY: plain waitpid; WNOHANG never blocks the loop.
                let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
                if pid <= 0 {
                    break;
                }
                // Redundant with the outer match by construction (ctx
                // is resolved once above and never changes mid-call);
                // kept deliberately (PR26-C2) so the loop body stands
                // alone — reaping must continue even when bookkeeping
                // is impossible, and this spells that out per pid.
                let Some(ctx) = ctx.as_ref() else { continue };
                if let Some((watched, watch)) = ctx.inner.autolaunch.get()
                    && pid == watched
                {
                    ctx.inner.autolaunch.set(None);
                    if watch {
                        crate::log::log_line("westonite: autolaunched client exited, terminating");
                        // SAFETY: data is the live wl_display registered
                        // with this source.
                        unsafe { weston_sys::wl_display_terminate(data.cast()) };
                    }
                    continue;
                }
                // Frontend-tracked children (C sigchld_handler's
                // wet_process walk): the Xwayland server (logged + relayed
                // to the module, which respawns on the next X connection)
                // and the screenshooter client (logged only).
                if crate::xwayland::handle_child_exit(ctx, pid, status) {
                    continue;
                }
                crate::screenshooter::handle_child_exit(ctx, pid, status);
            }
        };
        match ctx.as_ref() {
            Some(c) => c.with_depth(reap),
            None => reap(),
        }
    });
    1
}

extern "C" fn on_term_signal(signal: c_int, data: *mut c_void) -> c_int {
    // Async-safe enough: wl_event_loop signal sources deliver via
    // signalfd on the main loop, not in async signal context — which
    // is also why the weston_log below is safe, exactly as it is in
    // C's on_term_signal (main.c:831).
    crate::log::log_line(&format!("caught signal {signal}"));
    // Wrapped like on_sigchld now that the loop's base depth is zero
    // (see `run`).  wl_display_terminate only clears the loop's run
    // flag — it emits nothing and cannot re-enter us — so this is A4
    // hygiene rather than a live hazard.  A Ctx-less late signal still
    // terminates, just unwrapped: the display outlives the Ctx in
    // Compositor::drop, and dropping a SIGTERM on the floor would be a
    // worse failure than skipping a drain with nothing to drain.
    let Some(ctx) = Ctx::current() else {
        // SAFETY: as below; without a Ctx there is nothing to drain.
        unsafe { weston_sys::wl_display_terminate(data.cast()) };
        return 1;
    };
    ctx.with_depth(|| {
        // SAFETY: data is the live wl_display registered above.
        unsafe { weston_sys::wl_display_terminate(data.cast()) };
    });
    1
}

/// C sigint_helper (main.c:4406): runs in REAL async-signal context —
/// unlike every other extern "C" fn in this crate, which the event
/// loop calls synchronously.  No panic barrier, no logging, no Ctx:
/// nothing here but the one async-signal-safe raise(), exactly like C.
/// The re-raised SIGUSR2 is blocked by the loop source's sigprocmask,
/// so it lands in the signalfd and terminates via on_term_signal.
extern "C" fn sigint_helper(_sig: c_int) {
    // SAFETY: raise is async-signal-safe; SIGUSR2's route is the
    // event-loop source installed in install_signal_sources.
    unsafe { libc::raise(libc::SIGUSR2) };
}

/// The registry side of head/output tracking (§3b): registration and
/// destroy-listener attachment in one place, invoked from the sync-tier
/// heads-changed handler.  C simple_heads_changed, all three branches.
```

### A.18 `scripts/rust-smoke.sh` — copy as-is

```bash
#!/bin/bash
# Rust-migration R0 smoke (plan §7 R0 exit criterion): build the cargo
# workspace, run the fence-crate unit tests, then bring up the r0-smoke
# binary (compositor + headless backend + noop renderer), SIGTERM it,
# and require a clean exit — plus a valgrind pass over the same run.
# Runs inside the containers/Containerfile.build image.
set -euo pipefail

cd /src

export XDG_RUNTIME_DIR=/tmp/xdg
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"

fail() { echo "FAIL: $1" >&2; exit 1; }

# Poll a log file for a marker instead of sleeping a fixed time: fast
# when startup is fast, and a real deadline (not a race) when it isn't.
wait_for_marker() { # <file> <marker> <deadline-halfseconds>
	local f=$1 marker=$2 tries=$3
	for _ in $(seq 1 "$tries"); do
		grep -q "$marker" "$f" 2>/dev/null && return 0
		sleep 0.5
	done
	return 1
}

echo "== rust 1: workspace builds"
# --examples alone selects ONLY example targets; --bins must be listed
# too or the westonite-rs binary the frontend legs run never builds.
cargo build --locked --workspace --examples --bins

echo "== rust 2: unit tests (fence fake-C harness D18, shell policy D20, config/spawn R2a)"
cargo test --locked -p weston --features testsupport
cargo test --locked -p westonite-shell
cargo test --locked -p westonite-config
cargo test --locked -p westonite-spawn

echo "== rust 3: r0-smoke runs headless and exits 0 on SIGTERM"
target/debug/examples/r0-smoke > /tmp/r0.log 2>&1 &
PID=$!
wait_for_marker /tmp/r0.log "Output 'headless' enabled" 20 \
	|| { cat /tmp/r0.log >&2; fail "headless output was not enabled"; }
kill -0 "$PID" || { cat /tmp/r0.log >&2; fail "r0-smoke died during startup"; }
kill -TERM "$PID"
wait "$PID" || { cat /tmp/r0.log >&2; fail "r0-smoke exited non-zero on SIGTERM"; }
grep -q "westonite-r0: clean exit (0)" /tmp/r0.log \
	|| { cat /tmp/r0.log >&2; fail "no clean-exit marker"; }

echo "== rust 4: same run under valgrind (0 errors, 0 definite leaks)"
valgrind --error-exitcode=42 --leak-check=full \
	--errors-for-leak-kinds=definite \
	target/debug/examples/r0-smoke > /tmp/r0-vg.log 2>&1 &
VPID=$!
# Same assertions as the plain leg: the valgrind pass must demonstrably
# reach the enabled-output state and exit through the clean path, or it
# has valgrinded nothing.
wait_for_marker /tmp/r0-vg.log "Output 'headless' enabled" 120 \
	|| { cat /tmp/r0-vg.log >&2; fail "valgrind leg: headless output was not enabled"; }
kill -0 "$VPID" || { cat /tmp/r0-vg.log >&2; fail "valgrind leg: died during startup"; }
kill -TERM "$VPID"
wait "$VPID" || { cat /tmp/r0-vg.log >&2; fail "valgrind reported errors or leaks"; }
grep -q "westonite-r0: clean exit (0)" /tmp/r0-vg.log \
	|| { cat /tmp/r0-vg.log >&2; fail "valgrind leg: no clean-exit marker"; }

echo "== rust 5: westonite-rs frontend headless (R2a) exits 0 on SIGTERM"
# Fresh log every run: weston opens --log in append mode, so a stale
# file from an earlier container run would satisfy the marker greps.
rm -f /tmp/rs.log /tmp/rs-vg.log
target/debug/westonite-rs --backend=headless --no-config --log=/tmp/rs.log &
PID=$!
wait_for_marker /tmp/rs.log "westonite: wayland socket" 20 \
	|| { cat /tmp/rs.log >&2; fail "frontend: no wayland socket"; }
grep -q "westonite-shell: Rust shell initialized" /tmp/rs.log \
	|| { cat /tmp/rs.log >&2; fail "frontend: shell marker missing"; }
kill -TERM "$PID"
wait "$PID" || { cat /tmp/rs.log >&2; fail "frontend exited non-zero on SIGTERM"; }

echo "== rust 6: frontend + autolaunch watch under valgrind (SIGCHLD path)"
printf '#!/bin/sh\nsleep 2\n' > /tmp/rs-stub && chmod +x /tmp/rs-stub
valgrind --error-exitcode=42 --leak-check=full \
	--errors-for-leak-kinds=definite \
	target/debug/westonite-rs --backend=headless --no-config \
	--log=/tmp/rs-vg.log -o autolaunch.path=/tmp/rs-stub \
	-o autolaunch.watch=true \
	|| { cat /tmp/rs-vg.log >&2; fail "frontend valgrind reported errors or leaks"; }
grep -q "autolaunched client exited, terminating" /tmp/rs-vg.log \
	|| { cat /tmp/rs-vg.log >&2; fail "frontend valgrind leg: watch exit not exercised"; }

echo "== rust 7: westonite-rs VNC backend (R2c) under valgrind"
# No VNC client here (that needs the PAM stack — the e2e leg covers
# it); this gates startup/shutdown of the VNC path.  Suppressions
# cover the verified upstream vnc-backend leaks only (see the .supp).
rm -f /tmp/rs-vnc.log
valgrind --error-exitcode=42 --leak-check=full \
	--errors-for-leak-kinds=definite \
	--suppressions=/src/scripts/valgrind-upstream-vnc.supp \
	target/debug/westonite-rs --backend=vnc --renderer=pixman \
	--port=59920 --disable-transport-layer-security --no-config \
	--log=/tmp/rs-vnc.log > /tmp/rs-vnc-vg.log 2>&1 &
VNCPID=$!
wait_for_marker /tmp/rs-vnc.log "westonite: wayland socket" 120 \
	|| { cat /tmp/rs-vnc.log /tmp/rs-vnc-vg.log >&2; fail "vnc frontend: no wayland socket"; }
# The socket now binds AFTER the heads flush (C's order, PR16-C6), so
# reaching it implies the outputs are configured.  Keep the explicit
# output wait anyway: it names the thing this leg actually gates (the
# VNC configure path -- vnc_output_set_size / resizeable /
# forced-normal transform) instead of inferring it from ordering.
wait_for_marker /tmp/rs-vnc.log "Output 'vnc' enabled" 120 \
	|| { cat /tmp/rs-vnc.log /tmp/rs-vnc-vg.log >&2; fail "vnc frontend: output not enabled"; }
grep -q "westonite-shell: Rust shell initialized" /tmp/rs-vnc.log \
	|| { cat /tmp/rs-vnc.log >&2; fail "vnc frontend: shell marker missing"; }
kill -TERM "$VNCPID"
wait "$VNCPID" || { cat /tmp/rs-vnc-vg.log >&2; fail "vnc frontend valgrind reported errors or leaks"; }

echo "== rust 8: westonite-rs --xwayland (R2d) full lifecycle under valgrind"
# Lazy spawn → xdpyinfo roundtrip (displayfd + xserver_loaded + WM) →
# SIGTERM with the server still running (teardown's xserver_exited
# path).  Gates the new fence code in weston::xwayland end to end.
mkdir -p -m 1777 /tmp/.X11-unix
rm -f /tmp/rs-xw.log
valgrind --error-exitcode=42 --leak-check=full \
	--errors-for-leak-kinds=definite \
	target/debug/westonite-rs --backend=headless --xwayland --no-config \
	--socket=rs-xw-smoke --log=/tmp/rs-xw.log > /tmp/rs-xw-vg.log 2>&1 &
XWPID=$!
wait_for_marker /tmp/rs-xw.log "xserver listening on display" 120 \
	|| { cat /tmp/rs-xw.log /tmp/rs-xw-vg.log >&2; fail "xwayland: module did not listen"; }
grep -q "launching '/usr/bin/Xwayland'" /tmp/rs-xw.log \
	&& { cat /tmp/rs-xw.log >&2; fail "xwayland: eager spawn (must be lazy)"; }
XDISP=$(grep -oP "listening on display \K:[0-9]+" /tmp/rs-xw.log)
DISPLAY="$XDISP" xdpyinfo > /dev/null \
	|| { cat /tmp/rs-xw.log /tmp/rs-xw-vg.log >&2; fail "xwayland: xdpyinfo roundtrip failed"; }
wait_for_marker /tmp/rs-xw.log "created wm" 120 \
	|| { cat /tmp/rs-xw.log >&2; fail "xwayland: WM did not start (xserver_loaded path)"; }
kill -TERM "$XWPID"
wait "$XWPID" || { cat /tmp/rs-xw-vg.log >&2; fail "xwayland valgrind reported errors or leaks"; }

echo "== rust 9: westonite-rs --debug (R2g) attaches and detaches the authority cleanly"
# Debug build, so the frontend-listener assert in Compositor::drop is
# armed: an authority listener hand-attached without mark_attached()
# (the PR36-C1 bug) aborts here instead of freeing a still-linked node.
# Valgrind for the teardown itself.
rm -f /tmp/rs-dbg.log
valgrind --error-exitcode=42 --leak-check=full \
	--errors-for-leak-kinds=definite \
	target/debug/westonite-rs --backend=headless --no-config --debug \
	--socket=rs-dbg-smoke --log=/tmp/rs-dbg.log > /tmp/rs-dbg-vg.log 2>&1 &
DBGPID=$!
wait_for_marker /tmp/rs-dbg.log "westonite-shell: Rust shell initialized" 120 \
	|| { cat /tmp/rs-dbg.log /tmp/rs-dbg-vg.log >&2; fail "--debug: shell marker missing"; }
kill -TERM "$DBGPID"
wait "$DBGPID" \
	|| { cat /tmp/rs-dbg.log /tmp/rs-dbg-vg.log >&2; \
	     fail "--debug: non-zero exit (listener assert or valgrind errors)"; }

echo "ALL RUST SMOKE TESTS PASSED"
```

### A.18a `scripts/rust-asan-smoke.sh` — copy as-is

```bash
#!/bin/bash
# ASAN leg of the R0 exit criterion (plan §7 R0, D18): build the
# r0-smoke binary with AddressSanitizer and require the same clean
# SIGTERM exit.  Rust sanitizers are nightly-only, so this leg uses a
# rustup nightly toolchain (build/CI validation tool only — shipped
# binaries stay on the EL10 rust-toolset per plan §6).
# Runs inside the containers/Containerfile.build image; needs network
# on first run (rustup + crates).
set -euo pipefail

cd /src

export RUSTUP_HOME="${RUSTUP_HOME:-/tmp/rustup}"
export CARGO_HOME="${ASAN_CARGO_HOME:-${CARGO_HOME:-/tmp/asan-cargo}}"
if ! command -v rustup >/dev/null 2>&1 && [ ! -x "$CARGO_HOME/bin/rustup" ]; then
	curl -sSf https://sh.rustup.rs | sh -s -- -y \
		--default-toolchain nightly --profile minimal --component rust-src
fi
export PATH="$CARGO_HOME/bin:$PATH"
# (grep without -q: -q's early exit can SIGPIPE rustup under pipefail
# and spuriously trigger the reinstall.)
rustup toolchain list | grep nightly >/dev/null || \
	rustup toolchain install nightly --profile minimal --component rust-src

export XDG_RUNTIME_DIR=/tmp/xdg
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"

echo "== asan: build r0-smoke with -Zsanitizer=address"
RUSTFLAGS="-Zsanitizer=address" cargo +nightly build --locked -Zbuild-std \
	--target x86_64-unknown-linux-gnu \
	--example r0-smoke --target-dir target/asan

echo "== asan: run + SIGTERM (leaks off: libweston keeps startup allocs)"
ASAN_OPTIONS=detect_leaks=0 \
	target/asan/x86_64-unknown-linux-gnu/debug/examples/r0-smoke \
	> /tmp/r0-asan.log 2>&1 &
PID=$!
sleep 3
kill -0 "$PID" || { cat /tmp/r0-asan.log >&2; echo "FAIL: died at startup"; exit 1; }
kill -TERM "$PID"
wait "$PID" || { cat /tmp/r0-asan.log >&2; echo "FAIL: non-zero exit"; exit 1; }
grep -q "westonite-r0: clean exit (0)" /tmp/r0-asan.log \
	|| { cat /tmp/r0-asan.log >&2; echo "FAIL: no clean-exit marker"; exit 1; }

echo "== asan: build westonite-rs (R2a frontend) with -Zsanitizer=address"
RUSTFLAGS="-Zsanitizer=address" cargo +nightly build --locked -Zbuild-std \
	--target x86_64-unknown-linux-gnu \
	-p westonite --target-dir target/asan

echo "== asan: frontend + autolaunch watch (SIGCHLD/teardown paths)"
rm -f /tmp/rs-asan.log
printf '#!/bin/sh\nsleep 2\n' > /tmp/rs-asan-stub && chmod +x /tmp/rs-asan-stub
ASAN_OPTIONS=detect_leaks=0 \
	target/asan/x86_64-unknown-linux-gnu/debug/westonite-rs \
	--backend=headless --no-config --log=/tmp/rs-asan.log \
	-o autolaunch.path=/tmp/rs-asan-stub -o autolaunch.watch=true \
	|| { cat /tmp/rs-asan.log >&2; echo "FAIL: frontend non-zero exit"; exit 1; }
grep -q "autolaunched client exited, terminating" /tmp/rs-asan.log \
	|| { cat /tmp/rs-asan.log >&2; echo "FAIL: watch exit not exercised"; exit 1; }

echo "ASAN SMOKE PASSED"
```

### A.18b `scripts/rust-stress-test.sh` — copy, then apply at R4

Delta (§11.1 item 3): at R4 the default `BIN` becomes `target/release/westonite-rs` and the header comment describes the Rust binary as the default target.

```bash
#!/bin/bash
# Destroy-storm stress test (R1 exit criterion, plan §7/D18): client
# add/kill churn against the hybrid build (C frontend + Rust shell),
# run under valgrind memcheck — the invalidation paths §3b/§3f
# introduce are exactly what this exercises.  (Rust ASAN is nightly-only
# and cannot instrument the C half of the hybrid; valgrind memchecks
# the whole process instead.  The pure-Rust ASAN leg lives in
# rust-asan-smoke.sh.)
#
# WESTONITE_BIN points the storms at a different compositor — set it to
# target/release/westonite-rs to stress the pure-Rust frontend.  Both
# targets matter and neither subsumes the other: the hybrid has always
# dispatched our trampolines from C's own wl_display_run (base depth
# zero, so §3f frees happen at trampoline edges), while the Rust
# frontend only started doing that when `Compositor::run` stopped
# holding the depth counter open for the loop's lifetime.
# Runs inside the build container after the meson+rust install
# (smoke-test.sh or: ninja install && rust-shell-install.sh, plus
# -De2e-test-client=true for wtest-client).
set -euo pipefail

cd /src
WTEST=${WTEST_CLIENT:-/src/build/tests/e2e/clients/wtest-client}
BIN=${WESTONITE_BIN:-westonite}
[ -x "$WTEST" ] || { echo "wtest-client missing (build with -De2e-test-client=true)"; exit 1; }
# Check the compositor up front: a bad WESTONITE_BIN would otherwise
# surface 20s later as "compositor socket never appeared" with a
# valgrind log that only says the exec failed.
command -v "$BIN" >/dev/null 2>&1 || { echo "compositor '$BIN' not found (WESTONITE_BIN)"; exit 1; }

export XDG_RUNTIME_DIR=/tmp/xdg-stress
mkdir -p -m 0700 "$XDG_RUNTIME_DIR"
SOCKET=stress-0
LOG=/tmp/stress-westonite.log
VG=/tmp/stress-valgrind.log
CLIENTS=/tmp/stress-clients.log
: > "$CLIENTS"

valgrind --error-exitcode=42 --leak-check=full \
	--errors-for-leak-kinds=definite --log-file="$VG" \
	"$BIN" --backend=headless --socket="$SOCKET" --log="$LOG" &
WPID=$!

for i in $(seq 1 40); do
	if [ -S "$XDG_RUNTIME_DIR/$SOCKET" ]; then break; fi
	sleep 0.5
done
[ -S "$XDG_RUNTIME_DIR/$SOCKET" ] || { echo "compositor socket never appeared"; cat "$LOG"; exit 1; }

export WAYLAND_DISPLAY="$SOCKET"

echo "== storm 1: sequential add/kill churn (surface add/remove + focus hunt)"
for i in $(seq 1 25); do
	"$WTEST" --size 200x100 --title "churn-$i" >>"$CLIENTS" 2>&1 &
	CPID=$!
	sleep 0.15
	kill -9 "$CPID" 2>/dev/null || true
	wait "$CPID" 2>/dev/null || true
done

echo "== storm 2: concurrent clients, staggered kills (focus churn)"
PIDS=()
for i in $(seq 1 6); do
	"$WTEST" --size 160x120 --title "pack-$i" >>"$CLIENTS" 2>&1 &
	PIDS+=($!)
	sleep 0.1
done
sleep 0.5
# Kill in non-creation order: middle, first, last, rest.
for idx in 3 0 5 1 4 2; do
	kill -9 "${PIDS[$idx]}" 2>/dev/null || true
done
for p in "${PIDS[@]}"; do wait "$p" 2>/dev/null || true; done
sleep 0.5

# NOTE: wtest-client has no transient/child mode; all three storms churn
# single xdg toplevels.  Transient (parent/child) teardown is covered by
# the e2e suite only.
echo "== storm 3: rapid short-lived toplevel churn (map/kill singles)"
for i in $(seq 1 10); do
	"$WTEST" --size 200x100 --title "single-$i" >>"$CLIENTS" 2>&1 &
	CPID3=$!
	sleep 0.15
	kill -9 "$CPID3" 2>/dev/null || true
	wait "$CPID3" 2>/dev/null || true
done

# The storms must have exercised something real: wtest-client prints
# "mapped: WxH" once its first configure is acked.  Zero mapped clients
# means the whole run tested nothing (shell rejecting surfaces, client
# protocol error) — fail instead of reporting a hollow valgrind-clean.
MAPPED=$(grep -c '^mapped:' "$CLIENTS" || true)
if [ "${MAPPED:-0}" -eq 0 ]; then
	echo "FAIL: no wtest-client ever mapped a surface (storms exercised nothing)"
	cat "$CLIENTS"
	kill -9 "$WPID" 2>/dev/null || true
	wait "$WPID" 2>/dev/null || true
	tail -20 "$LOG"
	exit 1
fi
echo "clients mapped during storms: $MAPPED"

kill -TERM "$WPID"
# Teardown deadline: a hung teardown must fail in minutes, not at the CI
# job timeout.  120s is generous even under valgrind.  Poll-then-reap
# instead of a background watchdog subshell: nothing lingers past the
# script and no stale-PID kill can fire after exit.
for _ in $(seq 1 240); do
	kill -0 "$WPID" 2>/dev/null || break
	sleep 0.5
done
if kill -0 "$WPID" 2>/dev/null; then
	kill -9 "$WPID" 2>/dev/null || true
	wait "$WPID" 2>/dev/null || true
	echo "FAIL: compositor teardown hung >120s"; tail -40 "$VG"; tail -20 "$LOG"; exit 1
fi
wait "$WPID" || { echo "FAIL: compositor exited non-zero (valgrind errors or crash)"; tail -40 "$VG"; tail -20 "$LOG"; exit 1; }
grep -q "ERROR SUMMARY: 0 errors" "$VG" || { echo "FAIL: valgrind errors"; tail -40 "$VG"; exit 1; }

echo "DESTROY-STORM STRESS PASSED (valgrind clean, $MAPPED clients mapped)"
```

### A.18c `scripts/rust-shell-install.sh` — copy as-is

```bash
#!/bin/bash
# Build the Rust shell plugin (R1 hybrid, plan §7) and install it as the
# shipping desktop-shell.so, replacing the meson-installed C shell.
# Runs inside the build container after `ninja install`.
#
# Set WESTONITE_C_ORACLE=1 to skip (keeps the C shell — the behavioral
# oracle, plan R-B).
set -euo pipefail

if [ "${WESTONITE_C_ORACLE:-0}" = "1" ]; then
	echo "WESTONITE_C_ORACLE=1: keeping the C desktop-shell.so"
	exit 0
fi

cd /src
cargo build --locked -p westonite-shell-plugin --release
MODDIR=/usr/lib64/westonite
install -m 755 target/release/libwestonite_shell_plugin.so \
	"$MODDIR/desktop-shell.so"
echo "installed Rust shell -> $MODDIR/desktop-shell.so"
```

### A.18d `scripts/rust-e2e-test.sh` — copy, then apply

Delta: until R2d, add a `-k` deselection of the tests the Rust binary cannot run yet (§9.1 gate); remove it at R2d.

```bash
#!/bin/bash
# Build the Rust frontend (westonite-rs) and run the e2e suite against
# it (plan §7 R2a/R2b/R2c/R2d).  Since R2d (xwayland) the Rust frontend
# runs the ENTIRE suite; the C frontend leg (e2e-test.sh) remains the
# oracle only for the not-yet-ported backends.
#
# The suite needs the meson-built test clients (wtest-client) and the
# VNC PAM stack, so this mirrors e2e-test.sh's setup with the Rust
# binary under test (TOML config mode).
# Runs inside the containers/Containerfile.build image, as root.
# Usage: rust-e2e-test.sh [results-dir]
set -euo pipefail

RESULTS="${1:-/tmp}"

cd /src
cargo build --locked --release -p westonite

# Test clients only — the C compositor is not under test here, but the
# suite's wtest clients come out of the meson tree.
if [ -f build/build.ninja ]; then
	meson configure build -De2e-test-client=true >/dev/null
else
	meson setup build --prefix=/usr -De2e-test-client=true >/dev/null
fi
ninja -C build >/dev/null

mkdir -p -m 1777 /tmp/.X11-unix

E2E_USER=e2e
E2E_PASSWORD=westonite-e2e
printf 'auth     required pam_unix.so\naccount  required pam_unix.so\n' \
	> /etc/pam.d/weston-remote-access
id -u "$E2E_USER" >/dev/null 2>&1 || useradd -m "$E2E_USER"
echo "$E2E_USER:$E2E_PASSWORD" | chpasswd

mkdir -p "$RESULTS/failures-rust-frontend"
chown -R "$E2E_USER" "$RESULTS/failures-rust-frontend"
touch "$RESULTS/e2e-rust-frontend.xml" && chown "$E2E_USER" "$RESULTS/e2e-rust-frontend.xml"

exec runuser -u "$E2E_USER" -- env \
	WESTONITE_BIN=/src/target/release/westonite-rs \
	WESTONITE_CONFIG_FORMAT=toml \
	WESTONITE_VNC_USER="$E2E_USER" \
	WESTONITE_VNC_PASSWORD="$E2E_PASSWORD" \
	WTEST_CLIENT=/src/build/tests/e2e/clients/wtest-client \
	WTEST_XCLIENT=/src/build/tests/e2e/clients/wtest-xclient \
	WESTONITE_E2E_ARTIFACTS="$RESULTS/failures-rust-frontend" \
	python3 -m pytest /src/tests/e2e -v -p no:cacheprovider \
		--junit-xml="$RESULTS/e2e-rust-frontend.xml"
```

### A.19 `scripts/rust-fence-check.sh` — copy as-is (R4 adds the `--all-features` walk)

```bash
#!/bin/bash
# Fence-rule checks (plan §2 "Fence rules", D17) — the mechanical half.
# Runs inside the build container (or anywhere with cargo + python3 + git).
#
#  1. Dependency-graph rule: no safe crate may depend on weston-sys,
#     directly or transitively except through `weston`/`westonite-spawn`.
#     Checked on the resolved dependency graph, and every workspace
#     member must be classified safe or unsafe — a new crate that is in
#     neither list fails the check instead of silently escaping it.
#  2. forbid(unsafe_code) present in every safe crate.
#  3. Committed bindings are in sync with the installed headers (D5).
#
# The cargo-public-api snapshot check (rule 2 of §2) is NOT yet wired
# into CI — a known gap, tracked in the plan §6; do not rely on it.
set -euo pipefail

if [ -f /src/Cargo.toml ]; then cd /src; else cd "$(dirname "$0")/.."; fi

# Crate classification, per plan §2.  Grows at R2 (westonite-config,
# westonite); update here AND in the plan when it does.  Fence 1 fails
# on any workspace member missing from both lists.
SAFE_CRATES=(westonite-shell westonite-config westonite)
UNSAFE_CRATES=(weston-sys weston westonite-shell-plugin westonite-spawn)

echo "== fence 1: dependency graph (safe crates never see weston-sys)"
META=$(mktemp)
trap 'rm -f "$META"' EXIT
cargo metadata --format-version 1 --locked > "$META"
python3 - "$META" "${#SAFE_CRATES[@]}" "${SAFE_CRATES[@]}" "${UNSAFE_CRATES[@]}" <<'EOF'
import json, sys

meta_path = sys.argv[1]
n_safe = int(sys.argv[2])
SAFE = sys.argv[3 : 3 + n_safe]
UNSAFE = sys.argv[3 + n_safe :]

meta = json.load(open(meta_path))
by_id = {p["id"]: p for p in meta["packages"]}
members = {by_id[m]["name"] for m in meta["workspace_members"]}
resolve = {n["id"]: n for n in meta["resolve"]["nodes"]}
name_to_id = {by_id[m]["name"]: m for m in meta["workspace_members"]}

bad = []

# Every workspace member must be classified.
unclassified = members - set(SAFE) - set(UNSAFE)
if unclassified:
    bad.append(
        "unclassified workspace crates (add to SAFE_CRATES or "
        f"UNSAFE_CRATES in rust-fence-check.sh AND the plan §2): "
        f"{sorted(unclassified)}"
    )

# The only crates allowed to reach weston-sys, even transitively.
FENCE = {"weston", "westonite-spawn"}

def reaches_sys(pkg_id, seen):
    """weston-sys reachable without passing through a fence crate?"""
    if pkg_id in seen:
        return False
    seen.add(pkg_id)
    for dep in resolve[pkg_id]["deps"]:
        dep_name = by_id[dep["pkg"]]["name"]
        if dep_name == "weston-sys":
            return True
        if dep_name in FENCE:
            continue  # reaching weston-sys through the fence is the design
        if dep["pkg"] in resolve and reaches_sys(dep["pkg"], seen):
            return True
    return False

for name in SAFE:
    pkg_id = name_to_id.get(name)
    if pkg_id is None:
        bad.append(f"safe crate {name} missing from workspace")
        continue
    if reaches_sys(pkg_id, set()):
        bad.append(f"safe crate {name} reaches weston-sys outside the fence")

if bad:
    print("\n".join(bad)); sys.exit(1)
print(f"ok ({len(SAFE)} safe crates, {len(members)} members classified)")
EOF

echo "== fence 2: forbid(unsafe_code) in safe crates"
for c in "${SAFE_CRATES[@]:-}"; do
	[ -z "$c" ] && continue
	# lib.rs for library crates, main.rs for the westonite binary.
	root="crates/$c/src/lib.rs"
	[ -f "$root" ] || root="crates/$c/src/main.rs"
	# Anchored to line start so a doc-comment mention cannot satisfy it.
	grep -q '^#!\[forbid(unsafe_code)\]' "$root" \
		|| { echo "FAIL: crates/$c missing forbid(unsafe_code)"; exit 1; }
	grep -q '^unsafe_code *= *"forbid"' "crates/$c/Cargo.toml" \
		|| { echo "FAIL: crates/$c Cargo.toml missing unsafe_code = \"forbid\" lint"; exit 1; }
done
echo "ok"

echo "== fence 3: committed bindings match the installed headers"
if command -v bindgen >/dev/null 2>&1 || [ -n "${FENCE_CHECK_BINDINGS:-}" ]; then
	# Regenerate to a scratch copy: the committed file (and its mtime,
	# which cargo's rerun-if-changed watches) must not be touched.
	REGEN=$(mktemp)
	trap 'rm -f "$META" "$REGEN"' EXIT
	REGEN_BINDINGS_OUT="$REGEN" ./scripts/regen-bindings.sh >/dev/null
	if ! diff -u crates/weston-sys/src/bindings.rs "$REGEN" >/dev/null; then
		echo "FAIL: bindings.rs is stale — run scripts/regen-bindings.sh and commit"
		exit 1
	fi
	echo "ok"
else
	# Local-dev convenience only.  CI sets FENCE_CHECK_BINDINGS=1 (and
	# the build container ships a pinned bindgen-cli), so the check can
	# never be skipped silently there.
	echo "skipped (bindgen CLI not installed; set FENCE_CHECK_BINDINGS=1 to force)"
fi

echo "ALL FENCE CHECKS PASSED"
```

### A.19a `scripts/regen-bindings.sh` — copy as-is

```sh
#!/bin/sh
# Regenerate crates/weston-sys/src/bindings.rs from the *installed*
# libweston 14 headers (plan §6, D5).  Run inside the build container:
#
#   docker run --rm -v "$PWD":/src westonite-build /src/scripts/regen-bindings.sh
#
# The output is committed; build.rs asserts the recorded libweston version
# still matches pkg-config at every build (the R-C tripwire).  Re-run this
# script — and re-verify the §3 header facts — whenever EPEL bumps weston.
#
# REGEN_BINDINGS_OUT=<path> writes there instead of the committed file
# (used by rust-fence-check.sh to diff without touching the tree).
set -eu

cd "$(dirname "$0")/.."
SYS=crates/weston-sys
OUT="${REGEN_BINDINGS_OUT:-$SYS/src/bindings.rs}"

# Pinned: the committed output embeds the bindgen version, so an unpinned
# tool would make header drift indistinguishable from tool drift.  Bump
# this and regenerate in one commit when moving to a newer bindgen.
BINDGEN_VERSION=0.72.1

command -v bindgen >/dev/null 2>&1 || {
    echo "bindgen CLI not found; installing with cargo (needs network)..." >&2
    cargo install bindgen-cli --version "$BINDGEN_VERSION" --locked
    PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
}

ACTUAL=$(bindgen --version)
[ "$ACTUAL" = "bindgen $BINDGEN_VERSION" ] || {
    echo "bindgen version mismatch: have '$ACTUAL', need 'bindgen $BINDGEN_VERSION'" >&2
    echo "(install with: cargo install bindgen-cli --version $BINDGEN_VERSION --locked)" >&2
    exit 1
}

MODVERSION=$(pkg-config --modversion libweston-14)
CFLAGS=$(pkg-config --cflags libweston-14 wayland-server pixman-1 libinput libevdev)

TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT

bindgen "$SYS/wrapper.h" \
    --allowlist-item 'wl_.*' \
    --allowlist-item 'WL_.*' \
    --allowlist-item 'weston.*|WESTON.*' \
    --allowlist-item 'wet_.*' \
    --allowlist-item 'wsys_.*' \
    --allowlist-item 'pixman_.*' \
    --allowlist-item 'xkb_.*' \
    --allowlist-item 'libinput_.*|LIBINPUT_.*' \
    --allowlist-item 'libevdev_event_code_from_name' \
    --allowlist-item 'EV_KEY' \
    --blocklist-function 'weston_vlog.*' \
    --blocklist-function 'weston_log_scope_vprintf' \
    --blocklist-function 'weston_log_set_handler' \
    --blocklist-function 'wl_resource_post_error_vargs' \
    --no-doc-comments \
    --default-enum-style=moduleconsts \
    --no-prepend-enum-name \
    -o "$TMP" \
    -- $CFLAGS -I"$SYS" \
    "-DARRAY_LENGTH(a)=(sizeof (a) / sizeof (a)[0])"
# ^ windowed-output-api.h's static inline uses weston's internal
#   ARRAY_LENGTH helper macro, which the RPM does not install; supply it
#   so the installed headers parse standalone.

{
    echo "// @generated by scripts/regen-bindings.sh — do not edit."
    echo "// libweston-modversion: $MODVERSION"
    echo "// bindgen: $ACTUAL"
    cat "$TMP"
} > "$OUT"

echo "wrote $OUT (libweston $MODVERSION)"
```

### A.20 `containers/Containerfile.build` — the Rust additions

Add to the `dnf -y install` list, after `seatd`: `rust cargo clippy rustfmt clang-libs clang-devel clang-tools-extra valgrind libasan`. Append after the libweston sanity marker:

```dockerfile
RUN pkg-config --exists 'libweston-14 >= 14.0.1' \
    && test -e /usr/lib64/libweston-14/headless-backend.so \
    && test -e /usr/lib64/libweston-14/xwayland.so \
    && test -e /usr/include/libweston-14/libweston/xwayland-api.h

# Rust toolchain marker (rust migration, phase R0): cargo + rustc must be
# usable, and libclang must be present for the bindgen regen script.
RUN cargo --version && rustc --version \
    && ls /usr/lib64/libclang.so* >/dev/null

# Pinned bindgen CLI (must match BINDGEN_VERSION in
# scripts/regen-bindings.sh): baked into the image so the fence-check
# bindings-drift gate (rust-fence-check.sh fence 3) actually runs in CI
# instead of skipping, and so regeneration is deterministic.
RUN cargo install bindgen-cli --version 0.72.1 --locked --root /usr/local \
    && rm -rf /root/.cargo/registry \
    && bindgen --version

WORKDIR /src
```

### A.21 `rpm/westonite.spec` — the R3 form (R1–R2 use the interim spec described in §8.1)

```spec
Name:           westonite
Version:        14.0.1
Release:        3%{?dist}
Summary:        Standalone Weston-based Wayland compositor

# Vendored weston sources (the C oracle, not packaged); see VENDOR.md.
# The shipped binary is the Rust port; see PROVENANCE.md.
License:        MIT
URL:            https://github.com/nhwalker/example-weston-standalone
Source0:        %{name}-%{version}.tar.gz
# Vendored cargo registry dependencies (generated by scripts/rpm-build.sh
# with `cargo vendor`): the %%build cargo run is offline.
Source1:        %{name}-vendor-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  pkgconfig(libweston-14) >= 14.0.1
BuildRequires:  pkgconfig(wayland-server)
BuildRequires:  pkgconfig(libinput)
BuildRequires:  pkgconfig(libevdev)
BuildRequires:  pkgconfig(pixman-1)
# Enables the -listenfd Xwayland flag; the build works without it
BuildRequires:  pkgconfig(xwayland)

# libweston runtime, backends, renderers and xwayland.so (EPEL 10)
Requires:       weston-libs%{?_isa} >= 14.0.1
Recommends:     xorg-x11-server-Xwayland

%description
Westonite is a Weston 14 based Wayland compositor: the frontend and the
desktop shell, written in Rust against the distribution's libweston 14
packages and renamed so it installs alongside the stock weston package.
It ships no helper clients (no panel, no on-screen keyboard, no lock
screen) and reads its configuration from westonite.toml.

%prep
%autosetup
# Unpack the vendored crates and point cargo at them (offline build).
tar -xzf %{SOURCE1}
mkdir -p .cargo
cat > .cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

%build
cargo build --release --offline --locked -p westonite

%install
install -D -m 755 target/release/westonite-rs %{buildroot}%{_bindir}/westonite
install -D -m 644 data/westonite.desktop \
        %{buildroot}%{_datadir}/wayland-sessions/westonite.desktop
install -D -m 644 westonite.toml.example \
        %{buildroot}%{_datadir}/doc/westonite/westonite.toml.example

%files
%license COPYING
%doc VENDOR.md PROVENANCE.md
%{_bindir}/westonite
%{_datadir}/wayland-sessions/westonite.desktop
%{_datadir}/doc/westonite/westonite.toml.example

%changelog
* <date> <packager> - 14.0.1-3
- Ship the Rust frontend as /usr/bin/westonite with the shell linked in;
  no shared objects are installed any more; configuration moves to
  westonite.toml.

* <date> <packager> - 14.0.1-2
- Ship the Rust desktop-shell plugin (westonite-shell-plugin) as
  desktop-shell.so; cargo deps vendored via Source1 for offline %%build.

* <date> <packager> - 14.0.1-1
- Initial package: weston 14.0.1 frontend + desktop-shell built against
  EPEL 10 libweston-14 (weston-libs), renamed to westonite, no helper
  clients, Xwayland enabled.
```
`scripts/rpm-build.sh` gains, after the `git archive`: `rm -rf /tmp/rpm/vendor-work && mkdir -p /tmp/rpm/vendor-work && cargo vendor --locked /tmp/rpm/vendor-work/vendor >/dev/null && tar -C /tmp/rpm/vendor-work -czf "/tmp/rpm/SOURCES/westonite-vendor-$VERSION.tar.gz" vendor` (from R1). At R3 `scripts/rpm-install-test.sh` adds `WESTONITE_CONFIG_FORMAT=toml` to the pytest environment and greps the Rust shell marker instead of `Loading module`.


### A.22 `.github/workflows/ci.yml` — the R2c-drm form; R3 moves the RPM steps under the `rust` job

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

      - name: Smoke tests against the C oracle shell (plan R-B)
        # Keeps the C shell exercised until R3 deletes it; also proves
        # the WESTONITE_C_ORACLE switch still selects the C build.
        run: docker run --rm -v "$PWD":/src -e WESTONITE_C_ORACLE=1
          westonite-build /src/scripts/smoke-test.sh

      - name: Run the e2e suite
        run: |
          mkdir -p test-results
          docker run --rm -v "$PWD":/src -v "$PWD/test-results":/results \
            westonite-build /src/scripts/e2e-test.sh /results

      - name: Destroy-storm stress test (valgrind, D18)
        # Reuses the build tree the e2e step left in the workspace mount
        # (configuring it from scratch when absent, so the step also
        # works in isolation); installs are container-local, so ninja
        # install + rust-shell-install must rerun here regardless.
        run: docker run --rm -v "$PWD":/src westonite-build sh -c
          'cd /src
           && if [ -f build/build.ninja ];
              then meson configure build -De2e-test-client=true >/dev/null;
              else meson setup build --prefix=/usr -De2e-test-client=true >/dev/null; fi
           && ninja -C build >/dev/null && ninja -C build install >/dev/null
           && /src/scripts/rust-shell-install.sh && /src/scripts/rust-stress-test.sh'

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

  # Rust migration (docs/rust-migration-plan.md).  Same build image; the
  # cargo workspace lives beside the meson tree until R3.
  rust:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build the CentOS Stream 10 + EPEL build image
        run: docker build -f containers/Containerfile.build -t westonite-build .

      - name: rustfmt
        run: docker run --rm -v "$PWD":/src -w /src westonite-build cargo fmt --check

      - name: clippy (deny warnings, unsafe-hygiene lints)
        run: docker run --rm -v "$PWD":/src -w /src westonite-build
          cargo clippy --locked --workspace --all-targets --features weston/testsupport -- -D warnings

      - name: Fence checks (plan §2/D17)
        # FENCE_CHECK_BINDINGS=1: the bindings-drift gate (fence 3) must
        # run here, never skip — the image ships the pinned bindgen CLI.
        run: docker run --rm -v "$PWD":/src -w /src -e FENCE_CHECK_BINDINGS=1
          westonite-build /src/scripts/rust-fence-check.sh

      - name: Build, unit tests, headless smoke, valgrind
        run: docker run --rm -v "$PWD":/src -w /src westonite-build /src/scripts/rust-smoke.sh

      - name: Rust frontend e2e subset (R2a — re-specified CLI/config + lifecycle/children)
        run: |
          mkdir -p test-results
          docker run --rm -v "$PWD":/src -v "$PWD/test-results":/results \
            westonite-build /src/scripts/rust-e2e-test.sh /results

      - name: Upload test results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: test-results-rust
          path: test-results/
          if-no-files-found: ignore

  # The DRM backend needs a KMS device, which no container and no
  # GitHub-hosted runner offers.  This job boots a VM that carries its
  # own kernel (ours, from the same EL10 content set as everything else)
  # and loads vkms inside it -- so the only thing asked of the runner is
  # /dev/kvm, and even that is optional.  docs/drm-testing.md explains
  # the route and, importantly, what vkms does *not* prove.
  #
  # A gate since the Rust DRM port landed: it now exercises the code we
  # own, so a red X here is ours to fix.  The C-oracle leg stays
  # alongside the Rust one until R3 deletes the C frontend.
  drm-vm:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build the CentOS Stream 10 + EPEL build image
        run: docker build -f containers/Containerfile.build -t westonite-build .

      - name: Build the DRM VM image
        run: docker build -f containers/Containerfile.drm-vm -t westonite-drm-vm .

      - name: DRM e2e inside the VM (Rust frontend, then the C oracle)
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
          # Rust first: it is the thing under test, and running it first
          # means a failure is reported against our port rather than
          # after a passing oracle leg has already eaten the clock.
          for frontend in rust c; do
            docker run --rm $KVM -v "$PWD":/src -v "$PWD/test-results":/results \
              westonite-drm-vm /src/scripts/drm-vm-test.sh /results "$frontend"
          done

      - name: Upload DRM VM results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: test-results-drm-vm
          path: test-results/
          if-no-files-found: ignore

  # Sanitizer leg (D18): Rust sanitizers are nightly-only, so this job
  # pulls a rustup nightly inside the same container.  Validation tool
  # only — shipped binaries stay on the EL10 rust-toolset (plan §6).
  rust-asan:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build the CentOS Stream 10 + EPEL build image
        run: docker build -f containers/Containerfile.build -t westonite-build .

      - name: ASAN smoke (r0-smoke under AddressSanitizer)
        run: docker run --rm -v "$PWD":/src -w /src westonite-build /src/scripts/rust-asan-smoke.sh
```

### A.23 `tests/e2e/support/compositor.py` — the TOML-mode additions (the head of the final file)

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


# Which config interface the binary under test speaks (R2a re-spec,
# plan §5): "ini" for the C frontend (the oracle), "toml" for the Rust
# frontend (westonite-rs).  Selected by the runner, not per test.
CONFIG_FORMAT = os.environ.get("WESTONITE_CONFIG_FORMAT", "ini")
CONFIG_NAME = "westonite.toml" if CONFIG_FORMAT == "toml" else "westonite.ini"

# Log line proving the shell is up: the C frontend loads a plugin, the
# Rust frontend's statically linked shell logs its marker.
SHELL_READY_PATTERN = (
    r"westonite-shell: Rust shell initialized"
    if CONFIG_FORMAT == "toml"
    else r"Loading module '.*/desktop-shell\.so'")


def ini_to_config(text):
    """Translate the simple `[section]`/`key=value` configs the tests
    are written in into the format under test.  For TOML: repeated
    sections become array-of-tables and values are typed (bool/int
    bare, everything else quoted) -- the same near-diagonal mapping
    documented for users (D11)."""
    if CONFIG_FORMAT == "ini":
        return text
    # ini section name -> TOML array-of-tables name.  The TOML model is
    # kebab-case throughout, so the one ini section spelled with an
    # underscore ([color_characteristics]) is renamed here too.
    array_sections = {"output": "output",
                      "remote-output": "remote-output",
                      "pipewire-output": "pipewire-output",
                      "color_characteristics": "color-characteristics"}
    lines = []
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("[") and s.endswith("]"):
            name = s[1:-1]
            lines.append(f"[[{array_sections[name]}]]"
                         if name in array_sections else s)
        elif "=" in s and not s.startswith("#"):
            key, value = s.split("=", 1)
            lines.append(f"{key.strip()} = {_toml_value(value.strip())}")
        else:
            lines.append(line)
    return "\n".join(lines) + "\n"


def _toml_value(value):
    if value in ("true", "false"):
        return value
    try:
        int(value)
        return value
    except ValueError:
        pass
    # Floats: the colour-characteristics keys are the first typed-float
    # values in the model, and without this they arrive quoted and fail
    # deserialization as "invalid type: string, expected f64".
    # `float()` also accepts "inf"/"nan", which are not TOML floats, so
    # require a digit.
    try:
        float(value)
        if any(c.isdigit() for c in value):
            return value
```

### A.23b `scripts/drm-vm-test.sh` — the `rust` frontend leg

Relative to the C plan: the usage comment reads `frontend: "c" (default, the oracle) or "rust"`; after `ninja -C build` add `if [ "$FRONTEND" = rust ]; then cargo build --locked --release -p westonite; fi`; after the test-client copies add `if [ "$FRONTEND" = rust ]; then cp /src/target/release/westonite-rs /vm/run/usr/local/bin/; fi`; select `BIN`/`FORMAT` per frontend and export `WESTONITE_BIN=$BIN` and `WESTONITE_CONFIG_FORMAT=$FORMAT` in the guest command. The full final file:

```bash
#!/bin/bash
# Run the DRM leg of the e2e suite inside a throwaway VM that has a real
# KMS device (vkms).  See docs/drm-testing.md for why this exists and
# what it does not prove.
#
# Runs inside the containers/Containerfile.drm-vm image, as root.
# Usage: drm-vm-test.sh [results-dir] [frontend]
#   frontend: "c" (default, the oracle) or "rust"
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
if [ "$FRONTEND" = rust ]; then
	cargo build --locked --release -p westonite
fi

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
if [ "$FRONTEND" = rust ]; then
	cp /src/target/release/westonite-rs /vm/run/usr/local/bin/
fi
cp build/tests/e2e/clients/wtest-client /vm/run/usr/local/bin/ 2>/dev/null || true
cp build/tests/e2e/clients/wtest-xclient /vm/run/usr/local/bin/ 2>/dev/null || true

if [ "$FRONTEND" = rust ]; then
	BIN=/usr/local/bin/westonite-rs
	FORMAT=toml
else
	BIN=/usr/bin/westonite
	FORMAT=ini
fi

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
export WESTONITE_CONFIG_FORMAT=$FORMAT
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

### A.24 `scripts/valgrind-upstream-vnc.supp` — copy as-is

```text
# Known upstream leaks in the EPEL libweston 14.0.1 VNC backend,
# verified identical under the C frontend (PROVENANCE R2c entry):
# vnc_destroy() frees neither backend->xkb_rule_name.{rules,model,
# layout} (three strdups, vnc.c:1260-1262) nor backend->formats
# (pixel_format_get_array), and the output teardown leaks the
# weston_output_set_single_mode entry.  RPM-side, out of scope per the
# our-code-only decision (docs/e2e-test-plan.md §6 precedent).  This
# file lets the valgrind smoke legs keep gating OUR code at zero
# definite leaks.
#
# Scope, stated honestly: the rule below is not limited to those five
# blocks.  `...` + `obj:` matches any definite leak with a
# vnc-backend.so frame anywhere in its stack — in practice the whole
# allocation tree under weston_backend_init.  Nothing in the frontend
# allocates through that tree (our only entry into it is
# weston_compositor_load_backend, and libweston's own allocations carry
# no vnc-backend.so frame), so no leak of ours can hide here; but a
# *new* upstream leak would be swallowed silently rather than reported.
# Tightening this to per-leak `fun:` frames needs the leak stacks from a
# run with debuginfo for the backend installed.
{
   upstream-vnc-backend-startup-leaks
   Memcheck:Leak
   match-leak-kinds: definite
   ...
   obj:*vnc-backend.so*
}
```

### A.25 `westonite.toml.example` — copy, then apply

Deltas: under `[core]`, `modules` and `idle-time` are documented as refused (add `#idle-time = 300` with a NOT SUPPORTED comment); the `[rdp]` block is replaced by a comment stating that `[rdp]` is refused; `[libinput] touchscreen-calibrator`/`calibration-helper` keep their NOT SUPPORTED comment. The unit test `example_config_is_valid` must still pass, so every uncommented key must exist in the model.

```toml
# westonite.toml — annotated example configuration (plan §5, D9–D11).
#
# Search order (each location tried in turn, first hit wins):
#   $XDG_CONFIG_HOME/westonite.toml
#   $HOME/.config/westonite.toml
#   each dir in $XDG_CONFIG_DIRS (default /etc/xdg)
# Or pass --config=PATH / --no-config.  Any key here can also be set on
# the command line with `-o section.key=value` (repeatable; applied
# after the file, before dedicated flags).
#
# Unknown keys and type errors are STARTUP ERRORS with a line/column
# span — a typo'd key never silently does nothing.  Keys are
# kebab-case, matching the old ini's hyphenated spellings.

[core]
# Backend: drm (default), headless, x11, wayland, vnc, pipewire.
# `rdp` is NOT supported — deliberately dropped; use vnc for remote
# access.  Every other backend is available.
#backend = "headless"
# Or several: backends = ["drm", "vnc"] — the ini spelling
# backends = "drm,vnc" is accepted too, and so is a comma list in the
# singular key.  When both keys are present `backends` wins (C reads it
# first and falls back to `backend`).
# Renderer: auto (default), gl, pixman, noop.
#renderer = "auto"
# Require an input device before starting (default true).
#require-input = true
# Enable color management.
#color-management = false
# Start Xwayland support lazily.
#xwayland = false
# Deprecated aliases for renderer=; mutually exclusive, and neither may
# be combined with an explicit renderer.  (C honours the file spelling
# only on the headless backend; westonite honours it everywhere, like
# the matching --use-gl/--use-pixman flags.)
#use-gl = false
#use-pixman = false
# Draw a decoration border around headless outputs.  Headless-only, and
# it needs the GL renderer -- the backend refuses it on pixman/noop.
#output-decorations = false
# Log the pid and stop (SIGSTOP) at startup, before anything is
# created, so a debugger can attach.  Send SIGCONT to continue.  The
# --wait-for-debugger flag wins; this only applies when it is absent.
#wait-for-debugger = false
# Third-party frontend plugins.  NOT SUPPORTED: westonite refuses this
# key at startup rather than ignoring it.  The shell is built into the
# binary, and the two plugins that used to load this way (remoting,
# pipewire-output) are dropped.  The key is still recognised so the
# error can say so.
#modules = []

[shell]
# Background color, 0xAARRGGBB.  A quoted string keeps the hex spelling
# the ini used and is ALWAYS base-16, prefixed or not, exactly like the
# ini ("ff002244" works).  A bare 0xff002244 (TOML integer) works too,
# which is what `-o shell.background-color=0xff002244` produces.
#background-color = "0xff002244"

[keyboard]
#keymap-layout = "us"
#repeat-rate = 40
#repeat-delay = 400
#numlock-on = false

# Per-device libinput settings.  These reach libinput through the DRM
# backend and nowhere else, so on any other backend the section is
# accepted and inert — as it is in the C frontend.  A key a given device
# cannot do (tap on a mouse, left-handed on a keyboard) is skipped for
# that device, silently; a key with a *bad value* is a startup error.
[libinput]
#enable-tap = false
#tap-and-drag = false
#tap-and-drag-lock = false
#disable-while-typing = false
#natural-scroll = false
#left-handed = false
#middle-button-emulation = false
#rotation = 0
#accel-profile = "adaptive"    # flat | adaptive
#accel-speed = 0.0             # -1.0 .. 1.0
#scroll-method = "two-finger"  # two-finger | edge | button | none
#scroll-button = "BTN_RIGHT"   # evdev button name; needs scroll-method = "button"
# NOT SUPPORTED, refused at startup: the weston_touch_calibration
# protocol is deliberately dropped (no calibrator client ships with
# westonite).  calibration-helper only feeds it, so it is refused too.
#touchscreen-calibrator = false
#calibration-helper = ""

[autolaunch]
# Client to spawn once the compositor is up.  `watch = true` ties the
# session to it (compositor exits when the client exits).
#path = "/usr/bin/some-client"
#watch = false

[xwayland]
#path = "/usr/bin/Xwayland"

# Repeated ini [output] sections become TOML array-of-tables.  Each
# section configures the head whose name it matches; a section naming a
# head that never turns up configures nothing — but it is still
# checked at startup, so a typo in `transform` is fatal rather than
# quietly ignored.  mode/scale/transform/off are honoured on every
# backend.  Keys that only mean something on one backend —
# clone-of/max-bpc (drm), mirror-of/resizeable (vnc/rdp) — are a
# startup error when that backend is not loaded, rather than inert.
#[[output]]
#name = "headless"
# "WxH" or "WxH@rate"; an unparseable mode logs and keeps the backend
# default (1024x640 headless).  CLI --width/--height override it.
#mode = "1024x640"
# Positive integer; CLI --scale overrides it.
#scale = 1
# normal, rotate-90/180/270, flipped, flipped-rotate-90/180/270.
# An unknown name is a startup error.  CLI --transform overrides it.
# (VNC outputs are always normal, as in C.)
#transform = "normal"
# vnc/rdp: allow client-driven desktop resize (default true;
# forced off when this output mirrors another).
#resizeable = true
# Mirror a native output onto this remote head (vnc): the remote
# tracks the source's mode/position and disables client resize.
#mirror-of = "headless"

# Leave a head unenabled.  `mode = "off"` is the same switch under the
# ini spelling.  This is a westonite extension: C has no per-output off
# switch for the windowed backends.
#[[output]]
#name = "DP-1"
#off = true

[vnc]
#port = 5900
# Bind address (default: all interfaces).  CLI --address overrides.
#address = "127.0.0.1"
#refresh-rate = 60
#tls-cert = "/path/tls.crt"
#tls-key = "/path/tls.key"
#disable-transport-layer-security = false

[rdp]
#port = 3389

[pipewire]
#num-outputs = 1
```

### A.26 `docs/callback-inventory.md` — copy, then apply

Deltas: row L5 (the `--debug` allow-all authority) stays; rows L8 and L10 and binding B1 (the Super+S client spawn, its destroy listener, its authority) are marked `not ported — F5` instead of R2e; the Status column is reset to `—` at R0 and advanced per slice. The header counts describe the C tree before F5; say so in the header.

````markdown
# Callback inventory (R0 deliverable — plan §3e)

The audit surface for the fence: every place C calls into our code, one
row each, with its dispatch tier and (for sync-tier rows that borrow app
state) the required non-reentrancy proof (amendment A3).  A reviewer
checks the "no unsoundness outside `weston`" claim against this table,
not against 9.7k lines.

Row counts reconcile with the plan §1 figures (counted at trim T9):
**29** `wl_listener` fields (one of them a `main()` stack local, L7),
**23** signal/destroy-listener attach sites for those fields (the 24th
`wl_signal_add`-family call in the tree is
`weston_compositor_add_destroy_listener_once` at `shell.c:2184`, counted
under the compositor-destroy row; the two
`weston_compositor_add_screenshot_authority` calls attach L5/L10 outside
that count), **5** binding registrations with our own handlers +
`weston_install_debug_key_binding` (handler is libweston's) = 6
registrations total, and the `weston_desktop_api` vtable (14 struct
entries; the C shell installs 10, the 4 unfilled ones have "not
installed" rows).

Tier legend — **sync**: handled inside the trampoline (only
wrapper-held or callback-local data unless a proof is recorded);
**deferred**: trampoline captures payload eagerly, enqueues, drained at
depth zero.  *Sync+inval / deferred-policy*: destroy notifications split
per §3e — registry invalidation runs synchronously, the policy reaction
is a deferred event.

Port status legend: `—` not yet ported · `R0` implemented in the R0
foundation · statuses advance to `R1`/`R2x` as phases land.

**Correction (2026-08-02, PR17-C2):** L27 and L28's create half were
recorded *deferred* while the implementation has dispatched both sync
since R1 (`register_seat` / `register_output_shell` both end in
`dispatch_sync`).  The rows now carry the sync proofs; `events.rs`'s
section grouping is corrected to match.  This inventory is the audit
surface for exactly that class of drift — trust the Tier cells only as
far as the named dispatch sites confirm them.

**R1 status note (2026-07-25):** every shell-side row is live — the
desktop-shell listener set (L11–L28) is implemented in
`crates/weston/src/shell_init.rs` (seat/output/session/transform/
compositor-destroy wiring), `grab.rs` (all three pointer grabs + touch
move, L14/L15 deleted per §3c as designed), `desktop.rs` (the full
vtable), and `input_bindings.rs` (B3–B6); the shell policy behind them
is `crates/westonite-shell`.  Frontend rows (L1–L10, B1/B2) remain C
until R2.  The Status column below is updated per row group rather than
per cell; `git log` on the named modules is the per-row audit trail.

## 1. `wl_listener` fields and their attach sites

### frontend/main.c (7 fields)

| # | Field (owner struct) | Handler | Attach site | Signal | Tier | Notes / hazards | Status |
|---|---|---|---|---|---|---|---|
| L1 | `wet_head_tracker.head_destroy_listener` | `handle_head_destroy` (1918) | `main.c:1972` | head destroy | sync+inval / deferred-policy | destroys the output when its last head goes; output destroy cascades while head is dying (staleness shape 1) | R0 (wrapper head registry) |
| L2 | `wet_head_tracker.resized_listener` | `simple_heads_output_sharing_resize` (2581) | `main.c:2642` | `output_resized_signal` | **sync** (proof: emitted from output-resize processing, never from an outbound call under an app borrow; the handler touches only wrapper/policy state and libweston geometry) | mirror-of resize propagation: reposition the remote over its source and recompute its modeline | R2c-mirror (compositor.rs mirror_on_output_resized; one compositor-wide listener replaces C's per-tracker one — same effect, the rule table is the discriminator) |
| L3 | `wet_output.output_destroy_listener` | `wet_output_handle_destroy` (2509) | `main.c:2668` | output destroy | sync+inval / deferred-policy | listener identity used as lookup key in C (`wet_output_from_weston_output`, 2674) — becomes a registry lookup (§3c) | R0 (wrapper output registry) |
| L4 | `wet_backend.heads_changed_listener` | `simple_heads_changed` (2099) / `drm_heads_changed` (3006) | `main.c:3358` | `heads_changed_signal` | **sync** (proof: emitted only from `weston_compositor_flush_heads_changed`, called by us or from backend event sources — never from an outbound call made while the app borrow is held; R0 handler touches wrapper state only) | must create+enable outputs inside the flush (§3e list) | R2b-headless (policy-driven: `OutputPolicy` lookup + all three C branches — enable/disable/device-changed, filtered to this backend's heads as C's `wet_backend_iterate_heads` does; the policy is plain data, so the no-app-borrow proof is unchanged) |
| L5 | `wet_compositor.screenshot_auth` | `screenshot_allow_all` (4396) | via `weston_compositor_add_screenshot_authority` (4628) | auth request | sync (out-param decision; answers from config value) | — | — |
| L6 | `wet_compositor.output_created_listener` | `wet_output_handle_create` (2604) | `main.c:4245` | `output_created_signal` | **sync** (proof: emitted inside weston_compositor_add_output during the source's enable; the handler enables the deferred remote head — wrapper/policy work only, exactly C's nesting) | remote/mirror output setup: a native output's creation enables the remote head configured to mirror it | R2c-mirror (compositor.rs mirror_on_output_created; guards an already-enabled head where C would double-create) |
| L7 | local in `main()` (main.c:4443), `primary_client_destroyed` | `handle_primary_client_destroyed` (926) | `main.c:4704` | client destroy | deferred | shutdown-on-client-exit policy | — |

### frontend/weston-screenshooter.c (3 fields)

| # | Field | Handler | Attach site | Signal | Tier | Notes | Status |
|---|---|---|---|---|---|---|---|
| L8 | `screenshooter.client_destroy_listener` | `screenshooter_client_destroy` (46) | `w-s.c:79` | client destroy | **sync** (proof: resets the wrapper client slot only — a Cell write, no app borrow; C's handler is the same one-line reset) | one oneshot Listener embedded in the state, reattached per spawn (C reuses its embedded listener the same way); no box per press — that was a per-press leak while the drain gap lasted (since fixed, §3e/A4), and remains the right shape anyway | R2e (screenshooter.rs) |
| L9 | `screenshooter.compositor_destroy_listener` | `screenshooter_destroy` (119) | `w-s.c:148` | compositor `destroy_signal` | sync+inval | teardown ordering only | R2e — **not a listener in Rust**: Compositor::drop calls screenshooter::teardown before weston_compositor_destroy (same detach-before-free effect as C's in-destroy removal) |
| L10 | `screenshooter.authorization` | `authorize_screenshooter` (108) | via auth API | auth request | sync (out-param; writes att->authorized for the compositor's own spawned client only — wrapper state read, no app borrow) | attached by direct wl_signal_add on `output_capture.ask_auth` — the C helper (`weston_compositor_add_screenshot_authority`) would overwrite the Listener trampoline's notify | R2e (frontend_listeners: detached in Drop before destroy) |

### desktop-shell/shell.c + shell.h (19 fields — all rows R1 unless marked vestigial/deleted)

| # | Field (owner) | Handler | Attach site | Signal | Tier | Notes / hazards | Status |
|---|---|---|---|---|---|---|---|
| L11 | `focus_state.seat_destroy_listener` | `focus_state_seat_destroy` (352) | `shell.c:422` | seat destroy | sync+inval / deferred-policy | staleness shape 1 | — |
| L12 | `focus_state.surface_destroy_listener` | `focus_state_surface_destroy` (363) | `shell.c:456` | surface destroy | sync+inval / deferred-policy | hunts replacement focus from inside a destroy handler — the canonical §3b hazard; policy must run deferred | — |
| L13 | `shell_surface.output_destroy_listener` | `notify_output_destroy` (1059) | `shell.c:1094` | output destroy | sync+inval / deferred-policy | weak backref nulling in C → stale `OutputId` in Rust | — |
| L14 | `shell_grab.shsurf_destroy_listener` | `destroy_shell_grab_shsurf` (178) | `shell.c:243` | shsurf destroy (shell-internal signal) | n/a — **deleted**: intra-Rust notification never routes through `wl_signal` (§3c); becomes registry invalidation + grab-local `SurfaceId` going stale | grabbed-surface-dies-mid-grab | — |
| L15 | `shell_touch_grab.shsurf_destroy_listener` | same (178) | `shell.c:301` | same | same as L14 | — | — |
| L16 | `shell_seat.seat_destroy_listener` | `destroy_shell_seat` (1119) | `shell.c:1161` | seat destroy | sync+inval / deferred-policy | — | — |
| L17 | `shell_seat.caps_changed_listener` | `shell_seat_caps_changed` (1129) | `shell.c:1170` | `updated_caps_signal` | **sync** (proof: emitted from seat capability updates during input-device dispatch, never from shell outbound calls; handler only re-attaches L18/L19 — wrapper state) | dynamic attach/detach of the focus listeners (the `wl_list_empty` idiom §3c) | — |
| L18 | `shell_seat.pointer_focus_listener` | `handle_pointer_focus` (928) | `shell.c:1139` (from caps_changed) | pointer `focus_signal` | deferred | — | — |
| L19 | `shell_seat.keyboard_focus_listener` | *(none — vestigial)* | never attached (only `wl_list_init`, shell.c:1164) | — | n/a | vestigial after the T-series trims: no handler exists; not ported (see `shell_init.rs` header note) | n/a |
| L20 | `workspace.seat_destroyed_listener` | `seat_destroyed` (477, notify set at 500) | never attached (only `wl_list_init`, shell.c:499/2207) | — | n/a | dead code in the trimmed tree; not ported as a listener | n/a |
| L21 | `shell_output.destroy_listener` | `handle_output_destroy` (1902) | `shell.c:1981` | output destroy | sync+inval / deferred-policy | curtain teardown (kind-2 ownership) | — |
| L22 | `desktop_shell.transform_listener` | `transform_handler` (1730) | `shell.c:2192` | `transform_signal` | sync (proof: emitted during commit processing of xwayland surfaces only; handler reads xwayland API + surface state, no app-borrow needed beyond the subject) | xwayland send_position | — |
| L23 | `desktop_shell.resized_listener` | `handle_output_resized` (1915) | `shell.c:2228` | `output_resized_signal` | deferred | background curtain resize | — |
| L24 | `desktop_shell.destroy_listener` | `shell_destroy` (2087) | `shell.c:2184` (`weston_compositor_add_destroy_listener_once`) | compositor destroy | **sync** (teardown; proof: emitted from `weston_compositor_destroy`, which the frontend calls at exit outside any app borrow) | full teardown; queue discarded after (§3e) | — |
| L25 | `desktop_shell.session_listener` | `desktop_shell_notify_session` (2133) | `shell.c:2231` | `session_signal` | deferred | VT switch / lock policy | — |
| L26 | `desktop_shell.pointer_focus_listener` | *(none — vestigial)* | never attached (field declared shell.h:62, unreferenced in shell.c) | — | n/a | dead field; busy-cursor policy is driven by `ping_timeout` + the per-seat L18 path; not ported | n/a |
| L27 | `desktop_shell.seat_create_listener` | `handle_seat_created` (2162) | `shell.c:2225` | `seat_created_signal` | **sync** (proof: emitted from `weston_seat_init` during backend input bring-up / device hotplug — the shell makes no outbound call that creates a seat, so the emission can never nest under the app borrow; the handler registers wrapper seat state, then `dispatch_sync(SeatCreated)` — shell_init.rs `register_seat`) | registers shell_seat; the registry insert is sync-in-trampoline like every §3b registration | — |
| L28 | `desktop_shell.output_create_listener` / `output_move_listener` | `handle_output_create` (1991) / `handle_output_move` (2020) | `shell.c:2040/2044` | `output_created/moved_signal` | **sync** (create) / deferred (move).  Create proof: `output_created_signal` is emitted inside `weston_compositor_add_output` during output enable, reached only from heads-changed processing (wrapper/policy state, no app borrow — the L4 argument); the shell makes no outbound call that enables an output.  Handler: shell_init.rs `register_output_shell` → `dispatch_sync(OutputCreated)` | background curtain creation must happen **inside** the `output_created` emission — load-bearing since R2b (the curtain-missing bug PR19-C1) — which is why create is sync while move stays a plain deferred notification | — |

*(L28 folds two sibling fields into one row for brevity; unlike the
original plan the two halves are no longer the same tier — see the
Tier cell.  Field count: 7 + 3 + 19 = 29.)*

## 2. Bindings (6 registrations — B3–B6 are R1; B1/B2 are R2e)

| # | Binding | Handler | Site | Tier | Status |
|---|---|---|---|---|---|
| B1 | key Super+S | `screenshooter_binding` | `w-s.c:142` | **sync** (was planned deferred; proof: wrapper screenshooter state + fd/spawn plumbing + wl_client_create — no app borrow, and C's handler completes inside the binding too) | R2e (screenshooter.rs) |
| B2 | key Super+R | `recorder_binding` | `w-s.c:144` | **sync** (proof: wrapper recorder Cell + weston_recorder start/stop — no app borrow) | R2e (divergence: C's empty-output-list fallback is a wild `container_of` on the list head; ours logs and no-ops) |
| B3 | button BTN_LEFT | `click_to_activate_binding` | `shell.c:2120` | **sync** (reads pointer button/serial state valid only in the input frame; activation policy enqueued) | — |
| B4 | button BTN_RIGHT | `click_to_activate_binding` | `shell.c:2123` | sync (as B3) | — |
| B5 | touch | `touch_to_activate_binding` | `shell.c:2126` | sync (as B3) | — |
| B6 | debug key (Super) | libweston's own handler | `shell.c:2129` (`weston_install_debug_key_binding`) | n/a (no Rust callback) | — |

## 3. `weston_desktop_api` (14 struct entries; 10 installed at `shell.c:1607-1619` — all installed rows are R1, in `crates/weston/src/desktop.rs`)

| Entry | Installed | Tier | Notes | Status |
|---|---|---|---|---|
| `struct_size` | (size field) | — | set from the *bound* 14.0.1 headers (risk R-C) | — |
| `ping_timeout` | yes | deferred | unresponsive flag + busy cursor | — |
| `pong` | yes | deferred | — | — |
| `surface_added` | yes | **sync** (§3e closed list: brackets the C object's life; registry insert + user-data id write must happen inside the call) | — | — |
| `surface_removed` | yes | **sync policy + sync inval** (the half-dead window, §3a: `SurfaceRemoved` is dispatched sync while the id still resolves — A3: no app borrow can be live inside a desktop-api request — then state teardown and id invalidation follow in the same call) | — | — |
| `committed` | yes | **sync** (must map the surface inside commit processing) | — | — |
| `show_window_menu` | no — "not installed" | — | — | — |
| `set_parent` | yes | **sync** (deferring would reorder against stacking updates from the same request; A3: desktop-api request context, no app borrow live) | children fixup on death is shape-1 staleness | — |
| `move` | yes | **sync** (reads pointer serial/button inside the request frame; starts grab) | — | — |
| `resize` | yes | sync (as move) | — | — |
| `fullscreen_requested` | no — "not installed" | — | — | — |
| `maximized_requested` | no — "not installed" | — | — | — |
| `minimized_requested` | no — "not installed" | — | — | — |
| `set_xwayland_position` | yes | sync (writes position used by the following commit) | — | — |
| `get_position` | yes | **sync, out-params, NO app borrow** (A3: answers from wrapper-held geometry only) | `shell.c:1597-1605` | — |

## 4. Grabs (vtables, not listeners)

| Vtable | Entries | Tier | Status |
|---|---|---|---|
| pointer grab (`shell_grab`) | focus/motion/button/axis/axis_source/frame/cancel | sync, **no app borrow** (state lives in the grab box; policy events deferred; free deferred to depth-zero drain) | R1 (move/resize/busy live) |
| touch grab (`shell_touch_grab`) | down/up/motion/frame/cancel | same | R1 |
| busy-cursor grab | pointer iface | same | R1 |

## 5. Log handlers (§3k)

| Handler | Via | Tier | Status |
|---|---|---|---|
| `vlog` / `vlog_continue` | shim `wsys_install_log_handlers(scope)` (C main.c:214/241) | sync, no app borrow — timestamps the line and prints it into the **"log" scope**, where libweston's subscribers (the `--log` file, the flight recorder) take it | R0, rewritten at R2f |
| *(same pair, scope-free fallback)* | `wsys_install_log_handlers(NULL)` → `wsys_rust_log_sink` | sync, no app borrow (writes the `--log` file or stderr directly) | Used before a `LogContext` exists and after it is dropped — the R0 smoke binary, the unit harness, a panic-barrier line during teardown. C has no equivalent and drops those lines |
| `protocol_log_fn` (C main.c:267) | `wl_display_add_protocol_logger` in `build()`; the "proto" scope rides in as `user_data` | sync, no app borrow — formats one line and writes it into the scope | R2g. Installed **unconditionally**, as C does; returns immediately on an unsubscribed scope, so it is free until `--logger-scopes=proto`. The only trampoline that needs no `Ctx` at all: the scope pointer *is* its user data, which also keeps it correct across teardown ordering |
| `debug.allow_all_screenshots` (C `screenshot_allow_all`, main.c:4396) | `Listener` on `output_capture.ask_auth`, installed only under `--debug` | sync, no app borrow (writes `att->authorized`) | R2g. A `Listener` rather than a bare fn for the same reason as the screenshooter's authority: the C helper writes `listener->notify` directly and would overwrite the primitive's trampoline |
| `flight_rec_binding` (C `flight_rec_key_binding_handler`, main.c:4371) | `weston_compositor_add_debug_binding(KEY_D, …)` in `build()`, only when a recorder exists | sync, no app borrow — dumps the ring buffer through weston_log, so it takes the A4 wrap when a `Ctx` exists | R2f. The one callback in the tree with a real `data` payload (the subscriber) instead of `Ctx::current`: C passes it the same way, and the pointer belongs to the frontend's `LogContext`, which outlives every binding |

## 6. Event-loop signal sources (fd callbacks, not `wl_listener`s — C main.c `signals[]`)

| Source | Handler | Site | Tier | Status |
|---|---|---|---|---|
| SIGTERM / SIGINT | `on_term_signal` | `compositor.rs` `install_signal_sources` (C main.c:4553 block) | sync, no app borrow (`wl_display_terminate` only; delivered via signalfd on the loop, not async signal context) | R0 |
| SIGCHLD | `on_sigchld` | same site — installed in `build()`, deliberately **before** the frontend spawns any client (C installs signals[] before `execute_autolaunch`; a watched client exiting in that window must not be lost) | sync, no app borrow (panic-guarded `waitpid` WNOHANG loop; autolaunch-watch match terminates the display; since R2d an Xwayland-pid match logs the exit and relays `xserver_exited` — wrapper state + module call only) | R2a (+R2d xwayland branch) |

The same `install_signal_sources` also blocks SIGUSR1 process-wide
(C main.c:4570, unconditional): no handler consumes it in 14.x — the
block only keeps a stray SIGUSR1 from Xwayland from killing the
process, and must precede backend worker-thread creation so the
threads inherit it.

## 7. Xwayland plugin-API callbacks (R2d — C frontend/xwayland.c; no `wl_listener`s, so outside the §1 counts)

| Callback | Registered via | Tier | Notes / hazards | Status |
|---|---|---|---|---|
| `spawn_xserver` | `weston_xwayland_api.listen` (xwayland.rs `load`, C wet_load_xwayland) | **sync with a return value** (the module consumes the returned `wl_client` immediately; proof: fd plumbing + `westonite-spawn` fork/exec + `wl_client_create` — wrapper xwayland state only, no app borrow) | `abstract_fd`/`unix_fd` are **borrowed** from the module (C passes the raw numbers to the child; we dup) — never close them; on `wl_client_create` failure the spawned server stays untracked, like C's `err_proc` unlink | R2d |
| `handle_display_fd` | `wl_event_loop_add_fd` on the `-displayfd` pipe (registered inside `spawn_xserver`) | sync, no app borrow (reads the pipe; on the newline marker hands `wm_fd` to `xserver_loaded` — module call with no borrows held) | C returns 1 on EOF too (re-arms a level-triggered pipe that stays readable forever); we tear the watch down on EOF instead — documented divergence in xwayland.rs. Exactly one watch may exist: `spawn_xserver` removes a watch a previous server left behind (it can outrun its own EOF event when the module respawns inside the same dispatch batch) instead of overwriting the slot as C does | R2d |
| (C `xserver_cleanup` wet_process callback) | folded into the §6 SIGCHLD row — the pid match replaces C's `wet_process` list walk | — | teardown calls `xserver_exited` for a still-running server before `weston_compositor_destroy` (C wet_xwayland_destroy order) | R2d |

Maintenance rule: growing the sync tier or adding a callback without a
row here fails review; the R1/R2 porting PRs update the Status column
as rows go live.

## 8. Backend-config callbacks (R2c-input — C main.c, installed into a `weston_*_backend_config`; no `wl_listener`s, so outside the §1 counts)

| Callback | Registered via | Tier | Notes / hazards | Status |
|---|---|---|---|---|
| `configure_device` (C `configure_input_device`, main.c:2239) | `weston_drm_backend_config.configure_device` (`compositor.rs` `load_drm`; DRM is the only backend with the field) | sync, no app borrow (reads the `Ctx`-held `InputConfig` and calls libinput device-config setters — wrapper state only) | Called on C's own schedule: once per device already present *during* `weston_compositor_load_backend`, and again for every hotplug, so it needs the full trampoline shape (panic barrier, `Ctx::current`, A4 depth wrap) even though it dispatches nothing. Installed unconditionally, as C does, so the per-device log line appears whether or not `[libinput]` exists. There is no user-data argument on this hook — the config reaches it through `CtxInner::input_config`, which `load_drm` must populate **before** the load call | R2c-input |
````

### A.27 `docs/r0-header-facts.md` — copy as-is, re-verifying every row

````markdown
# R0 header-fact verification (plan §3a caveat)

Every §3 claim the wrapper design keys off, re-verified against the
**installed** EPEL 10 `weston-devel-14.0.1-3.el10_0` headers — the same
headers bindgen consumes — on 2026-07-25 inside the build container.
Re-run this check (and the bindings regen) whenever EPEL bumps weston
(plan §8; the pkg-config tripwire in `crates/weston-sys/build.rs` makes
a silent bump impossible).

| # | Claim (plan §3) | Verified against 14.0.1 headers | Result |
|---|---|---|---|
| F1 | `weston_keyboard` / `weston_touch` have **no** `destroy_signal` | struct definitions in `libweston.h` (only `focus_signal` + grab state) | ✅ — "never store input sub-objects" rule stands |
| F2 | `weston_seat`, `weston_surface`, `weston_view` announce death via `destroy_signal` | struct fields present | ✅ |
| F3 | `weston_pointer` has `destroy_signal` | field present | ✅ |
| F4 | `weston_output` death for *users* is `user_destroy_signal`, attached via `weston_output_add_destroy_listener`; the sibling `destroy_signal` fires **when disabled** | `libweston.h:548` (user), `:610` ("sent when disabled") | ✅ — registry must use the add/get-destroy-listener API, never the disable signal (encoded in `compositor.rs::register_output`) |
| F5 | `weston_head` has `destroy_signal` + dedicated add/get-listener API | `libweston.h:481`, `:2656-2661` | ✅ |
| F6 | Compositor destroy via `weston_compositor_add_destroy_listener_once` | `libweston.h:2451` | ✅ |
| F7 | `weston_desktop_surface` has **no** signal; death is the `surface_removed` API callback; exactly two user-data slots exist (`weston_desktop_surface_{set,get}_user_data`, get-only `weston_compositor_get_user_data`) | `desktop.h:154/187`, `libweston.h:2481` | ✅ |
| F8 | `weston_desktop_api` has 14 function-pointer entries, `struct_size`d | `desktop.h` (counted mechanically) | ✅ |
| F9 | `weston_curtain_params` has the `get_label` **callback** (15.x changed it to `char *label` — the reference-tree trap the plan flagged) | `shell-utils.h` | ✅ — 14.x shape confirmed; sync-tier, no-app-borrow row in the callback inventory |
| F10 | `weston_pointer_grab_interface`: 7 entries (focus, motion, button, axis, axis_source, frame, cancel) | `libweston.h` | ✅ — grab vtable in `grab.rs` matches |
| F11 | Headless config: `{base, renderer, decorate, refresh}`, `WESTON_HEADLESS_BACKEND_CONFIG_VERSION` | `backend-headless.h:39-52` | ✅ |
| F12 | Windowed output API v2 name `weston_windowed_output_api_headless_v2`; `weston_windowed_output_get_api` is a **static inline** wrapper over `weston_plugin_api_get` | `windowed-output-api.h:40, 89-103` | ✅ — Rust calls `weston_plugin_api_get` directly; no shim needed |
| F13 | Static-inline inventory of the installed headers: plugin-api getters (drm ×2, pipewire ×2, rdp, vnc, windowed, xwayland ×2, remoting), `matrix.h` coord constructors, `zalloc` | grep over `/usr/include/libweston-14` | ✅ — none need the C shim; getters → direct `weston_plugin_api_get`, coord math → plain Rust, `zalloc` irrelevant.  The shim carries only the wayland-util/server inlines (`wl_list_*`, `wl_signal_add`) + the va_list log pair (§3k) |
| F14 | `weston_compositor_create(display, log_ctx, user_data, test_data)` signature | `libweston.h:2443` | ✅ |
| F15 | Renderer enum: AUTO=0, NOOP=1, PIXMAN=2, GL=3 | `libweston.h:2465` | ✅ |

Findings that adjusted the R0 code (not §3 contradictions):

- **`weston_output_lazy_align` is not libweston API** — it is a static
  helper in `frontend/main.c:1990` (frontend-local placement policy).
  The R0 canned configurator skips it (single output at (0,0)); the
  R2b output-management port implements the real placement logic.
- **`weston_compositor_backends_loaded` is mandatory** before any
  output is created: it installs the default (no-op) color manager,
  and `weston_output_init` dereferences `compositor->color_manager`
  unconditionally.  Missing it is a startup segfault (found live, fixed
  in `compositor.rs`; the C frontend calls it at `main.c:4660`).
- The installed `windowed-output-api.h` does not parse standalone: its
  static inline uses weston's internal `ARRAY_LENGTH` macro, which the
  RPM does not install.  The bindings regen script supplies the macro
  (`scripts/regen-bindings.sh`).
````

### A.28 `docs/config-migration.md` — copy, then apply

Deltas: the `modules=` row becomes "refused (F3)"; add rows for `idle-time` (refused, F6) and `[rdp]` (refused, F1).

````markdown
# Config migration: `westonite.ini` → `westonite.toml`

The Rust frontend (R2a, plan §5, decisions D9–D12) re-specifies the
configuration interface.  There is no converter tool and no dual-format
fallback (D11): if a legacy `westonite.ini` sits where the TOML is
expected, startup logs a one-line hint and otherwise ignores it.

An annotated example lives at the repo root:
[`westonite.toml.example`](../westonite.toml.example).

## What stays the same

- The **search order**: `$XDG_CONFIG_HOME`, then `$HOME/.config`, then
  `$XDG_CONFIG_DIRS` (default `/etc/xdg`) — each location is tried in
  turn, so setting `XDG_CONFIG_HOME` does not mask a config in
  `$HOME/.config`; `--config=PATH` and `--no-config` still work.  The
  file *name* changes (`westonite.toml` instead of `westonite.ini`),
  and one system-config path moves: libweston read the ini from a
  hard-coded `weston/` subdirectory of each `$XDG_CONFIG_DIRS` entry
  (`/etc/xdg/weston/westonite.ini`); the TOML lives in the entry
  itself (`/etc/xdg/westonite.toml`).
- **Key names**: kebab-case throughout, matching the ini's hyphenated
  keys — most entries change section syntax only, not spelling.
- **Value grammars** that are really weston's own (modelines like
  `1920x1080@60`, XKB names, transforms, gbm formats, ICC paths) are
  unchanged strings.

## What changes

| ini | TOML | Note |
|---|---|---|
| `[core]` … `key=value` | `[core]` … `key = value` | strings need quotes: `backend=headless` → `backend = "headless"` |
| `backends=drm,vnc` | `backends = ["drm", "vnc"]` | comma lists become arrays; a single comma-separated string is still accepted |
| `modules=a.so,b.so` | `modules = ["a.so", "b.so"]` | " |
| repeated `[output]` sections | `[[output]]` array-of-tables | one `[[output]]` block per output |
| `[output] mode=off` | `mode = "off"` or `off = true` | " |
| `[color_characteristics]` | `[[color-characteristics]]` | the one ini section spelled with an underscore; kebab-case like every other key |
| booleans `true`/`false` | bare `true` / `false` | unquoted |
| numbers | bare numbers | unquoted |
| `background-color=0xff002244` | `background-color = "0xff002244"` | quoted string, hex spelling kept — **always base-16** like the ini (`"ff002244"` unprefixed works; `"12345678"` means 0x12345678, not twelve million). A bare TOML integer also works by value, so `-o shell.background-color=0xff002244` needs no quoting. Where the ini silently falls back to the default color on a bad value, this is a startup error |
| `[libinput] enable_tap=` (deprecated) | **dropped** — spell it `enable-tap` | C still honors the underscore spelling behind a `!!DEPRECATION WARNING!!`; here it is a startup error that names the rename |
| unknown/typo'd keys silently ignored | **startup error** with line/column | D9: `deny_unknown_fields` |
| `WESTON_CONFIG_FILE` exported to clients | **dropped** | D12: no shipped client reads it, and no stock client parses TOML |
| `weston.ini` never read | unchanged (`westonite.toml` only) | P2 behavior kept |
| — (no ini ancestor) | `[vnc] address = "…"` | re-spec extension for CLI/file symmetry (`--address` had no ini partner); reproducing a TOML config on the C oracle needs the CLI flag instead |

## CLI

Dedicated flags keep their C spellings (`--backend`, `--socket`,
`--log`, `--width`/`--height`, `--config`, `--no-config`, trailing
autolaunch command after `--`).  New: any file key is settable with
`-o`/`--set section.key=value` (repeatable), applied to the config
tree after the file and before dedicated flags.  Override values are
read as TOML when they parse as TOML (`-o core.xwayland=true` gives a
boolean) and as plain strings otherwise, so list keys take either
spelling: `-o core.backends=drm,vnc` or
`-o 'core.backends=["drm","vnc"]'`.  Unknown flags are rejected by clap
before startup (stderr), not logged as `unhandled option:`.

`--backend` and `--backends` are one flag with two spellings, as in
C (both option-table entries write the same variable): either accepts
a comma-separated list and the last occurrence on the command line
wins.  The `[core]` keys behave the same way: `backends` is consulted
first and `backend` only as its fallback (C main.c reads them in that
order into the same variable), and either one may be a comma list —
`backend = "headless,vnc"` loads both.

`--use-gl` and `--use-pixman` remain mutually exclusive, and neither
may be combined with `--renderer` (C: `Conflicting renderer
specifications`).  `--width`, `--height` and `--scale` must be
positive; C silently treated `0` as "use the default", which hid
typos.  `[[output]] scale` follows the same rule — C passed it
straight to `weston_output_set_scale`, where `0` trips an assert.

`[shell]` gains `cursor-theme` and `cursor-size` (C read them from
the same section for the nested wayland backend's cursor; default size
32).  `[pipewire] num-outputs` and `[[output]] gbm-format` are live for
the pipewire backend.

`[[output]]` sections are validated at startup, all of them, not
lazily when a head of that name turns up: an unknown `transform` name
is fatal (C's `Invalid transform "…"` wording), as is a section with
no `name` key, and so is any key whose behaviour is not ported yet
(`clone-of`, the colour-management attributes).  `mirror-of` is
ported for remote (vnc) heads — and unlike C, a *headless* mirror
source works: C aborts on `assert(native_mode_copy.width)`
(main.c:2543, verified live) because the headless backend publishes no
native mode (drm/x11/pipewire do), and the Rust frontend falls back to
the source's current mode instead.  C
resolves a section only when its head appears, so a section that
matches nothing is silently inert there — indistinguishable, from the
outside, from one that was honoured.  An unparseable `mode` is the
exception and stays non-fatal, matching C: it logs `Invalid mode for
output NAME. Using defaults.` and falls back to the backend default.
````

### A.29 `crates/westonite-config/src/model.rs` — copy, then apply

Deltas: `Libinput.touchscreen_calibrator` gains `#[serde(alias = "touchscreen_calibrator")]` and `calibration_helper` gains `#[serde(alias = "calibration_helper")]`, so the shared refusal tests written in the C spelling pass in TOML mode; the doc comments on `Core.idle_time`, `Core.modules` and `Rdp` say "refused". No field is removed: every key stays in the model so its refusal can name it.

```rust
//! The serde `Config` model: the entire `westonite.toml` surface.
//!
//! Kebab-case keys throughout (one `rename_all`), `deny_unknown_fields`
//! everywhere: a typo'd key is a startup error with a span, replacing
//! weston's silent-typo behavior (D9).  Defaults here mirror the C
//! frontend's defaults exactly; deviations are §10 material.

use std::fmt;

use serde::Deserialize;
use serde::de;

fn default_true() -> bool {
    true
}

/// Accept a quoted string *or* a bare number for keys whose value is
/// really a numeric string in weston's grammars (`[libinput]
/// scroll-button`; colors use [`de_opt_color_string`]).
///
/// Needed because `-o` overrides are parsed as TOML values before
/// deserialization (`overrides.rs`), and TOML reads `0xff002244` as an
/// integer — a bare-number spelling would otherwise die as
/// "invalid type: integer, expected a string".  Numbers are handed on
/// in decimal.
fn de_opt_scalar_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = Option<String>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string or a number")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D2: de::Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(V)
        }
    }
    d.deserialize_any(V)
}

/// The color-key variant of [`de_opt_scalar_string`]: a bare TOML
/// number is handed on as `0x…` **hex**, not decimal, because the
/// key's string grammar is C's `weston_config_section_get_color`
/// (shared/config-parser.c:232) — ALWAYS base-16, prefix or no prefix.
/// A decimal rendering of the integer would be re-read as hex and
/// silently change the value; formatting it as hex preserves it
/// exactly (`-o shell.background-color=0xff002244` arrives as the
/// TOML integer 4278199876 and leaves as `"0xff002244"`).  A value
/// that doesn't fit in 32 bits formats to more than 8 digits and is
/// rejected by `resolve::parse_color`'s C length gate; a negative
/// number is refused here, where the span still points at the key.
fn de_opt_color_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = Option<String>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a color string or a non-negative number")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            match u64::try_from(v) {
                Ok(u) => self.visit_u64(u),
                Err(_) => Err(E::invalid_value(
                    de::Unexpected::Signed(v),
                    &"a non-negative color value",
                )),
            }
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(format!("0x{v:08x}")))
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D2: de::Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(V)
        }
    }
    d.deserialize_any(V)
}

/// Accept a TOML array *or* one comma-separated string for list keys
/// (`[core] backends`, `[core] modules`).  The array is the documented
/// TOML spelling; the comma list keeps the ini spelling working, which
/// the `--backends`/`--modules` flags and `-o core.backends=drm,vnc`
/// both produce (an override value never arrives pre-typed as an
/// array unless the user writes TOML array syntax by hand).
fn de_string_or_seq<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an array of strings or a comma-separated string")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(v.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(String::from)
                .collect())
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                out.push(item);
            }
            Ok(out)
        }
    }
    d.deserialize_any(V)
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Config {
    pub core: Core,
    pub shell: Shell,
    pub keyboard: Keyboard,
    pub libinput: Libinput,
    pub autolaunch: Autolaunch,
    pub xwayland: Xwayland,
    pub rdp: Rdp,
    pub vnc: Vnc,
    pub pipewire: Pipewire,
    /// Repeated `[output]` ini sections become `[[output]]`.
    pub output: Vec<Output>,
    /// `[remote-output]` gstreamer streams (remoting plugin).
    pub remote_output: Vec<RemoteOutput>,
    /// `[pipewire-output]` streams.
    pub pipewire_output: Vec<PipewireOutput>,
    /// `[color_characteristics]` ini sections, referenced from outputs
    /// by name.
    pub color_characteristics: Vec<ColorCharacteristics>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Core {
    /// `[core] backend=` / `--backend`.  C default: drm
    /// (WESTON_NATIVE_BACKEND); resolution keeps that default.
    pub backend: Option<String>,
    /// `[core] backends=` / `--backends` (multi-backend, weston 14).
    /// Array, or one comma-separated string (the ini spelling).
    #[serde(default, deserialize_with = "de_string_or_seq")]
    pub backends: Vec<String>,
    /// `[core] renderer=` / `--renderer`: auto|gl|pixman|noop.
    pub renderer: Option<String>,
    /// `[core] use-gl=` / `use-pixman=`: deprecated aliases for
    /// `renderer=`.  Mutually exclusive, and neither may be combined
    /// with an explicit `renderer`.
    ///
    /// Divergence, deliberate: C reads these keys **only** in
    /// `load_headless_backend` (main.c:3490), so on any other backend
    /// the file spelling is silently inert while the identically named
    /// CLI flags work everywhere.  That asymmetry is an accident of
    /// where the read sits, not a design, and "silently inert" is the
    /// failure mode this port exists to remove — so here they apply to
    /// every backend, exactly like the flags.
    pub use_gl: bool,
    pub use_pixman: bool,
    /// `[core] output-decorations=`: draw a decoration border around
    /// headless outputs.  Headless-only in C too (it is a field of
    /// `weston_headless_backend_config`), so this one stays as C has
    /// it.
    pub output_decorations: bool,
    /// `[core] gbm-format=`.
    pub gbm_format: Option<String>,
    /// `[core] require-input=`.  C default true.
    #[serde(default = "default_true")]
    pub require_input: bool,
    /// `[core] color-management=`.
    pub color_management: bool,
    /// `[core] xwayland=` / `--xwayland`.
    pub xwayland: bool,
    /// `[core] idle-time=` (inert since T3 — kept for surface
    /// completeness, D1).
    pub idle_time: Option<u32>,
    /// `[core] pageflip-timeout=` (ms, DRM only; 0 disables).
    pub pageflip_timeout: Option<u32>,
    /// `[core] pixman-shadow=` (DRM only).  C default true.
    pub pixman_shadow: Option<bool>,
    /// `[core] require-outputs=`: any|all|none.  C default "any" —
    /// DRM only, since it is the only backend whose outputs can fail
    /// to come up (main.c:4644).
    pub require_outputs: Option<String>,
    /// `[core] wait-for-debugger=`: log the pid and SIGSTOP at
    /// startup.  The CLI flag wins; this only applies when it is
    /// absent (main.c:4585).
    pub wait_for_debugger: bool,
    /// `[core] repaint-window=` (ms; C validates -10..=1000).
    pub repaint_window: Option<i32>,
    /// `[core] modules=` / `--modules`: extra wet_module_init plugins.
    /// Array, or one comma-separated string (the ini spelling).
    #[serde(default, deserialize_with = "de_string_or_seq")]
    pub modules: Vec<String>,
    /// `[core] shell=` / `--shell`.  C default desktop-shell.so; the
    /// Rust frontend links its shell statically at R3 (D2) and treats
    /// any non-default value as a startup error then.
    pub shell: Option<String>,
}

impl Default for Core {
    fn default() -> Self {
        Core {
            backend: None,
            backends: Vec::new(),
            renderer: None,
            use_gl: false,
            use_pixman: false,
            output_decorations: false,
            gbm_format: None,
            require_input: true,
            color_management: false,
            xwayland: false,
            idle_time: None,
            pageflip_timeout: None,
            pixman_shadow: None,
            require_outputs: None,
            wait_for_debugger: false,
            repaint_window: None,
            modules: Vec::new(),
            shell: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Shell {
    /// `[shell] background-color=`, 0xAARRGGBB (string to keep the C
    /// hex spelling; parsed at resolve time).  C default 0xff002244.
    /// A bare TOML number is accepted too — see `de_opt_color_string`.
    #[serde(default, deserialize_with = "de_opt_color_string")]
    pub background_color: Option<String>,
    /// `[shell] client=` — empty means "no helper client" (P3); kept
    /// for surface completeness.
    pub client: Option<String>,
    /// `[shell] cursor-theme=` / `cursor-size=`: read by the nested
    /// wayland backend for the cursor it draws on the parent
    /// compositor (C load_wayland_backend, main.c:4084).  C default
    /// size 32.
    pub cursor_theme: Option<String>,
    pub cursor_size: Option<i32>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Keyboard {
    pub keymap_rules: Option<String>,
    pub keymap_model: Option<String>,
    pub keymap_layout: Option<String>,
    pub keymap_variant: Option<String>,
    pub keymap_options: Option<String>,
    pub repeat_rate: Option<u32>,
    pub repeat_delay: Option<u32>,
    pub numlock_on: Option<bool>,
    pub vt_switching: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Libinput {
    pub enable_tap: Option<bool>,
    /// C's deprecated underscore spelling of `enable-tap`, which C
    /// still honors behind a `!!DEPRECATION WARNING!!`
    /// (main.c:2260-2267).  Dropped in the re-spec — one spelling —
    /// but kept in the model so the frontend's refusal can name the
    /// rename: the user typed what their old weston.ini had, and a
    /// generic unknown-field error would point at a spelling the C
    /// docs never used.  See docs/config-migration.md.
    #[serde(rename = "enable_tap")]
    pub enable_tap_deprecated: Option<bool>,
    pub tap_and_drag: Option<bool>,
    pub tap_and_drag_lock: Option<bool>,
    /// `[libinput] disable-while-typing=` (touchpads).
    pub disable_while_typing: Option<bool>,
    pub natural_scroll: Option<bool>,
    pub left_handed: Option<bool>,
    pub middle_button_emulation: Option<bool>,
    pub rotation: Option<u32>,
    pub accel_profile: Option<String>,
    pub accel_speed: Option<f64>,
    pub scroll_method: Option<String>,
    /// evdev button *name*, e.g. `BTN_RIGHT` — that is all
    /// `libevdev_event_code_from_name` accepts, in C too.  The scalar
    /// deserializer is kept so a numeric spelling reaches the frontend
    /// as a string and is rejected with a message rather than a serde
    /// type error.
    #[serde(default, deserialize_with = "de_opt_scalar_string")]
    pub scroll_button: Option<String>,
    pub touchscreen_calibrator: Option<bool>,
    pub calibration_helper: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Autolaunch {
    /// `[autolaunch] path=`: client spawned at startup.
    pub path: Option<String>,
    /// `[autolaunch] watch=`: exit the compositor when it exits.
    pub watch: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Xwayland {
    /// `[xwayland] path=`.  C default /usr/bin/Xwayland.
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Rdp {
    pub port: Option<u16>,
    pub address: Option<String>,
    /// C config field `resizeable` (CLI spelling --no-resizeable).
    pub resizeable: Option<bool>,
    pub force_no_compression: Option<bool>,
    pub remotefx_codec: Option<bool>,
    pub external_listener_fd: Option<i32>,
    pub refresh_rate: Option<u32>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Vnc {
    pub port: Option<u16>,
    /// Hz (C VNC_DEFAULT_FREQ 60).
    pub refresh_rate: Option<u32>,
    /// Bind address (C --address only; the section spelling is a
    /// re-spec addition for CLI/file symmetry).
    pub address: Option<String>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub disable_transport_layer_security: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Pipewire {
    pub num_outputs: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Output {
    /// Head name this section configures (C `[output] name=`).
    pub name: Option<String>,
    /// Modeline / preferred / current / off (weston grammar, kept as a
    /// string — §5 "don't over-model").
    pub mode: Option<String>,
    pub scale: Option<i32>,
    pub transform: Option<String>,
    /// Explicit layout position "x,y" (weston grammar).
    pub position: Option<String>,
    /// Same-CRTC clone (DRM).
    pub clone_of: Option<String>,
    /// Mirror onto another head (P0 territory).
    pub mirror_of: Option<String>,
    /// DRM/windowed extras, all weston grammars:
    pub seat: Option<String>,
    pub gbm_format: Option<String>,
    pub pixman_shadow: Option<bool>,
    pub icc_profile: Option<String>,
    pub eotf_mode: Option<String>,
    pub colorimetry_mode: Option<String>,
    /// Name of a `[[color-characteristics]]` block.
    pub color_characteristics: Option<String>,
    pub max_bpc: Option<u32>,
    /// `[output] content-type=` (DRM): the HDMI content-type hint.
    pub content_type: Option<String>,
    /// `[output] force-on=` (DRM): enable the head even when the
    /// connector reads disconnected (C drm_head_should_force_enable).
    pub force_on: Option<bool>,
    /// `[output] resizeable=` (vnc/rdp: client-driven desktop resize;
    /// C default true).
    pub resizeable: Option<bool>,
    /// "true" disables the output (C `mode=off` alternative surface).
    pub off: Option<bool>,
    /// `[output] allow-hdcp=` (C's `allow_hdcp`), default true.  Read
    /// by every backend's configure, not just DRM.
    pub allow_hdcp: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct RemoteOutput {
    pub name: Option<String>,
    pub mode: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub gst_pipeline: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct PipewireOutput {
    pub name: Option<String>,
    pub mode: Option<String>,
    pub gbm_format: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct ColorCharacteristics {
    /// Referenced from `[[output]] color-characteristics = name`.
    pub name: Option<String>,
    /// C's ini spellings are `max_L` / `min_L` / `maxFALL`; the TOML
    /// model is kebab-case throughout (D11).
    pub max_luminance: Option<f64>,
    pub min_luminance: Option<f64>,
    pub max_fall: Option<f64>,
    pub red_x: Option<f64>,
    pub red_y: Option<f64>,
    pub green_x: Option<f64>,
    pub green_y: Option<f64>,
    pub blue_x: Option<f64>,
    pub blue_y: Option<f64>,
    pub white_x: Option<f64>,
    pub white_y: Option<f64>,
}
```

### A.30 `crates/westonite-config/src/cli.rs` — copy, then apply

Deltas: add `hide = true` to the eight RDP flags and to `idle_time`; the `modules` flag stays visible (its refusal names it).

```rust
//! The clap CLI (D10): ergonomic flags for today's CLI surface, plus
//! the generic `-o key.path=value` dotted override that patches the
//! config tree before deserialization — 100% CLI coverage of the file
//! surface without bespoke flags for structured sections.
//!
//! Flag spellings keep the C frontend's names (docs R-G checklist);
//! e2e test_cli drives them, so `--backend=headless`, `--log=`,
//! `--socket=`, `--width/height`, `--no-config`, `--config` behave as
//! before.  Trailing positional args remain the autolaunch command.

use clap::Parser;

#[derive(Debug, Clone, Parser, Default)]
#[command(
    name = "westonite",
    disable_version_flag = true,
    about = "Standalone Weston-based Wayland compositor",
    after_help = "Any [section] key of westonite.toml can be set with \
                  -o section.key=value; trailing arguments are launched \
                  as the autolaunch client (put -- before them when they \
                  carry flags of their own)."
)]
pub struct Cli {
    /// Print version and exit.
    #[arg(long)]
    pub version: bool,

    /// Backend(s) to load, comma-separated (headless, drm, x11,
    /// wayland, rdp, vnc, pipewire).  `--backends` is the same flag
    /// under its C spelling — in C both options write one variable, so
    /// the last occurrence wins and either accepts a list.
    /// (Self-override: C's option table lets the flag repeat with the
    /// last occurrence winning; clap needs that stated explicitly.)
    #[arg(
        long,
        short = 'B',
        visible_alias = "backends",
        overrides_with = "backend"
    )]
    pub backend: Option<String>,
    /// Renderer: auto, gl, pixman, noop.
    #[arg(long)]
    pub renderer: Option<String>,
    /// Legacy renderer toggles (per-backend in C; kept as aliases).
    #[arg(long)]
    pub use_gl: bool,
    #[arg(long)]
    pub use_pixman: bool,

    /// Wayland socket name to bind (default: automatic).
    #[arg(long, short = 'S')]
    pub socket: Option<String>,
    /// Log file path.
    #[arg(long)]
    pub log: Option<String>,
    /// Config file path (default: XDG search for westonite.toml).
    #[arg(long, short = 'c')]
    pub config: Option<String>,
    /// Do not read any config file.
    #[arg(long)]
    pub no_config: bool,
    /// Generic override: -o section.key=value (repeatable; applied to
    /// the config tree after the file, before flags).
    #[arg(short = 'o', long = "set", value_name = "KEY.PATH=VALUE")]
    pub set: Vec<String>,

    /// Wait for a debugger before continuing.
    #[arg(long)]
    pub wait_for_debugger: bool,
    /// Enable the weston-debug protocol.
    #[arg(long)]
    pub debug: bool,
    /// Log scopes to subscribe the logger to (comma-separated).
    #[arg(long)]
    pub logger_scopes: Option<String>,
    /// Log scopes for the in-memory flight recorder.
    #[arg(long)]
    pub flight_rec_scopes: Option<String>,

    // -- windowed/headless backend options --
    /// Output width (headless, x11, wayland, vnc, rdp, pipewire).
    #[arg(long)]
    pub width: Option<i32>,
    /// Output height.
    #[arg(long)]
    pub height: Option<i32>,
    /// Output scale (x11/wayland).
    #[arg(long)]
    pub scale: Option<i32>,
    /// Output transform: normal, rotate-90/180/270,
    /// flipped[-rotate-90/180/270] (headless, x11, wayland).
    #[arg(long)]
    pub transform: Option<String>,
    /// Fullscreen (x11/wayland nested).
    #[arg(long)]
    pub fullscreen: bool,
    /// Number of outputs (x11).
    #[arg(long)]
    pub output_count: Option<u32>,
    /// Disable input devices (x11).
    #[arg(long)]
    pub no_input: bool,
    /// One output per parent output (wayland nested).
    #[arg(long)]
    pub sprawl: bool,
    /// Parent wayland display (wayland nested).
    #[arg(long)]
    pub display: Option<String>,
    /// Create no outputs (headless).
    #[arg(long)]
    pub no_outputs: bool,
    /// Output repaint rate in mHz (headless only — C gives rdp/vnc no
    /// such flag; their Hz rate comes from `[rdp]`/`[vnc] refresh-rate`).
    #[arg(long)]
    pub refresh_rate: Option<i32>,

    // -- drm --
    /// libinput seat id (drm).
    #[arg(long)]
    pub seat: Option<String>,
    /// Primary DRM device.
    #[arg(long)]
    pub drm_device: Option<String>,
    /// Secondary DRM devices (comma-separated).
    #[arg(long)]
    pub additional_devices: Option<String>,
    /// Reuse the current CRTC mode.
    #[arg(long)]
    pub current_mode: bool,
    /// Keep running when no input device is present.
    #[arg(long)]
    pub continue_without_input: bool,

    // -- rdp/vnc --
    /// Listen port (rdp/vnc).
    #[arg(long)]
    pub port: Option<u16>,
    /// Bind address for the listener (vnc; default: all interfaces).
    #[arg(long)]
    pub address: Option<String>,
    /// TLS certificate (rdp/vnc).
    #[arg(long)]
    pub rdp_tls_cert: Option<String>,
    #[arg(long)]
    pub rdp_tls_key: Option<String>,
    #[arg(long)]
    pub vnc_tls_cert: Option<String>,
    #[arg(long)]
    pub vnc_tls_key: Option<String>,
    /// Disable TLS entirely (vnc).
    #[arg(long)]
    pub disable_transport_layer_security: bool,
    /// External listener fd (rdp).
    #[arg(long)]
    pub external_listener_fd: Option<i32>,
    /// No client-driven resize (rdp; C --no-resizeable).
    #[arg(long)]
    pub no_resizeable: bool,
    /// RDP4-style pre-shared key file (rdp; C --rdp4-key).
    #[arg(long)]
    pub rdp4_key: Option<String>,
    /// Use an inherited socket instead of listening (rdp; C
    /// --env-socket).
    #[arg(long)]
    pub env_socket: bool,
    /// Disable the RemoteFX codec (rdp; C --no-remotefx-codec).
    #[arg(long)]
    pub no_remotefx_codec: bool,
    /// Disable compression (rdp).
    #[arg(long)]
    pub force_no_compression: bool,

    // -- misc parity flags --
    /// Load Xwayland support.
    #[arg(long)]
    pub xwayland: bool,
    /// Idle timeout in seconds (inert since T3; accepted for parity).
    #[arg(long)]
    pub idle_time: Option<u32>,
    /// Extra wet_module_init plugins (comma-separated).
    #[arg(long)]
    pub modules: Option<String>,
    /// Shell plugin (parity flag; the Rust frontend's shell is built in
    /// and only the default value is accepted at R3).
    #[arg(long)]
    pub shell: Option<String>,

    /// Autolaunch client command (trailing args; use `--` before flags
    /// belonging to the client).
    #[arg(trailing_var_arg = true, allow_hyphen_values = false)]
    pub autolaunch: Vec<String>,
}
```

### A.31 `crates/westonite-config/src/lib.rs`, `overrides.rs`, `resolve.rs` — copy as-is

```rust
//! `westonite-config`: the re-specified configuration interface
//! (plan §5, decisions D9–D12).
//!
//! One serde `Config` model (TOML, kebab-case keys, unknown keys are
//! startup errors), a clap CLI, and generic `-o key.path=value` dotted
//! overrides patched into the TOML tree *before* deserialization.
//! Resolution order: defaults → file → `-o` overrides → flags, into an
//! immutable [`Settings`] resolved once at startup.  Consumers receive
//! typed slices; nobody re-reads the file or the CLI later.
//!
//! Completeness contract (risk R-G): every `[section]` key and CLI
//! option in `docs/frontend-capabilities.md` and
//! `docs/desktop-shell-capabilities.md` maps to a field below.  Values
//! that are really weston/libweston string grammars (modelines, XKB
//! names, gbm formats, transforms, ICC paths) stay strings — §5
//! re-specifies structure and validation, not weston's value syntaxes.

#![forbid(unsafe_code)]

mod cli;
mod model;
mod overrides;
mod resolve;

pub use cli::Cli;
pub use model::*;
pub use resolve::{Backend, ConfigError, Renderer, Settings, resolve, resolve_from};
```
```rust
//! `-o key.path=value` dotted overrides (D10): parse into the TOML
//! value tree *before* deserialization so overrides get exactly the
//! same typing, unknown-key, and validation treatment as the file.

use toml::Value;

/// Apply one `section.key=value` (or deeper `a.b.c=value`) override.
/// Values parse as TOML when they can (numbers, booleans, arrays) and
/// fall back to strings, so `-o core.xwayland=true` and
/// `-o shell.background-color=0xff336699` both do what they look like —
/// the latter arrives as a TOML integer (`0x…` is integer syntax), which
/// the model's numeric-string fields accept (`model::de_opt_scalar_string`).
/// Array-of-table paths accept an index: `-o output.0.mode=off`.
pub(crate) fn apply(root: &mut toml::Table, spec: &str) -> Result<(), String> {
    let Some((path, raw_value)) = spec.split_once('=') else {
        return Err(format!("override '{spec}': expected KEY.PATH=VALUE"));
    };
    let segments: Vec<&str> = path.split('.').collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Err(format!("override '{spec}': empty key path segment"));
    }
    // Validated before the first mutation: a rejected override must not
    // leave a half-built table behind in `root`.  A single segment names
    // a whole section, which is not key-shaped.
    if segments.len() < 2 {
        return Err(format!(
            "override '{spec}': path must have at least two segments"
        ));
    }

    let value = parse_value(raw_value);

    // First segment always names a table entry at the root.
    let mut current: &mut Value = root
        .entry(segments[0].to_string())
        .or_insert_with(|| Value::Table(toml::Table::new()));

    for (i, seg) in segments.iter().enumerate().skip(1) {
        let last = i == segments.len() - 1;
        if let Ok(index) = seg.parse::<usize>() {
            let arr = match current {
                Value::Array(a) => a,
                other => {
                    *other = Value::Array(Vec::new());
                    match other {
                        Value::Array(a) => a,
                        _ => unreachable!(),
                    }
                }
            };
            while arr.len() <= index {
                arr.push(Value::Table(toml::Table::new()));
            }
            if last {
                arr[index] = value;
                return Ok(());
            }
            current = &mut arr[index];
        } else {
            let table = match current {
                Value::Table(t) => t,
                other => {
                    *other = Value::Table(toml::Table::new());
                    match other {
                        Value::Table(t) => t,
                        _ => unreachable!(),
                    }
                }
            };
            if last {
                table.insert((*seg).to_string(), value);
                return Ok(());
            }
            current = table
                .entry((*seg).to_string())
                .or_insert_with(|| Value::Table(toml::Table::new()));
        }
    }

    // Unreachable: the >= 2 segment check above guarantees the loop
    // above returns from its `last` branch.
    Err(format!("override '{spec}': could not be applied"))
}

fn parse_value(raw: &str) -> Value {
    // Try full TOML value syntax first (numbers, bools, arrays,
    // quoted strings), fall back to a plain string.
    match format!("v = {raw}").parse::<toml::Table>() {
        Ok(mut t) => t
            .remove("v")
            .unwrap_or_else(|| Value::String(raw.to_string())),
        Err(_) => Value::String(raw.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn scalar_and_typed_values() {
        let mut root = toml::Table::new();
        apply(&mut root, "core.xwayland=true").unwrap();
        apply(&mut root, "core.backend=headless").unwrap();
        apply(&mut root, "keyboard.repeat-rate=40").unwrap();
        assert_eq!(root["core"]["xwayland"], Value::Boolean(true));
        assert_eq!(root["core"]["backend"], Value::String("headless".into()));
        assert_eq!(root["keyboard"]["repeat-rate"], Value::Integer(40));
    }

    #[test]
    fn array_of_tables_index() {
        let mut root = toml::Table::new();
        apply(&mut root, "output.0.mode=off").unwrap();
        apply(&mut root, "output.1.name=HDMI-A-1").unwrap();
        let outs = root["output"].as_array().unwrap();
        assert_eq!(outs[0]["mode"], Value::String("off".into()));
        assert_eq!(outs[1]["name"], Value::String("HDMI-A-1".into()));
    }

    #[test]
    fn malformed_specs_are_errors() {
        let mut root = toml::Table::new();
        assert!(apply(&mut root, "no-equals").is_err());
        assert!(apply(&mut root, "onlyroot=1").is_err());
        assert!(apply(&mut root, "a..b=1").is_err());
    }

    #[test]
    fn rejected_specs_leave_the_tree_untouched() {
        // A half-applied override would deserialize as a bogus empty
        // section if the caller ever kept going after the error.
        let mut root = toml::Table::new();
        assert!(apply(&mut root, "onlyroot=1").is_err());
        assert!(root.is_empty(), "{root:?}");
    }

    #[test]
    fn prefixed_integers_stay_typed_for_the_model_to_widen() {
        let mut root = toml::Table::new();
        apply(&mut root, "shell.background-color=0xff336699").unwrap();
        // TOML reads 0x… as an integer; the model accepts numbers for
        // this key, so the override is not lost.
        assert_eq!(
            root["shell"]["background-color"],
            Value::Integer(0xff336699)
        );
    }
}
```
```rust
//! Resolution (D9–D12): defaults → file → `-o` overrides → flags, into
//! an immutable [`Settings`] resolved once at startup.
//!
//! File discovery mirrors the ini search the C frontend had (P2), with
//! the new name: `$XDG_CONFIG_HOME/westonite.toml`, then
//! `$HOME/.config/westonite.toml`, then each of `$XDG_CONFIG_DIRS`
//! (default `/etc/xdg`).  Both home locations are tried, in that order
//! — libweston's `open_config_file` falls through from
//! `$XDG_CONFIG_HOME` to `$HOME/.config` rather than treating the
//! former as an override, and the C frontend inherits that.
//! `WESTON_CONFIG_FILE` is dropped (D12).  If a legacy
//! `westonite.ini` sits where the TOML is expected, resolution reports
//! a one-line hint and otherwise ignores it (D11).

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::cli::Cli;
use crate::model::Config;
use crate::overrides;

#[derive(Debug)]
pub enum ConfigError {
    /// File read/parse/validation problems — fatal at startup, with
    /// the TOML span in the message (deny_unknown_fields, D9).
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    Drm,
    Headless,
    X11,
    Wayland,
    Rdp,
    Vnc,
    Pipewire,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Backend> {
        // Accept both the short name and the C module-name spelling.
        let s = s.strip_suffix("-backend.so").unwrap_or(s);
        Some(match s {
            "drm" => Backend::Drm,
            "headless" => Backend::Headless,
            "x11" => Backend::X11,
            "wayland" => Backend::Wayland,
            "rdp" => Backend::Rdp,
            "vnc" => Backend::Vnc,
            "pipewire" => Backend::Pipewire,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Backend::Drm => "drm",
            Backend::Headless => "headless",
            Backend::X11 => "x11",
            Backend::Wayland => "wayland",
            Backend::Rdp => "rdp",
            Backend::Vnc => "vnc",
            Backend::Pipewire => "pipewire",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Renderer {
    #[default]
    Auto,
    Gl,
    Pixman,
    Noop,
}

/// The immutable result of resolution.  Consumers receive typed slices
/// of this; nothing re-reads the file or CLI afterwards (§5).
#[derive(Debug, Clone)]
pub struct Settings {
    /// Backends to load, primary first (multi-backend via --backends).
    pub backends: Vec<Backend>,
    pub renderer: Renderer,
    pub socket: Option<String>,
    // No log_file field (PR19-C10): the frontend opens the log from
    // `cli.log` BEFORE resolution runs — config errors must reach the
    // sink — so a resolved copy here was populated and never read.
    pub debug_protocol: bool,

    pub wait_for_debugger: bool,
    pub xwayland: bool,
    pub idle_time: Option<u32>,
    pub modules: Vec<String>,
    pub require_input: bool,
    pub color_management: bool,
    pub gbm_format: Option<String>,

    /// Parsed `[shell] background-color` (0xAARRGGBB), C default.
    pub background_color: u32,
    pub shell_client: Option<String>,

    /// Windowed/headless geometry (CLI-level; per-output config in
    /// `config.output`).
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub scale: Option<i32>,
    /// `--transform` (weston transform-name grammar, validated by the
    /// frontend against the typed enum).
    pub transform: Option<String>,
    pub fullscreen: bool,
    pub output_count: Option<u32>,
    pub no_input: bool,
    pub sprawl: bool,
    /// `--display`: the parent compositor socket for the nested
    /// wayland backend (None = the child's own WAYLAND_DISPLAY).
    pub wayland_display: Option<String>,
    /// `[shell] cursor-theme` / `cursor-size` (wayland backend).
    pub cursor_theme: Option<String>,
    pub cursor_size: i32,
    /// `[pipewire] num-outputs`, C default 1.
    pub pipewire_num_outputs: Option<u32>,
    pub parent_display: Option<String>,
    pub no_outputs: bool,
    pub refresh_rate: Option<i32>,

    pub drm_seat: Option<String>,
    pub drm_device: Option<String>,
    pub drm_additional_devices: Option<String>,
    pub drm_current_mode: bool,
    pub continue_without_input: bool,

    /// Listen port for whichever of rdp/vnc is in `backends` (`--port`
    /// wins over the matching section).
    pub rdp_vnc_port: Option<u16>,
    /// `--address` / `[vnc] address` (vnc bind address).
    pub vnc_bind_address: Option<String>,
    /// `[vnc] refresh-rate`, Hz (no CLI flag in C either).
    pub vnc_refresh_rate: Option<u32>,
    pub vnc_disable_tls: bool,
    pub rdp_tls_cert: Option<String>,
    pub rdp_tls_key: Option<String>,
    pub vnc_tls_cert: Option<String>,
    pub vnc_tls_key: Option<String>,
    pub rdp_external_listener_fd: Option<i32>,
    /// C config field `resizeable` (CLI --no-resizeable inverts it).
    pub rdp_resizeable: bool,
    pub rdp_force_no_compression: bool,
    pub rdp4_key: Option<String>,
    pub rdp_env_socket: bool,
    /// C default true; CLI --no-remotefx-codec inverts.
    pub rdp_remotefx_codec: bool,

    /// Autolaunch command: CLI trailing args win over `[autolaunch]
    /// path`; watch only from the config.
    pub autolaunch: Option<Vec<String>>,
    pub autolaunch_watch: bool,
    pub xwayland_path: PathBuf,

    /// The config file actually used, if any (logged at startup).
    pub config_path: Option<PathBuf>,
    /// D11 hint: a legacy ini was found and ignored.
    pub legacy_ini_found: Option<PathBuf>,

    /// The full validated model, for consumers of structured sections
    /// (outputs, keyboard, libinput, backend sections).
    pub config: Config,
}

/// C `weston_config_section_get_color` (shared/config-parser.c:232):
/// the value is **always base-16** — `"0"` exactly, or 8 hex digits
/// (unprefixed `ff002244` is valid ini), or 10 chars with the `0x`
/// prefix; any other length is invalid.  Two deliberate divergences,
/// both fail-loud where C is silent (D9): C falls back to the default
/// color on any parse error, and C parses a 10-char *unprefixed*
/// value as 40 bits of hex and silently truncates the `uint32_t`
/// assignment — here both are startup errors.  (Bare TOML integers
/// never reach this by-value ambiguity: `model::de_opt_color_string`
/// hands them on pre-formatted as `0x…`.)
fn parse_color(s: &str, what: &str) -> Result<u32, ConfigError> {
    let t = s.trim();
    let invalid = || ConfigError::Invalid(format!("{what}: invalid color '{s}'"));
    if t == "0" {
        return Ok(0);
    }
    if t.len() != 8 && t.len() != 10 {
        return Err(invalid());
    }
    let hex = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    u32::from_str_radix(hex, 16).map_err(|_| invalid())
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

/// File discovery (P2 search order, TOML name).  Returns the chosen
/// path plus any ignored legacy ini next to the search locations.
fn discover(env: &HashMap<String, String>) -> (Option<PathBuf>, Option<PathBuf>) {
    let mut dirs: Vec<PathBuf> = Vec::new();
    // Both home locations, in order — not either/or: libweston tries
    // $XDG_CONFIG_HOME and then falls through to $HOME/.config.
    if let Some(x) = env.get("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(x));
    }
    if let Some(h) = env.get("HOME").filter(|v| !v.is_empty()) {
        let home_config = Path::new(h).join(".config");
        if !dirs.contains(&home_config) {
            dirs.push(home_config);
        }
    }
    // Deliberate divergence, documented in docs/config-migration.md:
    // libweston reads the ini from a hard-coded `weston/` SUBDIR of
    // each entry (/etc/xdg/weston/weston.ini); the TOML lives in the
    // entry itself (/etc/xdg/westonite.toml) — no foreign project name
    // in a rebranded compositor's path.
    match env.get("XDG_CONFIG_DIRS").filter(|v| !v.is_empty()) {
        Some(list) => {
            for d in list.split(':').filter(|d| !d.is_empty()) {
                dirs.push(PathBuf::from(d));
            }
        }
        None => dirs.push(PathBuf::from("/etc/xdg")),
    }

    let mut legacy = None;
    for d in &dirs {
        let toml = d.join("westonite.toml");
        if toml.is_file() {
            return (Some(toml), legacy);
        }
        if legacy.is_none() {
            let ini = d.join("westonite.ini");
            if ini.is_file() {
                legacy = Some(ini);
            }
        }
    }
    (None, legacy)
}

/// Resolve from real process environment.
pub fn resolve(cli: &Cli) -> Result<Settings, ConfigError> {
    let env: HashMap<String, String> = std::env::vars().collect();
    resolve_from(cli, &env)
}

/// Resolve with an explicit environment (unit tests).
pub fn resolve_from(cli: &Cli, env: &HashMap<String, String>) -> Result<Settings, ConfigError> {
    // -- file --
    let mut legacy_ini_found = None;
    let config_path: Option<PathBuf> = if cli.no_config {
        None
    } else if let Some(p) = &cli.config {
        Some(PathBuf::from(p))
    } else {
        let (found, legacy) = discover(env);
        legacy_ini_found = legacy;
        found
    };

    let mut tree: toml::Table = match &config_path {
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| {
                ConfigError::Invalid(format!("cannot read config file '{}': {e}", p.display()))
            })?;
            text.parse()
                .map_err(|e| ConfigError::Invalid(format!("config file '{}': {e}", p.display())))?
        }
        None => toml::Table::new(),
    };

    // -- -o overrides --
    for spec in &cli.set {
        overrides::apply(&mut tree, spec).map_err(ConfigError::Invalid)?;
    }

    // -- deserialize (unknown keys/type errors become startup errors) --
    let config: Config = toml::Value::Table(tree)
        .try_into()
        .map_err(|e| match &config_path {
            Some(p) => ConfigError::Invalid(format!("config file '{}': {e}", p.display())),
            None => ConfigError::Invalid(format!("config overrides: {e}")),
        })?;

    // -- flags on top --
    // --backend/--backends are one C variable (main.c:4458-4459): the
    // last occurrence won and either spelling takes a comma list, so
    // the merged clap field is always split.
    // The two section keys are that same one variable (main.c:4602-6):
    // `backends` is consulted first and `backend` only as its fallback,
    // and whichever answers is comma-split by load_backends — so
    // `backend = "headless,vnc"` is a list too.
    let mut backend_names: Vec<String> = Vec::new();
    if let Some(b) = &cli.backend {
        backend_names.extend(split_list(b));
    } else if !config.core.backends.is_empty() {
        backend_names.extend(config.core.backends.iter().cloned());
    } else if let Some(b) = &config.core.backend {
        backend_names.extend(split_list(b));
    } else {
        // C default: the native backend.
        backend_names.push("drm".to_string());
    }
    let mut backends = Vec::new();
    for name in &backend_names {
        let Some(b) = Backend::parse(name) else {
            // Wording kept from the C frontend: test_cli greps it.
            return Err(ConfigError::Invalid(format!("unknown backend \"{name}\"")));
        };
        if !backends.contains(&b) {
            backends.push(b);
        }
    }

    // C load_headless_backend: use-gl and use-pixman are mutually
    // exclusive, and neither may be combined with an explicit
    // renderer.  Wording kept from the C frontend.
    //
    // Flag OR config key, because that is what C's parse_options does
    // to the variable it already read the key into: the flag can only
    // turn the switch on, never cancel a `use-pixman = true` in the
    // file.  See the model for why the keys apply on every backend
    // here where C honours them only on headless.
    let use_gl = cli.use_gl || config.core.use_gl;
    let use_pixman = cli.use_pixman || config.core.use_pixman;
    let renderer_given = cli.renderer.is_some() || config.core.renderer.is_some();
    if (use_gl && use_pixman) || (renderer_given && (use_gl || use_pixman)) {
        return Err(ConfigError::Invalid(
            "Conflicting renderer specifications".to_string(),
        ));
    }
    let renderer_name = cli
        .renderer
        .clone()
        .or_else(|| {
            if use_gl {
                Some("gl".into())
            } else if use_pixman {
                Some("pixman".into())
            } else {
                None
            }
        })
        .or_else(|| config.core.renderer.clone());
    let renderer = match renderer_name.as_deref() {
        None | Some("auto") => Renderer::Auto,
        Some("gl") => Renderer::Gl,
        Some("pixman") => Renderer::Pixman,
        Some("noop") => Renderer::Noop,
        Some(other) => {
            return Err(ConfigError::Invalid(format!(
                "unknown renderer \"{other}\""
            )));
        }
    };

    let background_color = match &config.shell.background_color {
        Some(s) => parse_color(s, "[shell] background-color")?,
        None => 0xff002244,
    };

    let autolaunch = if !cli.autolaunch.is_empty() {
        Some(cli.autolaunch.clone())
    } else {
        config.autolaunch.path.clone().map(|p| vec![p])
    };

    let modules = match &cli.modules {
        Some(m) => split_list(m),
        None => config.core.modules.clone(),
    };

    // C parse_simple_mode only applies --width/--height when they are
    // non-zero, so a zero silently means "default" there and a negative
    // reaches the backend as a size.  Neither is useful: reject both up
    // front rather than hand a bad geometry to weston_windowed_output.
    for (name, value) in [
        ("--width", cli.width),
        ("--height", cli.height),
        ("--scale", cli.scale),
    ] {
        if let Some(v) = value
            && v <= 0
        {
            return Err(ConfigError::Invalid(format!(
                "{name} must be positive (got {v})"
            )));
        }
    }

    // Same rule for the per-output section scale.  C reads it with
    // weston_config_section_get_int and hands it straight to
    // weston_output_set_scale, whose `assert(output->current_scale)`
    // then aborts on 0 (and a negative scale produces nonsense
    // geometry) — reject both here, where --scale is already rejected.
    for out in &config.output {
        if let Some(v) = out.scale
            && v <= 0
        {
            let name = out.name.as_deref().unwrap_or("<unnamed>");
            return Err(ConfigError::Invalid(format!(
                "[output] '{name}': scale must be positive (got {v})"
            )));
        }
    }

    // Backend-specific sections belong to the backend that is actually
    // loaded: keying only off "vnc first, else rdp" would let a stray
    // [vnc] port hijack an RDP run.
    let has = |b: Backend| backends.contains(&b);
    let section_port = if has(Backend::Vnc) {
        config.vnc.port
    } else if has(Backend::Rdp) {
        config.rdp.port
    } else {
        None
    };

    Ok(Settings {
        backends,
        renderer,
        socket: cli.socket.clone(),
        debug_protocol: cli.debug,
        // C main.c:4585: the flag wins, and only when it is absent is
        // the config consulted -- so `[core] wait-for-debugger = false`
        // cannot cancel `--wait-for-debugger`.
        wait_for_debugger: cli.wait_for_debugger || config.core.wait_for_debugger,
        xwayland: cli.xwayland || config.core.xwayland,
        idle_time: cli.idle_time.or(config.core.idle_time),
        modules,
        require_input: config.core.require_input,
        color_management: config.core.color_management,
        gbm_format: config.core.gbm_format.clone(),
        background_color,
        shell_client: config.shell.client.clone(),
        width: cli.width,
        height: cli.height,
        scale: cli.scale,
        fullscreen: cli.fullscreen,
        output_count: cli.output_count,
        no_input: cli.no_input,
        sprawl: cli.sprawl,
        wayland_display: cli.display.clone(),
        cursor_theme: config.shell.cursor_theme.clone(),
        cursor_size: config.shell.cursor_size.unwrap_or(32),
        pipewire_num_outputs: config.pipewire.num_outputs,
        parent_display: cli.display.clone(),
        no_outputs: cli.no_outputs,
        transform: cli.transform.clone(),
        refresh_rate: cli.refresh_rate,
        drm_seat: cli.seat.clone(),
        drm_device: cli.drm_device.clone(),
        drm_additional_devices: cli.additional_devices.clone(),
        drm_current_mode: cli.current_mode,
        continue_without_input: cli.continue_without_input,
        rdp_vnc_port: cli.port.or(section_port),
        vnc_bind_address: cli.address.clone().or_else(|| config.vnc.address.clone()),
        vnc_refresh_rate: config.vnc.refresh_rate,
        vnc_disable_tls: cli.disable_transport_layer_security
            || config.vnc.disable_transport_layer_security.unwrap_or(false),
        rdp_tls_cert: cli
            .rdp_tls_cert
            .clone()
            .or_else(|| config.rdp.tls_cert.clone()),
        rdp_tls_key: cli
            .rdp_tls_key
            .clone()
            .or_else(|| config.rdp.tls_key.clone()),
        vnc_tls_cert: cli
            .vnc_tls_cert
            .clone()
            .or_else(|| config.vnc.tls_cert.clone()),
        vnc_tls_key: cli
            .vnc_tls_key
            .clone()
            .or_else(|| config.vnc.tls_key.clone()),
        rdp_external_listener_fd: cli.external_listener_fd.or(config.rdp.external_listener_fd),
        rdp_resizeable: !cli.no_resizeable && config.rdp.resizeable.unwrap_or(true),
        rdp_force_no_compression: cli.force_no_compression
            || config.rdp.force_no_compression.unwrap_or(false),
        rdp4_key: cli.rdp4_key.clone(),
        rdp_env_socket: cli.env_socket,
        rdp_remotefx_codec: !cli.no_remotefx_codec && config.rdp.remotefx_codec.unwrap_or(true),
        autolaunch,
        // C main.c execute_command: a positional command line is always
        // watched; config [autolaunch] watch applies otherwise.
        autolaunch_watch: config.autolaunch.watch || !cli.autolaunch.is_empty(),
        xwayland_path: config
            .xwayland
            .path
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/usr/bin/Xwayland")),
        config_path,
        legacy_ini_found,
        config,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clap::Parser;

    fn cli(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("westonite").chain(args.iter().copied()))
    }

    fn no_env() -> HashMap<String, String> {
        HashMap::new()
    }

    /// Every key in `westonite.toml.example` must be a real key, in
    /// the right section.
    ///
    /// The example ships fully commented out, so nothing else ever
    /// parses it -- which is how `wait-for-debugger` (a `[core]` key)
    /// came to sit under `[shell]` for a slice without anyone
    /// noticing.  Uncommenting every `#key = value` line and feeding
    /// the result to the same `deny_unknown_fields` model the real
    /// loader uses turns the example into something the compiler
    /// checks.
    ///
    /// Commented section headers (`#[[output]]`) are restored along
    /// with the keys; prose comments keep their `#` and are dropped.
    #[test]
    fn example_config_is_valid() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../westonite.toml.example");
        let text =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
        let uncommented: String = text
            .lines()
            .filter_map(|line| {
                let t = line.trim_start();
                match t.strip_prefix('#') {
                    // A commented section header: `#[[output]]`.  These
                    // must come back too, or the keys under them float
                    // up into the previous section and the check fails
                    // for the wrong reason.
                    Some(rest) if rest.starts_with('[') && rest.trim_end().ends_with(']') => {
                        Some(rest.to_string())
                    }
                    // A commented key: `#name = value`.  Prose comments
                    // have a space or punctuation after the `#`.
                    Some(rest)
                        if rest.contains(" = ")
                            && rest.split(" = ").next().is_some_and(|k| {
                                !k.is_empty()
                                    && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                            }) =>
                    {
                        Some(rest.to_string())
                    }
                    Some(_) => None,
                    // Section headers and blank lines pass through.
                    None => Some(line.to_string()),
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let parsed: Result<Config, _> = toml::from_str(&uncommented);
        assert!(
            parsed.is_ok(),
            "westonite.toml.example does not match the config model: {}\n\
             --- reconstructed ---\n{uncommented}",
            parsed.unwrap_err()
        );
    }

    #[test]
    fn defaults_match_c() {
        let s = resolve_from(&cli(&["--no-config"]), &no_env()).unwrap();
        assert_eq!(s.backends, vec![Backend::Drm]);
        assert_eq!(s.renderer, Renderer::Auto);
        assert_eq!(s.background_color, 0xff002244);
        assert!(s.require_input);
        assert!(s.autolaunch.is_none());
        assert_eq!(s.xwayland_path, PathBuf::from("/usr/bin/Xwayland"));
    }

    #[test]
    fn unknown_backend_message_matches_c() {
        let err = resolve_from(&cli(&["--no-config", "--backend=bogus"]), &no_env())
            .unwrap_err()
            .to_string();
        assert_eq!(err, "unknown backend \"bogus\"");
    }

    #[test]
    fn file_then_override_then_flag_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(
            &path,
            "[core]\nbackend = \"headless\"\nrenderer = \"pixman\"\n\
             [shell]\nbackground-color = \"0xff336699\"\n",
        )
        .unwrap();
        let c = cli(&[
            &format!("--config={}", path.display()),
            "-o",
            "core.renderer=noop",
            "--backend=vnc",
        ]);
        let s = resolve_from(&c, &no_env()).unwrap();
        assert_eq!(s.backends, vec![Backend::Vnc]); // flag beats file
        assert_eq!(s.renderer, Renderer::Noop); // -o beats file
        assert_eq!(s.background_color, 0xff336699);
    }

    #[test]
    fn unknown_key_is_a_startup_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(&path, "[core]\nbakend = \"headless\"\n").unwrap();
        let err = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env())
            .unwrap_err()
            .to_string();
        assert!(err.contains("bakend"), "{err}");
    }

    #[test]
    fn xdg_discovery_and_legacy_ini_hint() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = no_env();
        env.insert(
            "XDG_CONFIG_HOME".into(),
            dir.path().to_string_lossy().into_owned(),
        );
        // Only a legacy ini: ignored, but reported.
        std::fs::write(dir.path().join("westonite.ini"), "[core]\n").unwrap();
        let s = resolve_from(&cli(&[]), &env).unwrap();
        assert!(s.config_path.is_none());
        assert_eq!(s.legacy_ini_found, Some(dir.path().join("westonite.ini")));
        // A real toml wins.
        std::fs::write(dir.path().join("westonite.toml"), "[core]\n").unwrap();
        let s = resolve_from(&cli(&[]), &env).unwrap();
        assert_eq!(s.config_path, Some(dir.path().join("westonite.toml")));
    }

    #[test]
    fn home_fallback_when_no_xdg_config_home() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".config")).unwrap();
        std::fs::write(dir.path().join(".config/westonite.toml"), "[core]\n").unwrap();
        let mut env = no_env();
        env.insert("HOME".into(), dir.path().to_string_lossy().into_owned());
        let s = resolve_from(&cli(&[]), &env).unwrap();
        assert_eq!(
            s.config_path,
            Some(dir.path().join(".config/westonite.toml"))
        );
    }

    #[test]
    fn autolaunch_trailing_args_beat_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(
            &path,
            "[autolaunch]\npath = \"/bin/cfg-app\"\nwatch = true\n",
        )
        .unwrap();
        let c = cli(&[
            &format!("--config={}", path.display()),
            "--",
            "/bin/cli-app",
            "--flag",
        ]);
        let s = resolve_from(&c, &no_env()).unwrap();
        assert_eq!(
            s.autolaunch,
            Some(vec!["/bin/cli-app".to_string(), "--flag".to_string()])
        );
        assert!(s.autolaunch_watch);

        let s2 = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
        assert_eq!(s2.autolaunch, Some(vec!["/bin/cfg-app".to_string()]));
    }

    #[test]
    fn positional_command_is_always_watched() {
        // C execute_command sets autolaunch_watch = true unconditionally.
        let s = resolve_from(&cli(&["--no-config", "--", "/bin/app"]), &no_env()).unwrap();
        assert!(s.autolaunch_watch);
        // ... while a config path without watch= stays unwatched.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(&path, "[autolaunch]\npath = \"/bin/cfg-app\"\n").unwrap();
        let s2 = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
        assert!(!s2.autolaunch_watch);
    }

    #[test]
    fn hex_color_override_is_not_a_type_error() {
        // TOML parses 0x… as an integer, so the documented spelling
        // `-o shell.background-color=0xff336699` must still resolve.
        let s = resolve_from(
            &cli(&["--no-config", "-o", "shell.background-color=0xff336699"]),
            &no_env(),
        )
        .unwrap();
        assert_eq!(s.background_color, 0xff336699);
    }

    #[test]
    fn color_accepts_quoted_and_bare_spellings() {
        let dir = tempfile::tempdir().unwrap();
        for (i, body) in [
            "background-color = \"0xff336699\"",
            "background-color = 0xff336699",
            "background-color = 4281558681",
        ]
        .iter()
        .enumerate()
        {
            let path = dir.path().join(format!("c{i}.toml"));
            std::fs::write(&path, format!("[shell]\n{body}\n")).unwrap();
            let s =
                resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
            assert_eq!(s.background_color, 0xff336699, "{body}");
        }
    }

    #[test]
    fn color_strings_are_base_16_like_c() {
        // C weston_config_section_get_color is strtoul(..., 16) behind
        // a length gate: an unprefixed 8-digit string is hex — so
        // "ff336699" works, and all-decimal-digits "12345678" means
        // 0x12345678, NOT twelve million (PR19-C6: the port used to
        // read unprefixed values as decimal).  "0" alone is C's other
        // accepted spelling.
        let dir = tempfile::tempdir().unwrap();
        for (i, (value, want)) in [
            ("ff336699", 0xff336699u32),
            ("12345678", 0x12345678),
            ("0", 0),
        ]
        .iter()
        .enumerate()
        {
            let path = dir.path().join(format!("h{i}.toml"));
            std::fs::write(&path, format!("[shell]\nbackground-color = \"{value}\"\n")).unwrap();
            let s =
                resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
            assert_eq!(s.background_color, *want, "{value}");
        }
    }

    #[test]
    fn color_rejects_what_c_rejects_but_loudly() {
        // C's length gate (8 or 10 chars, or exactly "0") — where C
        // silently substitutes the default color, resolution fails
        // (D9).  The 10-digit unprefixed case is the one C accepts
        // and silently TRUNCATES to 32 bits; that is a loud error
        // here too.
        let dir = tempfile::tempdir().unwrap();
        for (i, value) in ["0x1234", "ff33669", "ff00224411", "zzzzzzzz"]
            .iter()
            .enumerate()
        {
            let path = dir.path().join(format!("b{i}.toml"));
            std::fs::write(&path, format!("[shell]\nbackground-color = \"{value}\"\n")).unwrap();
            let err = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env());
            assert!(err.is_err(), "'{value}' should be rejected");
        }
    }

    #[test]
    fn deprecated_enable_tap_spelling_reaches_the_model() {
        // C honors the underscore spelling behind a deprecation
        // warning (main.c:2260); the re-spec drops it, but the key
        // must PARSE — the frontend refuses it with a message naming
        // the rename (see westonite's refusal table), which a generic
        // unknown-field death here would preempt.  It must also stay
        // a separate field: aliasing it onto enable-tap would silently
        // honor it instead.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(&path, "[libinput]\nenable_tap = true\n").unwrap();
        let s = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
        assert_eq!(s.config.libinput.enable_tap_deprecated, Some(true));
        assert_eq!(s.config.libinput.enable_tap, None);
    }

    #[test]
    fn list_keys_accept_comma_strings_and_arrays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        // The ini spelling survives the near-diagonal D11 mapping.
        std::fs::write(&path, "[core]\nbackends = \"headless,vnc\"\n").unwrap();
        let s = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
        assert_eq!(s.backends, vec![Backend::Headless, Backend::Vnc]);

        std::fs::write(&path, "[core]\nmodules = [\"a.so\", \"b.so\"]\n").unwrap();
        let s = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
        assert_eq!(s.modules, vec!["a.so".to_string(), "b.so".to_string()]);

        let s = resolve_from(
            &cli(&["--no-config", "-o", "core.modules=a.so,b.so"]),
            &no_env(),
        )
        .unwrap();
        assert_eq!(s.modules, vec!["a.so".to_string(), "b.so".to_string()]);
    }

    #[test]
    fn backend_and_backends_are_one_variable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        let conf = |args: &[&str]| {
            let mut argv = vec![format!("--config={}", path.display())];
            argv.extend(args.iter().map(|a| a.to_string()));
            let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
            resolve_from(&cli(&argv), &no_env()).unwrap().backends
        };

        // C main.c:4602-4606 reads `backends` first and falls back to
        // `backend`; with both present, `backends` wins.
        std::fs::write(
            &path,
            "[core]\nbackend = \"headless\"\nbackends = \"vnc,headless\"\n",
        )
        .unwrap();
        assert_eq!(conf(&[]), vec![Backend::Vnc, Backend::Headless]);

        // The singular key feeds the same comma-split variable.
        std::fs::write(&path, "[core]\nbackend = \"headless,vnc\"\n").unwrap();
        assert_eq!(conf(&[]), vec![Backend::Headless, Backend::Vnc]);

        // Either CLI spelling takes a list, and the last one wins.
        assert_eq!(
            conf(&["--backend=vnc", "--backends=headless,vnc"]),
            vec![Backend::Headless, Backend::Vnc]
        );
        assert_eq!(
            conf(&["--backends=headless,vnc", "--backend=vnc"]),
            vec![Backend::Vnc]
        );
    }

    #[test]
    fn xdg_config_home_falls_through_to_home_config() {
        // libweston's open_config_file tries $XDG_CONFIG_HOME and then
        // $HOME/.config; an empty XDG_CONFIG_HOME must not mask a config
        // sitting in the home fallback.
        let dir = tempfile::tempdir().unwrap();
        let xdg = dir.path().join("xdg");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&xdg).unwrap();
        std::fs::create_dir_all(home.join(".config")).unwrap();
        std::fs::write(home.join(".config/westonite.toml"), "[core]\n").unwrap();
        let mut env = no_env();
        env.insert("XDG_CONFIG_HOME".into(), xdg.to_string_lossy().into_owned());
        env.insert("HOME".into(), home.to_string_lossy().into_owned());
        let s = resolve_from(&cli(&[]), &env).unwrap();
        assert_eq!(s.config_path, Some(home.join(".config/westonite.toml")));
    }

    #[test]
    fn conflicting_renderer_flags_are_rejected() {
        for args in [
            vec!["--no-config", "--use-gl", "--use-pixman"],
            vec!["--no-config", "--renderer=gl", "--use-pixman"],
        ] {
            let err = resolve_from(&cli(&args), &no_env())
                .unwrap_err()
                .to_string();
            assert_eq!(err, "Conflicting renderer specifications", "{args:?}");
        }
    }

    #[test]
    fn non_positive_geometry_is_rejected() {
        for (flag, args) in [
            ("--width", vec!["--no-config", "--width=0"]),
            ("--height", vec!["--no-config", "--height=-1"]),
            ("--scale", vec!["--no-config", "--scale=0"]),
        ] {
            let err = resolve_from(&cli(&args), &no_env())
                .unwrap_err()
                .to_string();
            assert!(err.starts_with(flag), "{err}");
        }
    }

    #[test]
    fn non_positive_output_section_scale_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(&path, "[[output]]\nname = \"headless\"\nscale = 0\n").unwrap();
        let cfg = format!("--config={}", path.display());
        let err = resolve_from(&cli(&[&cfg]), &no_env())
            .unwrap_err()
            .to_string();
        assert_eq!(err, "[output] 'headless': scale must be positive (got 0)");

        std::fs::write(&path, "[[output]]\nname = \"headless\"\nscale = 2\n").unwrap();
        let s = resolve_from(&cli(&[&cfg]), &no_env()).unwrap();
        assert_eq!(s.config.output[0].scale, Some(2));
    }

    #[test]
    fn backend_sections_do_not_leak_across_backends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(&path, "[vnc]\nport = 5900\n[rdp]\nport = 3389\n").unwrap();
        let cfg = format!("--config={}", path.display());
        let s = resolve_from(&cli(&[&cfg, "--backend=rdp"]), &no_env()).unwrap();
        assert_eq!(s.rdp_vnc_port, Some(3389));
        let s = resolve_from(&cli(&[&cfg, "--backend=vnc"]), &no_env()).unwrap();
        assert_eq!(s.rdp_vnc_port, Some(5900));
        // Neither backend loaded: no port at all, rather than a stray one.
        let s = resolve_from(&cli(&[&cfg, "--backend=headless"]), &no_env()).unwrap();
        assert_eq!(s.rdp_vnc_port, None);
        // --port still wins.
        let s = resolve_from(&cli(&[&cfg, "--backend=vnc", "--port=1234"]), &no_env()).unwrap();
        assert_eq!(s.rdp_vnc_port, Some(1234));
    }

    #[test]
    fn output_sections_pass_through_typed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("westonite.toml");
        std::fs::write(
            &path,
            "[[output]]\nname = \"headless\"\nmode = \"800x600\"\nscale = 2\n\
             [[output]]\nname = \"X1\"\nmode = \"off\"\n",
        )
        .unwrap();
        let s = resolve_from(&cli(&[&format!("--config={}", path.display())]), &no_env()).unwrap();
        assert_eq!(s.config.output.len(), 2);
        assert_eq!(s.config.output[0].scale, Some(2));
        assert_eq!(s.config.output[1].mode.as_deref(), Some("off"));
    }
}
```

### A.32 `crates/westonite/src/main.rs` — copy, then apply

Deltas: in `reject_unported`, add the `[rdp]`-section refusal next to the `Backend::Rdp` one, add `(cli.idle_time.is_some(), "--idle-time")` to `unconsumed_flags`, and add the `[core] idle-time` refusal (§5.4 item 6) before the calibrator refusals. Everything else as-is.

```rust
//! The westonite frontend (plan §4: the main.c port, growing by slice).
//!
//! R2a slice: CLI/config (§5 re-spec — TOML + clap + `-o`), logging,
//! XDG_RUNTIME_DIR verification, the headless backend, the statically
//! linked Rust shell, autolaunch (via `westonite-spawn`), SIGCHLD
//! watch, clean signal-driven shutdown.
//! R2b slice: real output management — `[[output]]` mode/scale/
//! transform/off resolved into an [`weston::OutputPolicy`], CLI
//! `--scale`/`--transform`/`--no-outputs`/`--refresh-rate`, hotplug
//! enable/disable via the policy-driven heads-changed handler.
//! Everything not yet ported fails loudly (never silently degrades) —
//! the C frontend remains the oracle for those paths until their slice
//! lands (plan §7).

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use westonite_config::{Backend, Cli, Renderer, Settings};

/// A startup-fatal error: logged through weston_log (so it lands in
/// `--log` when one is set, stderr otherwise), then exit(1) — the C
/// frontend's `goto out` contract.
fn fatal(msg: &str) -> ExitCode {
    weston::log::message(&format!("fatal: {msg}"));
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.version {
        println!("westonite {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    // Logging first, in C's order (main.c:4499): the fallback handlers
    // so the next few lines can fail loudly, then the real log context
    // -- scope, log file, subscribers -- before the banner.
    weston::log::install_stderr_handlers();
    let log = match weston::log::LogContext::new(&weston::log::LogSetup {
        file: cli.log.as_deref().map(std::path::PathBuf::from),
        logger_scopes: split_scopes(cli.logger_scopes.as_deref()),
        flight_rec_scopes: cli
            .flight_rec_scopes
            .as_deref()
            .map(|v| split_scopes(Some(v))),
    }) {
        Ok(l) => l,
        // C exits here too (main.c:4501/4509): with no log context and
        // no log file there is nowhere for the session's diagnostics to
        // go, and starting anyway would hide every later failure.
        Err(e) => return fatal(&e.to_string()),
    };

    weston::log::message(&format!(
        "westonite {} (Rust frontend, weston 14 based)",
        env!("CARGO_PKG_VERSION")
    ));
    let cmdline: Vec<String> = std::env::args().collect();
    weston::log::message(&format!("Command line: {}", cmdline.join(" ")));
    // C main.c:4534, and in the same place: whether the in-memory
    // recorder is running is the first thing you want to know when
    // reading a log from a session that went wrong.
    weston::log::message(&format!(
        "Flight recorder: {}",
        if log.flight_rec_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    ));

    if let Some(code) = verify_xdg_runtime_dir() {
        return code;
    }

    let settings = match westonite_config::resolve(&cli) {
        Ok(s) => s,
        Err(e) => return fatal(&e.to_string()),
    };

    match &settings.config_path {
        Some(p) => weston::log::message(&format!("Using config file '{}'", p.display())),
        None => weston::log::message("Starting with no config file."),
    }
    if let Some(ini) = &settings.legacy_ini_found {
        weston::log::message(&format!(
            "warning: found legacy ini config '{}' and ignored it: westonite reads \
             westonite.toml now (see docs/config-migration.md for the ini mapping)",
            ini.display()
        ));
    }

    if let Some(code) = reject_unported(&cli, &settings) {
        return code;
    }

    // C main.c:4589, and in the same place: after the config is loaded
    // (so `[core] wait-for-debugger` counts) but before anything is
    // created, which is the point of stopping at all.
    if settings.wait_for_debugger {
        weston::wait_for_debugger();
    }

    let policy = match build_output_policy(&settings) {
        Ok(p) => p,
        Err(msg) => return fatal(&msg),
    };
    let input_config = match build_input_config(&settings.config.libinput) {
        Ok(c) => c,
        Err(msg) => return fatal(&msg),
    };
    let renderer = match settings.renderer {
        // C load_backend passes the one global choice, AUTO included,
        // into every loader's config.renderer and lets each backend
        // resolve it (headless → noop, vnc → pixman); the first backend
        // to reach `if (!compositor->renderer)` wins for the whole
        // compositor.  Resolving AUTO here instead would make the
        // result depend on our resolution rather than the load order —
        // `--backends=vnc,headless` would hand the VNC backend
        // WESTON_RENDERER_NOOP, which it rejects outright ("unsupported
        // renderer", vnc.c) where C comes up on pixman.
        Renderer::Auto => weston::RendererKind::Auto,
        Renderer::Noop => weston::RendererKind::Noop,
        Renderer::Gl => weston::RendererKind::Gl,
        Renderer::Pixman => weston::RendererKind::Pixman,
    };

    let kb = &settings.config.keyboard;
    let mut builder = weston::CompositorBuilder::new()
        .with_log_context(&log)
        .renderer(renderer)
        .with_output_policy(policy)
        .with_keyboard(weston::KeyboardConfig {
            rules: kb.keymap_rules.clone(),
            model: kb.keymap_model.clone(),
            layout: kb.keymap_layout.clone(),
            variant: kb.keymap_variant.clone(),
            options: kb.keymap_options.clone(),
            repeat_rate: kb.repeat_rate.and_then(|v| i32::try_from(v).ok()),
            repeat_delay: kb.repeat_delay.and_then(|v| i32::try_from(v).ok()),
            vt_switching: kb.vt_switching,
        });
    if let Some(msec) = settings.config.core.repaint_window {
        builder = builder.with_repaint_window_msec(msec);
    }
    builder = builder.color_management(settings.color_management);
    builder = builder.debug_protocol(settings.debug_protocol);
    if let Some(r) = &settings.config.core.require_outputs {
        match weston::RequireOutputs::parse(r) {
            Some(r) => builder = builder.require_outputs(r),
            None => {
                return fatal(&format!(
                    "[core] require-outputs: unknown value \"{r}\" (any|all|none)"
                ));
            }
        }
    }
    // C load_backends: comma-list order, primary first.
    for b in &settings.backends {
        builder = match b {
            Backend::Headless => builder.add_headless(weston::HeadlessOptions {
                no_outputs: settings.no_outputs,
                refresh_mhz: settings.refresh_rate,
                decorate: settings.config.core.output_decorations,
            }),
            Backend::Vnc => builder.add_vnc(weston::VncOptions {
                bind_address: settings.vnc_bind_address.clone(),
                port: settings.rdp_vnc_port.map(i32::from),
                refresh_rate_hz: settings
                    .vnc_refresh_rate
                    .and_then(|v| i32::try_from(v).ok()),
                tls_cert: settings.vnc_tls_cert.clone(),
                tls_key: settings.vnc_tls_key.clone(),
                disable_tls: settings.vnc_disable_tls,
            }),
            Backend::X11 => builder.add_x11(weston::X11Options {
                fullscreen: settings.fullscreen,
                no_input: settings.no_input,
                output_count: settings
                    .output_count
                    .and_then(|v| i32::try_from(v).ok())
                    .unwrap_or(1),
            }),
            Backend::Wayland => builder.add_wayland(weston::WaylandOptions {
                display_name: settings.wayland_display.clone(),
                fullscreen: settings.fullscreen,
                sprawl: settings.sprawl,
                output_count: settings
                    .output_count
                    .and_then(|v| i32::try_from(v).ok())
                    .unwrap_or(1),
                cursor_theme: settings.cursor_theme.clone(),
                cursor_size: settings.cursor_size,
            }),
            Backend::Pipewire => builder.add_pipewire(weston::PipewireOptions {
                gbm_format: settings.gbm_format.clone(),
                num_outputs: settings
                    .pipewire_num_outputs
                    .and_then(|v| i32::try_from(v).ok())
                    .unwrap_or(1),
            }),
            Backend::Drm => builder.add_drm(weston::DrmOptions {
                seat_id: settings.drm_seat.clone(),
                specific_device: settings.drm_device.clone(),
                additional_devices: settings.drm_additional_devices.clone(),
                gbm_format: settings.gbm_format.clone(),
                pageflip_timeout: settings.config.core.pageflip_timeout.unwrap_or(0),
                use_pixman_shadow: settings.config.core.pixman_shadow.unwrap_or(true),
                current_mode: settings.drm_current_mode,
                continue_without_input: settings.continue_without_input,
                input: input_config.clone(),
            }),
            // reject_unported() has already refused rdp, a permanent
            // product decision — see there.
            _ => builder,
        };
    }
    builder = match &settings.socket {
        Some(name) => builder.with_socket_name(name),
        None => builder.with_socket(),
    };
    builder = builder.with_shell(settings.background_color, |bg| {
        Box::new(westonite_shell::Shell::new(westonite_shell::ShellConfig {
            background_color: bg,
        }))
    });
    // C main.c:4725: --xwayland / [core] xwayland loads the module
    // (lazy server spawn from [xwayland] path).
    if settings.xwayland {
        builder = builder.with_xwayland(settings.xwayland_path.clone());
    }

    let mut compositor = match builder.build() {
        Ok(c) => c,
        Err(e) => return fatal(&e.to_string()),
    };

    if let Some(code) = start_autolaunch(&cli, &settings, &compositor) {
        return code;
    }

    let exit_code = compositor.run();
    drop(compositor);
    match u8::try_from(exit_code) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(n) => ExitCode::from(n),
        Err(_) => ExitCode::FAILURE,
    }
}

/// C main.c verify_xdg_runtime_dir: unset or non-directory is fatal
/// (the message wording is pinned by test_cli), wrong mode *or owner*
/// is a warning.
fn verify_xdg_runtime_dir() -> Option<ExitCode> {
    use std::os::unix::fs::MetadataExt;
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return Some(fatal(
            "environment variable XDG_RUNTIME_DIR is not set.\n\
             Refer to your distribution on how to get it, or\n\
             http://www.freedesktop.org/wiki/Specifications/basedir-spec\n\
             on how to implement it.",
        ));
    };
    let path = PathBuf::from(&dir);
    let not_a_dir = || {
        Some(fatal(&format!(
            "environment variable XDG_RUNTIME_DIR is set to \"{}\", which is not a directory.",
            path.display()
        )))
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return not_a_dir();
    };
    if !meta.is_dir() {
        return not_a_dir();
    }
    // C masks with 0777 — the setuid/setgid/sticky bits are not part of
    // the check, and the owner must be us.
    let mode = meta.mode() & 0o777;
    // Through westonite-spawn: the getuid call itself belongs in the
    // audited-unsafe crate, so this one keeps forbid(unsafe_code).
    let uid = westonite_spawn::real_uid();
    if mode != 0o700 || meta.uid() != uid {
        weston::log::message(&format!(
            "warning: XDG_RUNTIME_DIR \"{}\" is not configured correctly: unix access \
             mode must be 0700 (current mode is {mode:04o}), and it must be owned by \
             UID {uid} (current owner is UID {})",
            path.display(),
            meta.uid()
        ));
    }
    None
}

/// Fail-loud gate: options whose behavior is not yet ported are
/// startup errors, never silent no-ops (the C frontend stays the
/// oracle for them — plan §7).
///
/// `[[output]]` sections are checked whether or not a head of that name
/// will show up.  C would leave an unmatched section inert, but "inert"
/// and "honoured" are indistinguishable to someone who wrote
/// `clone-of` and got no output and no message — so an unported key is
/// fatal wherever it appears.  `build_output_policy` validates the
/// ported keys on the same all-sections basis.
fn reject_unported(cli: &Cli, settings: &Settings) -> Option<ExitCode> {
    // RDP is a deliberate product decision, not a gap: it stays
    // unimplemented, so say so in its own words rather than promising a
    // future slice (see PROVENANCE / plan §7).
    if settings.backends.contains(&Backend::Rdp) {
        return Some(fatal(
            "the rdp backend is not supported by westonite (deliberately dropped); \
             use the vnc backend for remote access",
        ));
    }
    // [remote-output] — the remoting plugin — is dropped by the same
    // kind of product decision (2026-07-31).  Not deprecated upstream
    // (weston 15.0.90 still builds it by default), just not something
    // westonite ships: it streams an output over GStreamer/RTP, works
    // only on the DRM backend, and westonite's remote story is VNC.
    //
    // Note this was a SILENT no-op before this refusal existed: the
    // section parsed into the config model and nothing ever read it,
    // which is exactly the failure mode the fail-loud rule exists to
    // prevent.
    if !settings.config.remote_output.is_empty() {
        return Some(fatal(
            "[[remote-output]] is not supported by westonite (the remoting plugin is \
             deliberately dropped); use the vnc backend for remote access",
        ));
    }
    // [pipewire-output] — the pipewire *plugin* — is dropped alongside
    // remoting (2026-07-31).  Weston keeps these as two independent
    // build options and they are genuinely different features:
    //   backend-pipewire  "PipeWire backend: screencasting via PipeWire"
    //   pipewire          "Virtual remote output with Pipewire on DRM backend"
    // The BACKEND is ported (--backend=pipewire, R2c-nested); it is
    // only this DRM-grafted virtual output that westonite does not
    // ship.  Neither is deprecated upstream — both are still default-on
    // in weston 15.0.90 — so this is a product decision, like RDP and
    // remoting, not a reaction to upstream removing anything.
    if !settings.config.pipewire_output.is_empty() {
        return Some(fatal(
            "[[pipewire-output]] is not supported by westonite (the pipewire virtual-output \
             plugin is deliberately dropped); the pipewire *backend* (--backend=pipewire) \
             is supported",
        ));
    }
    // (There used to be a catch-all "backend not yet ported" refusal
    // here.  Every Backend variant is now either loaded or refused by
    // name above -- Rdp returns early with its own message -- so the
    // filter could never match, and a refusal that cannot fire is worse
    // than none: it reads like a live guard.)

    // C consumes CLI options per backend loader; anything left over is
    // `fatal: unhandled option`.  Same contract here, as a table: a
    // flag whose consuming backend is not loaded is a startup error,
    // not a silent no-op.
    let has_headless = settings.backends.contains(&Backend::Headless);
    let has_vnc = settings.backends.contains(&Backend::Vnc);
    let has_x11 = settings.backends.contains(&Backend::X11);
    let has_wayland = settings.backends.contains(&Backend::Wayland);
    let has_pipewire = settings.backends.contains(&Backend::Pipewire);
    let has_drm = settings.backends.contains(&Backend::Drm);
    // One row per flag, with the set of loaded backends that would
    // consume it — transcribed from the C `weston_option` tables:
    // headless main.c:3497, x11 3947, wayland 4070, vnc 3707,
    // pipewire 3625.  (--width/--height are in every table, so they
    // never appear here.)
    let per_backend_flags = [
        (cli.seat.is_some(), "--seat", has_drm),
        (cli.drm_device.is_some(), "--drm-device", has_drm),
        (
            cli.additional_devices.is_some(),
            "--additional-devices",
            has_drm,
        ),
        (cli.current_mode, "--current-mode", has_drm),
        (
            cli.continue_without_input,
            "--continue-without-input",
            has_drm,
        ),
        (
            settings.scale.is_some(),
            "--scale",
            has_headless || has_x11 || has_wayland,
        ),
        (settings.transform.is_some(), "--transform", has_headless),
        (settings.no_outputs, "--no-outputs", has_headless),
        (
            settings.refresh_rate.is_some(),
            "--refresh-rate",
            has_headless,
        ),
        (cli.use_gl, "--use-gl", has_headless),
        (
            cli.use_pixman,
            "--use-pixman",
            has_headless || has_x11 || has_wayland,
        ),
        (cli.fullscreen, "--fullscreen", has_x11 || has_wayland),
        (
            cli.output_count.is_some(),
            "--output-count",
            has_x11 || has_wayland,
        ),
        (cli.no_input, "--no-input", has_x11),
        (cli.sprawl, "--sprawl", has_wayland),
        (cli.display.is_some(), "--display", has_wayland),
        (cli.port.is_some(), "--port", has_vnc),
        (cli.address.is_some(), "--address", has_vnc),
        (cli.vnc_tls_cert.is_some(), "--vnc-tls-cert", has_vnc),
        (cli.vnc_tls_key.is_some(), "--vnc-tls-key", has_vnc),
        (
            cli.disable_transport_layer_security,
            "--disable-transport-layer-security",
            has_vnc,
        ),
    ];
    // `has_pipewire` has no flags of its own (C's pipewire table is
    // width/height only); named so the set stays visibly complete.
    let _ = has_pipewire;
    for (set, flag, consumed_by_a_loaded_backend) in per_backend_flags {
        if set && !consumed_by_a_loaded_backend {
            return Some(unhandled_option(flag));
        }
    }
    // Consumed by no ported backend at all: the drm-only options, and
    // the rdp ones (a backend westonite deliberately does not ship).
    // Reaching here means the flag is left over unconditionally — C's
    // `unhandled option`.
    let unconsumed_flags = [
        (cli.rdp_tls_cert.is_some(), "--rdp-tls-cert"),
        (cli.rdp_tls_key.is_some(), "--rdp-tls-key"),
        (cli.external_listener_fd.is_some(), "--external-listener-fd"),
        (cli.no_resizeable, "--no-resizeable"),
        (cli.rdp4_key.is_some(), "--rdp4-key"),
        (cli.env_socket, "--env-socket"),
        (cli.no_remotefx_codec, "--no-remotefx-codec"),
        (cli.force_no_compression, "--force-no-compression"),
    ];
    for (set, flag) in unconsumed_flags {
        if set {
            return Some(unhandled_option(flag));
        }
    }
    // [libinput] touchscreen-calibrator / calibration-helper --
    // libweston's weston_touch_calibration protocol plus a helper C
    // runs through system() (main.c:1075/1214) -- dropped by product
    // decision (2026-08-01), not deferred.  It is a different feature
    // from the per-device settings around it, and westonite ships no
    // client that could drive it: upstream's weston-touch-calibrator
    // is not in our package, and without a touchscreen there is
    // nothing to test against either.
    //
    // Not gated on DRM: C enables the calibrator from
    // weston_compositor_init_config, whatever the backend.
    if settings
        .config
        .libinput
        .touchscreen_calibrator
        .unwrap_or(false)
    {
        return Some(fatal(
            "[libinput] touchscreen-calibrator is not supported by westonite (the touch \
             calibration protocol is deliberately dropped; no calibrator client ships \
             with it)",
        ));
    }
    if settings
        .config
        .libinput
        .calibration_helper
        .as_deref()
        .is_some_and(|h| !h.is_empty())
    {
        return Some(fatal(
            "[libinput] calibration-helper is only consulted by touchscreen-calibrator, \
             which is not supported by westonite",
        ));
    }
    // C's deprecated `enable_tap` underscore spelling — honored there
    // behind a "!!DEPRECATION WARNING!!" (main.c:2260-2267) — is
    // dropped in the re-spec: one spelling, `enable-tap`.  The key
    // stays in the model so this can say what to type instead of
    // pointing a generic unknown-field error at a spelling the C docs
    // never used (the user typed what their old weston.ini had).
    if settings.config.libinput.enable_tap_deprecated.is_some() {
        return Some(fatal(
            "[libinput] enable_tap is C's deprecated spelling and is not supported by \
             westonite; spell it enable-tap",
        ));
    }
    // `--modules` / `[core] modules` — third-party `wet_module_init`
    // plugins — dropped by product decision (2026-08-01), reversing
    // plan D2's "modules= dlopen survives".  Nothing westonite ships
    // uses it: the shell is linked in (D2), and the two plugins that
    // did load this way (remoting, pipewire-output) are themselves
    // dropped.  What it would cost is the whole reason to say no —
    // a stable `wet_module_init` ABI, a dlopen path through the fence
    // for arbitrary C, and a decision about the `wet_get_config`
    // contract D9 deliberately ends at R3.
    //
    // "For now": the door is not nailed shut, and the key stays in the
    // config model so this message can be a real one instead of a
    // generic unknown-field error.
    if !settings.modules.is_empty() {
        return Some(fatal(
            "--modules / [core] modules is not supported by westonite (third-party \
             plugin loading is deliberately dropped); the shell is built in",
        ));
    }
    // R2b: outputs (mode/scale/transform/off, --no-outputs,
    // --refresh-rate) are ported; the attributes still DRM-bound stay
    // fail-loud so a request for them never silently degrades.
    for out in &settings.config.output {
        let name = out.name.as_deref().unwrap_or("<unnamed>");
        // clone-of is a DRM concept: C only resolves it in
        // drm_config_find_controlling_output_section.  On any other
        // backend the key would be silently inert, so say so.
        if out.clone_of.is_some() && !has_drm {
            return Some(fatal(&format!(
                "[[output]] '{name}': clone-of applies to the drm backend only"
            )));
        }
        // mirror-of (R2c-mirror): valid only on a remote head's section
        // — C's machinery only ever mirrors ONTO rdp/vnc/pipewire
        // outputs (the simple_head_enable deferral is keyed on those
        // backend types), and of them only vnc is ported, whose one
        // head is named "vnc".  On any other section the key would be
        // silently inert (C's lazy sections make it a no-op there;
        // fail-loud instead).
        if let Some(src) = &out.mirror_of {
            if !settings.backends.contains(&Backend::Vnc) {
                return Some(fatal(&format!(
                    "[[output]] '{name}': mirror-of requires a remote backend (vnc) in \
                     the loaded backends"
                )));
            }
            if name != "vnc" {
                return Some(fatal(&format!(
                    "[[output]] '{name}': mirror-of is supported on remote outputs only \
                     (the vnc head)"
                )));
            }
            if src == name {
                return Some(fatal(&format!(
                    "[[output]] '{name}': mirror-of must name a different output"
                )));
            }
        }
        // max-bpc IS ported (R2c-drm reads it in decide_drm, including
        // C's mode=current interaction) — but only the DRM configure
        // consumes it, so elsewhere it would be inert.  Same shape as
        // clone-of above.
        if out.max_bpc.is_some() && !has_drm {
            return Some(fatal(&format!(
                "[[output]] '{name}': max-bpc applies to the drm backend only"
            )));
        }
        // eotf-mode / colorimetry-mode / color-characteristics reach
        // libweston only through the DRM configure (main.c:2417-2423);
        // the windowed and remote ones read icc-profile and allow-hdcp
        // and nothing else.  On a non-DRM output they would be inert,
        // so say so rather than accept them.
        if !has_drm
            && (out.eotf_mode.is_some()
                || out.colorimetry_mode.is_some()
                || out.color_characteristics.is_some())
        {
            return Some(fatal(&format!(
                "[[output]] '{name}': eotf-mode, colorimetry-mode and \
                 color-characteristics apply to the drm backend only"
            )));
        }
        // C returns from wet_output_set_color_profile before even
        // reading icc_profile when the manager is absent, so an
        // icc-profile without color-management=true is silently
        // ignored there.  Refuse instead.
        if out.icc_profile.is_some() && !settings.color_management {
            return Some(fatal(&format!(
                "[[output]] '{name}': icc-profile requires [core] color-management = true"
            )));
        }
    }

    if let Some(shell) = &cli.shell {
        // Parity flag: the Rust frontend's shell is built in; only the
        // default spelling is accepted (D19).
        if shell != "desktop" && shell != "desktop-shell.so" {
            return Some(fatal(&format!(
                "unknown shell \"{shell}\": the Rust frontend ships only the built-in \
                 desktop shell"
            )));
        }
    }
    None
}

/// `--logger-scopes` / `--flight-rec-scopes`: C's comma list.
///
/// Takes a nested Option so the caller can keep the distinction C keeps
/// for the flight recorder: the flag absent (default scopes) versus the
/// flag given empty (recorder off).
fn split_scopes(v: Option<&str>) -> Vec<String> {
    v.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

/// C main.c's leftover-argv contract (`fatal: unhandled option: %s`),
/// reached through the applicability tables in `reject_unported`.
fn unhandled_option(flag: &str) -> ExitCode {
    fatal(&format!(
        "unhandled option: {flag} (not consumed by any loaded backend)"
    ))
}

/// Resolve `[libinput]` into the typed config the DRM backend's
/// `configure_device` hook applies per device.
///
/// Where C warns and carries on with the key silently inert — an
/// unknown `accel-profile` or `scroll-method`, a `scroll-button` name
/// libevdev does not know, an `accel-speed` outside -1..=1 — this is a
/// startup error instead: a setting that was asked for and does nothing
/// is exactly the failure mode the migration refuses to reproduce.
/// Capability gating is *not* validated here, because "this touchpad
/// cannot rotate" is a runtime fact about the device, not a mistake in
/// the config; those keys are skipped per device, as in C.
fn build_input_config(li: &westonite_config::Libinput) -> Result<weston::InputConfig, String> {
    let accel_profile = match &li.accel_profile {
        Some(p) => Some(weston::AccelProfile::parse(p).ok_or_else(|| {
            format!(
                "[libinput] '{p}' is not a valid accel-profile. Try one of: {}",
                weston::AccelProfile::NAMES.join(" ")
            )
        })?),
        None => None,
    };
    if let Some(s) = li.accel_speed
        && !(-1.0..=1.0).contains(&s)
    {
        return Err(format!(
            "[libinput] accel-speed {s} is out of range (-1.0 to 1.0)"
        ));
    }
    let scroll_method = match &li.scroll_method {
        Some(m) => Some(weston::ScrollMethod::parse(m).ok_or_else(|| {
            format!(
                "[libinput] '{m}' is not a valid scroll-method. Try one of: {}",
                weston::ScrollMethod::NAMES.join(" ")
            )
        })?),
        None => None,
    };
    let scroll_button = match &li.scroll_button {
        Some(b) => Some(weston::ScrollButton::parse(b).ok_or_else(|| {
            format!("[libinput] '{b}' is not an evdev button name (e.g. BTN_RIGHT)")
        })?),
        None => None,
    };
    // C reads scroll-button only under scroll-method=button and
    // discards it otherwise; say so rather than accepting it inert.
    if scroll_button.is_some() && scroll_method != Some(weston::ScrollMethod::Button) {
        return Err(
            "[libinput] scroll-button only applies with scroll-method = \"button\"".to_string(),
        );
    }
    Ok(weston::InputConfig {
        enable_tap: li.enable_tap,
        tap_and_drag: li.tap_and_drag,
        tap_and_drag_lock: li.tap_and_drag_lock,
        disable_while_typing: li.disable_while_typing,
        middle_button_emulation: li.middle_button_emulation,
        left_handed: li.left_handed,
        rotation: li.rotation,
        accel_profile,
        accel_speed: li.accel_speed,
        natural_scroll: li.natural_scroll,
        scroll_method,
        scroll_button,
    })
}

/// The colour slice of one `[[output]]`, with C's validation.
///
/// C `parse_color_characteristics` (main.c:1538) enforces a rule that
/// is easy to miss and easy to get wrong: the eleven characteristic
/// keys form five groups — primaries (six values), white (two), max
/// luminance, min luminance, maxFALL — and each group must be given
/// **entirely or not at all**.  Half a group is a config error, not a
/// partial application.  Ranges are checked too: 0..=1 for the
/// chromaticity coordinates, 0..=1e5 for the luminances.
fn build_color_setup(
    settings: &Settings,
    out: &westonite_config::Output,
    name: &str,
) -> Result<weston::ColorSetup, String> {
    let eotf_mode = match &out.eotf_mode {
        Some(m) => Some(weston::EotfMode::parse(m).ok_or_else(|| {
            format!(
                "[[output]] '{name}': '{m}' is not a valid EOTF mode. Try one of: {}",
                weston::EotfMode::NAMES.join(" ")
            )
        })?),
        None => None,
    };
    let colorimetry_mode = match &out.colorimetry_mode {
        Some(m) => Some(weston::ColorimetryMode::parse(m).ok_or_else(|| {
            format!(
                "[[output]] '{name}': '{m}' is not a valid colorimetry mode. Try one of: {}",
                weston::ColorimetryMode::NAMES.join(" ")
            )
        })?),
        None => None,
    };
    // C: a non-default eotf/colorimetry mode needs the colour manager.
    if eotf_mode.is_some_and(|m| m != weston::EotfMode::Sdr) && !settings.color_management {
        return Err(format!(
            "[[output]] '{name}': a non-SDR eotf-mode requires [core] color-management = true"
        ));
    }
    if colorimetry_mode.is_some_and(|m| m != weston::ColorimetryMode::Default)
        && !settings.color_management
    {
        return Err(format!(
            "[[output]] '{name}': a non-default colorimetry-mode requires \
             [core] color-management = true"
        ));
    }

    let characteristics = match &out.color_characteristics {
        Some(cc_name) => Some(resolve_characteristics(settings, cc_name, name)?),
        None => None,
    };

    Ok(weston::ColorSetup {
        icc_profile: out.icc_profile.clone(),
        eotf_mode,
        colorimetry_mode,
        characteristics,
        // C allow_content_protection default (main.c:1681).
        allow_hdcp: out.allow_hdcp.unwrap_or(true),
    })
}

fn resolve_characteristics(
    settings: &Settings,
    cc_name: &str,
    output: &str,
) -> Result<weston::ColorCharacteristics, String> {
    let cc = settings
        .config
        .color_characteristics
        .iter()
        .find(|c| c.name.as_deref() == Some(cc_name))
        .ok_or_else(|| {
            format!(
                "output {output}: no [[color-characteristics]] section with \
                 'name = \"{cc_name}\"' found"
            )
        })?;
    // C rejects ':' in the name: it is reserved for the profile
    // descriptions libweston builds from these.
    if cc_name.contains(':') {
        return Err(format!(
            "[[color-characteristics]] name={cc_name}: reserved name. \
             Do not use ':' character in the name."
        ));
    }

    let range = |v: Option<f64>, key: &str, lo: f64, hi: f64| -> Result<Option<f32>, String> {
        match v {
            // NaN shall not pass, as C puts it.
            Some(x) if x.is_nan() || x < lo || x > hi => Err(format!(
                "[[color-characteristics]] name={cc_name}: {key} value {x} is outside \
                 of the range {lo} - {hi}."
            )),
            Some(x) => Ok(Some(x as f32)),
            None => Ok(None),
        }
    };
    let prim = [
        ("red-x", cc.red_x),
        ("red-y", cc.red_y),
        ("green-x", cc.green_x),
        ("green-y", cc.green_y),
        ("blue-x", cc.blue_x),
        ("blue-y", cc.blue_y),
    ];
    let white = [("white-x", cc.white_x), ("white-y", cc.white_y)];

    // Each group entirely or not at all.
    let group = |keys: &[(&str, Option<f64>)], label: &str| -> Result<bool, String> {
        let set = keys.iter().filter(|(_, v)| v.is_some()).count();
        if set == 0 {
            return Ok(false);
        }
        if set != keys.len() {
            let missing: Vec<&str> = keys
                .iter()
                .filter(|(_, v)| v.is_none())
                .map(|(k, _)| *k)
                .collect();
            return Err(format!(
                "[[color-characteristics]] name={cc_name}: group {label} key {} is missing. \
                 You must set either none or all keys of a group.",
                missing.join(", ")
            ));
        }
        Ok(true)
    };
    let have_primaries = group(&prim, "primaries")?;
    let have_white = group(&white, "white")?;

    let mut out_cc = weston::ColorCharacteristics::default();
    if have_primaries {
        let v: Vec<f32> = prim
            .iter()
            .map(|(k, v)| range(*v, k, 0.0, 1.0).map(|x| x.unwrap_or(0.0)))
            .collect::<Result<_, _>>()?;
        out_cc.primaries = Some([(v[0], v[1]), (v[2], v[3]), (v[4], v[5])]);
    }
    if have_white {
        let x = range(cc.white_x, "white-x", 0.0, 1.0)?.unwrap_or(0.0);
        let y = range(cc.white_y, "white-y", 0.0, 1.0)?.unwrap_or(0.0);
        out_cc.white = Some((x, y));
    }
    out_cc.max_luminance = range(cc.max_luminance, "max-luminance", 0.0, 1e5)?;
    out_cc.min_luminance = range(cc.min_luminance, "min-luminance", 0.0, 1e5)?;
    out_cc.max_fall = range(cc.max_fall, "max-fall", 0.0, 1e5)?;
    Ok(out_cc)
}

/// Resolve the settings into the fence's [`weston::OutputPolicy`]
/// (R2b): every `[[output]]` section becomes a typed rule applied to
/// the head of that name, and CLI --width/--height/--scale/--transform
/// become the overriding layer.  Precedence per head is C's
/// (wet_configure_windowed_output_from_config): backend defaults →
/// name-matched section → CLI.
///
/// Three deliberate divergences from C (see also `reject_unported`,
/// which validates *all* sections for the same fail-loud reason):
///
///  * C resolves a section only when a head of that name shows up, so a
///    section that matches nothing is silently inert — including one
///    with a typo'd name or an unparseable `transform`.  We validate
///    every section at startup instead: a bad transform name is fatal
///    with C's `Invalid transform "…"` wording, and a section with no
///    `name` key (which could never match anything) is fatal too.
///  * C logs `Invalid mode for output %s. Using defaults.` when a
///    section exists but has *no* `mode` key at all
///    (`if (!mode || sscanf(…) < 2)`, main.c parse_simple_mode).  We
///    log it only for a `mode` that is present and unparseable —
///    warning about a section that merely sets `scale` is noise.
///  * With more than one backend, C loses the CLI geometry entirely:
///    every loader calls `wet_init_parsed_options`, which *replaces*
///    `compositor->parsed_options` with a freshly zeroed one (leaking
///    the previous), while `parse_options` has already removed
///    `--width`/`--height`/`--scale`/`--transform` from argv for the
///    first loader that listed them.  The configure callbacks run at
///    the heads flush, after every load, so they all read the *last*
///    loader's empty table — `--backends=headless,vnc --width=800`
///    sizes neither output in C.  We apply the CLI layer to every
///    backend instead (the C option's evident intent), so a
///    multi-backend run honours `--width` where C silently drops it.
fn build_output_policy(settings: &Settings) -> Result<weston::OutputPolicy, String> {
    let mut policy = weston::OutputPolicy::defaults(1024, 640);

    for out in &settings.config.output {
        let Some(name) = out.name.clone() else {
            return Err("[[output]] section without a name= key".to_string());
        };
        let mut rule = weston::OutputRule {
            name: name.clone(),
            off: out.off == Some(true),
            size: None,
            // Positivity is validated in westonite-config, next to the
            // --width/--height/--scale checks it shares a rule with.
            scale: out.scale,
            transform: None,
            resizeable: out.resizeable,
            gbm_format: out.gbm_format.clone(),
            mirror_of: out.mirror_of.clone(),
            // DRM-only keys; the DRM configure is the only reader.
            // `mode_string` keeps the raw text alongside the parsed
            // size because on DRM "preferred"/"current"/a modeline are
            // all valid and only the backend can resolve a modeline.
            mode_string: out.mode.clone(),
            max_bpc: out.max_bpc,
            content_type: out.content_type.clone(),
            seat: out.seat.clone(),
            force_on: out.force_on == Some(true),
            clone_of: out.clone_of.clone(),
            color: build_color_setup(settings, out, &name)?,
        };
        if let Some(mode) = &out.mode {
            if mode == "off" {
                rule.off = true;
            } else if let Some(size) = parse_mode(mode) {
                rule.size = Some(size);
            } else if !matches!(mode.as_str(), "preferred" | "current") {
                // On DRM these two, and anything else, are meaningful
                // (see mode_string above); on every other backend C
                // logs this and falls back to the defaults.
                weston::log::message(&format!("Invalid mode for output {name}. Using defaults."));
            }
        }
        if let Some(t) = &out.transform {
            rule.transform = Some(
                weston::OutputTransform::parse(t)
                    .ok_or_else(|| format!("Invalid transform \"{t}\" for output {name}"))?,
            );
        }
        policy.rules.push(rule);
    }

    policy.cli.width = settings.width;
    policy.cli.height = settings.height;
    policy.cli.scale = settings.scale;
    if let Some(t) = &settings.transform {
        policy.cli.transform = Some(
            weston::OutputTransform::parse(t)
                .ok_or_else(|| format!("Invalid transform \"{t}\""))?,
        );
    }
    Ok(policy)
}

/// "WxH" or "WxH@rate" (weston's simple-mode grammar; rate ignored by
/// the headless output).  Anything else — `preferred`, `current`, a
/// full modeline — is None, and the caller falls back to the defaults
/// with C's log line, exactly as C's `sscanf("%dx%d") != 2` does
/// (the trimmed tree's parse_simple_mode dropped upstream's `@%d`
/// framerate conversion; a trailing `@rate` still parses because
/// sscanf stops after its two conversions).
///
/// Stricter than that sscanf on three shapes it would wave through:
/// embedded spaces (`1024 x 640`), trailing junk (`1024x640junk`,
/// where sscanf stops happily after two conversions), and
/// non-positive dimensions (`-100x480`, `0x0` — C hands them straight
/// to output_set_size).  All are typos, and C silently running at
/// 1024x640 is what makes them expensive.
fn parse_mode(mode: &str) -> Option<(i32, i32)> {
    let core = mode.split('@').next().unwrap_or(mode);
    let (w, h) = core.split_once('x')?;
    let w: i32 = w.parse().ok()?;
    let h: i32 = h.parse().ok()?;
    if w > 0 && h > 0 { Some((w, h)) } else { None }
}

/// C execute_autolaunch / execute_command: X_OK precheck for the config
/// path (exact message pinned by test_children), WAYLAND_DISPLAY from
/// the bound socket, watch registration for the SIGCHLD handler.
fn start_autolaunch(
    cli: &Cli,
    settings: &Settings,
    compositor: &weston::Compositor,
) -> Option<ExitCode> {
    let Some(argv) = &settings.autolaunch else {
        return None;
    };
    let from_config = cli.autolaunch.is_empty();
    if from_config {
        // C: access(path, X_OK); the positional path goes straight to
        // exec (PATH lookup) like execvp does.
        let path = std::path::Path::new(&argv[0]);
        if !is_executable(path) {
            return Some(fatal(&format!(
                "Specified autolaunch path ({}) is not executable",
                argv[0]
            )));
        }
    }
    let Some(mut cmd) = westonite_spawn::Command::from_argv(argv) else {
        return Some(fatal("autolaunch command is empty"));
    };
    if let Some(socket) = compositor.socket_name() {
        cmd = cmd.env("WAYLAND_DISPLAY", socket);
    }
    // D12: no WESTON_CONFIG_FILE export — the TOML config is not
    // readable by any stock client, and we ship none that read it.
    match cmd.spawn() {
        Ok(child) => {
            let pid = match i32::try_from(child.id()) {
                Ok(p) => p,
                Err(_) => return Some(fatal("autolaunch pid out of range")),
            };
            compositor.set_autolaunch(pid, settings.autolaunch_watch);
            None
        }
        Err(e) => Some(fatal(&format!(
            "Failed to spawn the autolaunch process: {e}"
        ))),
    }
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        // access(X_OK) approximation: any execute bit (we run as one
        // uid; exactness beyond this doesn't change the exec outcome —
        // a wrong positive still fails at spawn with a logged error).
        Ok(m) => m.is_file() && m.mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_mode;

    #[test]
    fn simple_mode_grammar() {
        assert_eq!(parse_mode("1024x640"), Some((1024, 640)));
        assert_eq!(parse_mode("1920x1080@60"), Some((1920, 1080)));
        // Rate present but empty: C's sscanf takes the two conversions
        // it got and moves on, and so do we.
        assert_eq!(parse_mode("800x500@"), Some((800, 500)));

        // Fall back to the defaults (with C's log line) for everything
        // the windowed grammar does not cover.
        for bad in [
            "preferred",
            "current",
            "off",
            "1024",
            "1024x",
            "x640",
            "0x640",
            "1024x-1",
            // Stricter than C's sscanf on purpose — see parse_mode.
            "1024 x 640",
            "1024x640junk",
        ] {
            assert_eq!(parse_mode(bad), None, "{bad}");
        }
    }
}
```

### A.33 `crates/westonite-shell/Cargo.toml` — copy as-is

```toml
[package]
name = "westonite-shell"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "SAFE: the desktop-shell policy state machine (shell.c port), written against weston::ShellHost; never touches weston-sys, the config file, or the CLI."

[dependencies]
weston = { workspace = true }

[dev-dependencies]
weston = { workspace = true, features = ["test-ids"] }

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
# D13: no unwrap/expect on wrapper results — a panic kills the session.
unwrap_used = "deny"
expect_used = "deny"
```

### A.34 `crates/westonite-shell-plugin/Cargo.toml` — copy as-is

```toml
[package]
name = "westonite-shell-plugin"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "R1-ONLY packaging shim (deleted at R3, plan D2): the cdylib installed as desktop-shell.so. Contains just the wet_shell_init entry point (opaque pointers; never names weston-sys types, though it transitively links weston-sys via weston's hybrid-r1 — hence UNSAFE in the fence rules); logic lives in westonite-shell, the fence in weston."

[lib]
crate-type = ["cdylib"]

[dependencies]
weston = { workspace = true, features = ["hybrid-r1"] }
westonite-shell = { workspace = true }

[lints.rust]
unsafe_op_in_unsafe_fn = "deny"

[lints.clippy]
# Plan §6: unsafe hygiene lints apply to every unsafe crate.
undocumented_unsafe_blocks = "deny"
```

### A.35 `crates/westonite-config/Cargo.toml` — copy as-is

```toml
[package]
name = "westonite-config"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "SAFE: the re-specified configuration interface (plan §5, D9-D12): serde TOML model of the full option surface, clap CLI, -o dotted overrides, defaults→file→overrides→flags resolution into an immutable Settings."

[dependencies]
serde = { workspace = true }
toml = { workspace = true }
clap = { workspace = true }

[dev-dependencies]
tempfile = "3"

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
# D13: no unwrap/expect outside tests.
unwrap_used = "deny"
expect_used = "deny"
```

### A.36 `crates/westonite-shell-plugin/src/lib.rs` — copy as-is

```rust
//! The hybrid-phase `desktop-shell.so` (plan §2, D2 — R1 only, deleted
//! at R3).  The C frontend dlopens this and calls `wet_shell_init`;
//! everything else happens behind `weston::shell_init`.

use std::ffi::{c_char, c_int, c_void};

use westonite_shell::{Shell, ShellConfig};

/// The module entry point the westonite frontend loads
/// (frontend/weston.h contract).  Opaque pointers only — this crate
/// never names a weston-sys type.
///
/// D16 (no unwinding across the C boundary): `shell_init` runs its
/// body under `catch_unwind` and logs any panic through weston_log
/// before reporting failure — that guard lives in the `weston` crate
/// because the log path is not visible here.  The `catch_unwind` below
/// is the boundary's own backstop, turning anything that still unwinds
/// out of the call into a plain `-1` (module init failed) instead of
/// the edition-2024 abort shim firing mid-`dlopen`.
///
/// # Safety
/// Called by the C frontend exactly once per compositor with a live
/// `weston_compositor *`; argc/argv are unused (the R1 shell takes no
/// module arguments, like the trimmed C shell).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wet_shell_init(
    ec: *mut c_void,
    _argc: *mut c_int,
    _argv: *mut *mut c_char,
) -> c_int {
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        weston::shell_init::shell_init(ec, |background_color| {
            Box::new(Shell::new(ShellConfig { background_color }))
        })
    }))
    .unwrap_or(false);
    if ok { 0 } else { -1 }
}
```

### A.37 `crates/weston/examples/r0-smoke.rs` — copy as-is

```rust
//! R0 exit-criterion smoke binary (plan §7 R0): bring up compositor +
//! headless backend + noop renderer through the fence crate, run the
//! event loop, exit 0 on SIGTERM — mirroring the Phase-1 C smoke test.
//!
//! Throwaway by design: the real frontend (`westonite` crate) replaces
//! it at R2.  It deliberately uses only the safe public API.

use weston::{CompositorBuilder, Event, ShellApp, ShellHost};

struct Smoke;

impl ShellApp for Smoke {
    fn handle(&mut self, _ctx: &weston::Ctx, event: Event) {
        eprintln!("westonite-r0: event {event:?}");
    }
}

fn main() -> std::process::ExitCode {
    // A compositor needs a log context, exactly as in C -- and it has
    // to outlive the compositor, so it is created first and dropped
    // last (main.c order).  Default setup: stderr, the "log" scope, and
    // the default flight recorder.
    weston::log::install_stderr_handlers();
    let log = match weston::log::LogContext::new(&weston::log::LogSetup::default()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("westonite-r0: log setup failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let compositor = match CompositorBuilder::headless()
        .with_log_context(&log)
        .renderer(weston::RendererKind::Noop)
        .output_size(1024, 768)
        .with_socket()
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("westonite-r0: startup failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    compositor.ctx().set_app(Box::new(Smoke));

    // Prove the safe query surface works: enumerate outputs.
    let ctx = compositor.ctx().clone();
    for id in ctx.outputs() {
        match ctx.output_info(id) {
            Some(info) => eprintln!(
                "westonite-r0: output {:?} \"{}\" {}x{}",
                id, info.name, info.geometry.width, info.geometry.height
            ),
            None => eprintln!("westonite-r0: output {id:?} (stale)"),
        }
    }

    let mut compositor = compositor;
    let code = compositor.run();
    drop(compositor);
    eprintln!("westonite-r0: clean exit ({code})");
    std::process::ExitCode::from(code as u8)
}
```

### A.38 `crates/westonite-spawn/Cargo.toml` and `crates/westonite/Cargo.toml` — copy, then apply

Delta (R2a, §9.1 item 4): in `crates/westonite/Cargo.toml` the `weston` dependency is `weston = { workspace = true }` with no `features`, and its comment goes; the native shell attach is unconditional after the split.

```toml
[package]
name = "westonite-spawn"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "UNSAFE (one audited module): process spawning for the westonite frontend — exec-string env parsing, fd passing, and the pre_exec block (fd unCLOEXEC, setsid, sigmask reset) with only async-signal-safe calls between fork and exec (plan §2, risk R-D lives here and only here)."

[dependencies]
libc = { workspace = true }

[lints.rust]
unsafe_op_in_unsafe_fn = "deny"

[lints.clippy]
undocumented_unsafe_blocks = "deny"
```
```toml
[package]
name = "westonite"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "SAFE: the westonite frontend binary (main.c port, R2 — growing by slice; headless core at R2a). Links westonite-shell statically; reaches C only through the weston fence."

[[bin]]
name = "westonite-rs"
path = "src/main.rs"

[dependencies]
# `hybrid-r1` gates the whole shell_init module today (attach_shell_native
# included); the native binary needs it for with_shell.  The feature split
# (native vs dlsym hybrid extras) happens when the plugin is deleted at R3.
weston = { workspace = true, features = ["hybrid-r1"] }
westonite-shell = { workspace = true }
westonite-config = { workspace = true }
westonite-spawn = { workspace = true }
clap = { workspace = true }

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
```

### A.39 `crates/weston/src/layoutput.rs` — the normative header and the first types

```rust
//! Layout outputs: the DRM head→output grouping (plan §7 R2c).
//!
//! Every other backend takes the *simple* path — one head, one output,
//! configure, enable, done (`simple_heads_changed` in C,
//! `heads_changed` + `enable_head` here).  DRM does not, because on
//! DRM several connectors can be driven as clones of one logical
//! output, and because a mode set can *fail* at enable time in ways
//! only the kernel knows about.
//!
//! So C gives DRM its own handler and a small pile of bookkeeping —
//! `wet_layoutput`, `wet_output`, `wet_head_array`,
//! `drm_process_layoutput(s)`, `drm_try_attach`, `drm_try_enable` —
//! and this module is that pile.  The shape is worth stating once,
//! because it is not obvious from any single function:
//!
//! - A **layoutput** is a named group of heads that will be driven as
//!   clones.  Its name comes from the `[[output]]` section's `name=`
//!   (so two heads whose sections share a name land in one group), or
//!   from the head itself when no section matches.
//! - Heads arriving in a flush are *staged* on the layoutput's `add`
//!   list, not enabled immediately.  C does this because the whole
//!   point is to collect all the clones of one output before creating
//!   it — enabling the first one eagerly would foreclose the group.
//! - Enabling is then **attach-then-enable with undo**: attach the
//!   staged heads, try to enable, and on failure detach the heads one
//!   at a time and retry, until either it comes up or nothing is left.
//!   Heads that lost are pushed to the next round rather than dropped.
//!
//! That retry loop is the reason this cannot collapse into the simple
//! path: `weston_output_enable` failing is normal on real hardware
//! (not enough CRTCs, incompatible modes), and the frontend is
//! expected to degrade rather than abort.

use std::cell::Cell;
use std::ffi::CString;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ctx::Ctx;
use crate::listener::Listener;
use crate::log;
use crate::output_policy::{DrmMode, DrmOutputSetup, OutputPolicy};

/// C `MAX_CLONE_HEADS`.  C's guard is `if (n + 1 >= ARRAY_LENGTH)`,
/// which caps the list one below the array size; kept exactly, because
/// a group that silently grew past C's limit would diverge from the
/// oracle in the one case hardest to reproduce.
const MAX_CLONE_HEADS: usize = 16;

/// C `wet_output`: one weston_output belonging to a layoutput, plus the
/// destroy listener that nulls the pointer when the output dies.
///
/// The output can be destroyed behind our back — `drm_output_destroy()`
/// defers destruction in some paths — so the pointer is a `Cell` that
/// the listener clears, exactly as C nulls `output->output`.
pub(crate) struct LayoutputOutput {
    /// Shared with the destroy listener's closure — that is the whole
    /// point of the `Rc`: the listener must clear the same cell the
    /// readers look at, or a destroyed output stays live to us.
    output: Rc<Cell<*mut weston_sys::weston_output>>,
    /// Held so the listener lives as long as the entry.
    _destroy: Listener,
}

impl LayoutputOutput {
    fn get(&self) -> Option<NonNull<weston_sys::weston_output>> {
        NonNull::new(self.output.get())
    }
}

```

### A.40 `crates/weston/src/xwayland.rs` — the normative header

```rust
//! Xwayland frontend glue (R2d): the whole of C `frontend/xwayland.c`.
//!
//! The xwayland.so module owns the X sockets and calls back
//! ([`spawn_xserver`]) when the first X client connects; the frontend's
//! job is to fork the X server with the right fds (through
//! `westonite-spawn`, the audited fork/exec surface), watch the
//! `-displayfd` readiness pipe, and relay process exit to the module so
//! it can respawn lazily.
//!
//! Ownership map (§3 memory model):
//! - `abstract_fd`/`unix_fd` arrive **borrowed** from the module (C
//!   passes their numbers to the child and never closes them); we dup
//!   each into an `OwnedFd` for the child and leave the originals
//!   untouched.
//! - the wayland socketpair's parent end transfers to libwayland at
//!   `wl_client_create` (freed with the client);
//! - `wm_fd`'s parent end transfers to the module at `xserver_loaded`
//!   (the WM closes it);
//! - the displayfd pipe's read end stays ours (closed when the watch
//!   ends), the write end goes to the child.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, OsStr};
use std::os::fd::{AsRawFd, BorrowedFd, IntoRawFd, OwnedFd};
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::compositor::CompositorError;
use crate::ctx::Ctx;
use crate::log;

/// libwayland's WL_EVENT_READABLE (an anonymous enum in the header, so
/// bindgen gives it a positional module name).
const WL_EVENT_READABLE: u32 = weston_sys::_bindgen_ty_2::WL_EVENT_READABLE;
/// `_bindgen_ty_2` is *positional*: a header change that introduces an
/// anonymous enum ahead of libwayland's event mask would silently point
/// the constant above at an unrelated enum.  WL_EVENT_READABLE is 0x01
/// by the wayland-server ABI — pin it (risk R-C, same family as the
/// bindings version tripwire).  Spelled as a const `if`/`panic!` rather
/// than `assert!`, which clippy reads as an always-true assertion.
const _: () = {
    if WL_EVENT_READABLE != 0x01 {
        panic!("_bindgen_ty_2 is no longer libwayland's event mask — re-check the regen output");
    }
};

/// Frontend-side Xwayland state (C `struct wet_xwayland`), reached from
/// the spawn/displayfd/SIGCHLD callbacks via `ctx.inner.xwayland`.
/// Fields are individually interior-mutable; no borrow is held across
/// an FFI call (the ctx aliasing rule).
pub(crate) struct XwaylandState {
    pub(crate) api: NonNull<weston_sys::weston_xwayland_api>,
    pub(crate) xwayland: NonNull<weston_sys::weston_xwayland>,
    /// `[xwayland] path` (C XSERVER_PATH default, applied in resolve).
    xserver_path: PathBuf,
    /// The running X server, if any (C `wxw->process`): pid for the
    /// SIGCHLD match, path logged on exit.
```

### A.41 `crates/weston/src/desktop.rs` — the vtable

```rust
//! The `weston_desktop_api` vtable and desktop-surface lifecycle
//! (plan §3d; tier assignments + A3 proofs in docs/callback-inventory.md).
//!
//! All 10 installed entries are sync tier except ping_timeout/pong
//! (deferred).  Registration pairs the table insert with everything
//! that will invalidate it (§3b): the desktop surface slot is staled in
//! the `surface_removed` trampoline itself — the "no signal, API
//! callback announces death" row of the §3a table.

use std::ffi::{c_int, c_void};
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ctx::{Ctx, SurfaceRec};
use crate::events::Event;
use crate::host::ResizeEdges;
use crate::ids::DesktopSurfaceId;
use crate::panic_barrier;

static SHELL_DESKTOP_API: weston_sys::weston_desktop_api = weston_sys::weston_desktop_api {
    struct_size: std::mem::size_of::<weston_sys::weston_desktop_api>(),
    surface_added: Some(tramp_surface_added),
    surface_removed: Some(tramp_surface_removed),
    committed: Some(tramp_committed),
    move_: Some(tramp_move),
    resize: Some(tramp_resize),
    set_parent: Some(tramp_set_parent),
    ping_timeout: Some(tramp_ping_timeout),
    pong: Some(tramp_pong),
    set_xwayland_position: Some(tramp_set_xwayland_position),
    get_position: Some(tramp_get_position),
    // Not installed by the C shell (T-series trims): advertised
    // capabilities stay identical.
    show_window_menu: None,
    fullscreen_requested: None,
    maximized_requested: None,
    minimized_requested: None,
};

/// `weston_desktop_create` with our vtable (wet_shell_init tail).
```

### A.42 `crates/weston/src/grab.rs` — the pinned box shape

```rust
//! Grabs: move / resize / busy pointer grabs + the touch move grab
//! (plan §3f).  The C hazards this module owns: the allocation is
//! handed to libweston by address and freed from inside its own
//! callbacks at nine C sites; starting a grab can synchronously cancel
//! and free a different grab; the grabbed surface can die mid-grab.
//!
//! Design: grab-local state lives in the pinned box; every trampoline
//! is sync tier with **no app borrow** (A3) — it touches only the box,
//! the wrapper rec, and libweston.  Ending a grab moves the box to the
//! pending-drop list (freed at depth-zero drain, never inside the frame
//! C is dispatching on) and enqueues a deferred `GrabEnded` policy
//! event.  The back-reference is a `DesktopSurfaceId`: "surface died
//! mid-grab" is a `None` resolution, not a nulled pointer.

use std::cell::{Cell, UnsafeCell};
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::ctx::Ctx;
use crate::events::{ActivateVia, Event};
use crate::ids::{DesktopSurfaceId, SeatId};
use crate::panic_barrier;

const EDGE_TOP: u32 = 1;
const EDGE_BOTTOM: u32 = 2;
const EDGE_LEFT: u32 = 4;
const EDGE_RIGHT: u32 = 8;

// wl_fixed_* are static inlines, mirrored here.  `fixed_from` is
// wl_fixed_from_double's union trick: adding the 3 << 43 magic double
// leaves the rounded-to-nearest 24.8 value in the low mantissa bits (a
// plain `(d * 256.0) as i32` would truncate and diverge from C by one
// fixed-point unit).
fn fixed_from(d: f64) -> i32 {
    ((d + (3i64 << (51 - 8)) as f64).to_bits() as i64) as i32
}
fn fixed_to_int(f: i32) -> i32 {
    f / 256
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerGrabKind {
    Move,
    Resize,
    Busy,
}

/// `raw` MUST remain first (§3f: offset math in exactly one place —
/// the trampolines' cast).  `UnsafeCell`: C writes `grab->pointer`
/// through the handed-out pointer.  The offset-0 cast is valid
/// because this struct is `repr(C)` with `raw` first; `UnsafeCell`
/// being `repr(transparent)` only keeps the field's own layout
/// identical to the C struct (PR16-S3).
#[repr(C)]
pub(crate) struct PointerGrabInner {
    raw: UnsafeCell<weston_sys::weston_pointer_grab>,
    kind: PointerGrabKind,
    surface: DesktopSurfaceId,
    // move state
    delta: Cell<(f64, f64)>,
    // resize state
    edges: u32,
    start_w: i32,
    start_h: i32,
    ended: Cell<bool>,
    _pin: PhantomPinned,
}

#[repr(C)]
```

### A.43 `crates/westonite-shell/src/lib.rs` — the state and the event dispatcher head

```rust
//! `westonite-shell`: the desktop-shell policy state machine — the safe
//! port of `desktop-shell/shell.c` (plan §4, D4/D20).
//!
//! Plain structs, plain maps, `Copy` ids, `&mut self` — no `RefCell`,
//! no `Rc`, no lifetimes (plan goal 2).  Every wrapper result is an
//! `Option` handled with `let … else` skips (D13); `unwrap`/`expect`
//! are compile errors here.  All host interaction goes through
//! [`weston::ShellHost`], so the whole state machine unit-tests against
//! the mock in `tests/`.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use weston::{
    ActivateFlags, ActivateTarget, ActivateVia, DesktopSurfaceId, Event, OutputId, ResizeEdges,
    SeatId, ShellHost, SurfaceId,
};

#[derive(Debug, Clone, Copy)]
pub struct ShellConfig {
    /// `[shell] background-color` (C default 0xff002244).
    pub background_color: u32,
}

impl Default for ShellConfig {
    fn default() -> Self {
        ShellConfig {
            background_color: 0xff002244,
        }
    }
}

#[derive(Debug, Default)]
struct SurfState {
    parent: Option<DesktopSurfaceId>,
    /// Stacking-order children (C children_list, tail = most recent).
    children: Vec<DesktopSurfaceId>,
    focus_count: i32,
    xwayland: Option<(f64, f64)>,
    last_width: i32,
    last_height: i32,
}

#[derive(Debug, Default)]
struct SeatState {
    /// The main desktop surface last activated on this seat
    /// (C shell_seat.focused_surface).
    focused_surface: Option<DesktopSurfaceId>,
}

pub struct Shell {
    config: ShellConfig,
    surfaces: HashMap<DesktopSurfaceId, SurfState>,
    seats: HashMap<SeatId, SeatState>,
    /// Per-seat keyboard-focus tracking (C focus_state of the single
    /// workspace): the actual focused weston_surface, watched by the
    /// wrapper until untracked.
    focus: HashMap<SeatId, SurfaceId>,
    /// Deterministic placement rng (C uses unseeded random(), which is
    /// deterministic too; bit-parity is not required — §10 cosmetic).
    rng: u64,
}

impl Shell {
    pub fn new(config: ShellConfig) -> Shell {
        Shell {
            config,
            surfaces: HashMap::new(),
            seats: HashMap::new(),
            focus: HashMap::new(),
            rng: 0x853c49e6748fea9b,
        }
    }

    fn next_rand(&mut self, modulo: i32) -> i32 {
        // xorshift64*; only used for initial window placement.
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        if modulo <= 0 {
            0
        } else {
            ((x.wrapping_mul(0x2545F4914F6CDD1D) >> 33) % modulo as u64) as i32
        }
    }

    /// The public entry point; `weston`'s dispatch calls this through
    /// [`weston::ShellApp`].
    pub fn on_event(&mut self, host: &dyn ShellHost, ev: Event) {
        match ev {
            Event::SurfaceAdded { surface } => {
                self.surfaces.insert(surface, SurfState::default());
            }
            Event::SurfaceRemoved { surface } => self.surface_removed(host, surface),
            Event::Committed {
                surface,
                buf_dx,
                buf_dy,
                resize_edges,
                width,
                height,
                mapped,
            } => self.committed(
                host,
                surface,
                buf_dx,
                buf_dy,
                resize_edges,
                width,
                height,
                mapped,
            ),
            Event::ParentSet { surface, parent } => self.set_parent(surface, parent),
            Event::XwaylandPosition { surface, x, y } => {
                if let Some(st) = self.surfaces.get_mut(&surface) {
                    st.xwayland = Some((x, y));
                }
```

### A.44 `crates/westonite-shell/src/tests.rs` — the mock host

```rust
//! Mock-`ShellHost` unit tests (D20): the focus-churn and
//! teardown-ordering cases §3b flags as the risky ones, plus child
//! activation — all without a compositor.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use weston::ids::forge;
use weston::{
    ActivateFlags, ActivateTarget, ActivateVia, DesktopSurfaceId, Event, OutputId, OutputInfo,
    Rect, ResizeEdges, SeatId, ShellHost, SurfaceId,
};

use crate::{Shell, ShellConfig};

#[derive(Default)]
struct MockHost {
    outputs: Vec<(OutputId, Rect)>,
    seats: Vec<SeatId>,
    mapped: RefCell<HashSet<DesktopSurfaceId>>,
    view_pos: RefCell<HashMap<DesktopSurfaceId, (f64, f64)>>,
    // command journal the tests assert on
    activated: RefCell<Vec<(DesktopSurfaceId, bool)>>,
    input_activations: RefCell<Vec<(DesktopSurfaceId, SeatId)>>,
    raised: RefCell<Vec<DesktopSurfaceId>>,
    destroyed: RefCell<Vec<DesktopSurfaceId>>,
    untracked: RefCell<Vec<SurfaceId>>,
    tracked_for: RefCell<HashMap<DesktopSurfaceId, SurfaceId>>,
    /// Emulates the wrapper's ref-count contract: activate_input
    /// tracks (+1), untrack_surface drops (-1); the destroy listener
    /// lives while the count is positive.
    track_counts: RefCell<HashMap<SurfaceId, i32>>,
    stacking: RefCell<Vec<DesktopSurfaceId>>, // top first
}

impl MockHost {
    fn track_count(&self, s: SurfaceId) -> i32 {
        self.track_counts.borrow().get(&s).copied().unwrap_or(0)
    }
}

impl ShellHost for MockHost {
    fn outputs(&self) -> Vec<OutputId> {
        self.outputs.iter().map(|(o, _)| *o).collect()
    }
    fn output_info(&self, id: OutputId) -> Option<OutputInfo> {
        self.outputs
            .iter()
            .find(|(o, _)| *o == id)
            .map(|(_, g)| OutputInfo {
                name: "mock".into(),
                geometry: *g,
            })
    }
    fn seats(&self) -> Vec<SeatId> {
        self.seats.clone()
    }
    fn pointer_pos(&self, _seat: SeatId) -> Option<(f64, f64)> {
        None
    }
    fn pointer_focus(&self, _seat: SeatId) -> Option<DesktopSurfaceId> {
        None
    }
    fn workspace_views_top_down(&self) -> Vec<DesktopSurfaceId> {
        self.stacking.borrow().clone()
    }
    fn is_view_mapped(&self, id: DesktopSurfaceId) -> bool {
        self.mapped.borrow().contains(&id)
    }
    fn view_pos(&self, id: DesktopSurfaceId) -> Option<(f64, f64)> {
        self.view_pos.borrow().get(&id).copied()
    }
    fn view_output(&self, _id: DesktopSurfaceId) -> Option<OutputId> {
        None
    }
    fn surface_geometry(&self, _id: DesktopSurfaceId) -> Option<Rect> {
        Some(Rect::default())
    }
    fn activate_input(
        &self,
        target: ActivateTarget,
        seat: SeatId,
        _flags: ActivateFlags,
    ) -> Option<SurfaceId> {
        let ActivateTarget::SurfaceView(id) = target else {
            return None;
        };
        self.input_activations.borrow_mut().push((id, seat));
        let es = self.tracked_for.borrow().get(&id).copied();
        if let Some(es) = es {
            *self.track_counts.borrow_mut().entry(es).or_insert(0) += 1;
        }
        es
    }
    fn untrack_surface(&self, id: SurfaceId) {
        self.untracked.borrow_mut().push(id);
        *self.track_counts.borrow_mut().entry(id).or_insert(0) -= 1;
    }
    fn set_activated(&self, id: DesktopSurfaceId, active: bool) {
        self.activated.borrow_mut().push((id, active));
    }
    fn raise_to_workspace_top(&self, id: DesktopSurfaceId) {
        self.raised.borrow_mut().push(id);
        let mut s = self.stacking.borrow_mut();
        s.retain(|v| *v != id);
        s.insert(0, id);
    }
    fn map_surface(&self, id: DesktopSurfaceId) {
        self.mapped.borrow_mut().insert(id);
    }
    fn set_view_position(&self, id: DesktopSurfaceId, x: f64, y: f64) {
        self.view_pos.borrow_mut().insert(id, (x, y));
    }
    fn set_view_position_with_offset(
        &self,
        id: DesktopSurfaceId,
        x: f64,
        y: f64,
        ox: f64,
        oy: f64,
    ) {
        self.view_pos.borrow_mut().insert(id, (x + ox, y + oy));
    }
    fn update_transforms(&self, _id: DesktopSurfaceId) {}
    fn mark_view_dirty(&self, _id: DesktopSurfaceId) {}
    fn destroy_surface_state(&self, id: DesktopSurfaceId) {
        self.destroyed.borrow_mut().push(id);
        self.mapped.borrow_mut().remove(&id);
        self.stacking.borrow_mut().retain(|v| *v != id);
    }
    fn set_busy_cursor(&self, _id: DesktopSurfaceId, _seat: SeatId) {}
    fn end_busy_grabs_for_client_of(&self, _id: DesktopSurfaceId) {}
    fn recreate_background(&self, _output: OutputId, _argb: u32) {}
}

fn commit(shell: &mut Shell, host: &MockHost, s: DesktopSurfaceId, mapped: bool) {
    shell.on_event(
        host,
        Event::Committed {
            surface: s,
            buf_dx: 0.0,
            buf_dy: 0.0,
            resize_edges: ResizeEdges::empty(),
            width: 100,
            height: 100,
            mapped,
        },
    );
```

---

## Appendix B — pitfalls index

| # | Where | Pitfall |
|---|---|---|
| 1 | R0 | `weston_compositor_backends_loaded` is mandatory before the heads flush: it installs the no-op colour manager that `weston_output_init` dereferences. Missing it is a startup segfault. |
| 2 | R0 | `weston_output_lazy_align` is a frontend-local static in `main.c`, not libweston API; implement it, and truncate the running position through `int` as C does. |
| 3 | R0 | The installed `windowed-output-api.h` needs `ARRAY_LENGTH` defined on the bindgen command line or it does not parse. |
| 4 | R0 | With `--as-needed` a library nothing references is dropped from `DT_NEEDED`: `weston-sys` builds clean against libinput/libevdev while the binary links neither. Keep the address-taking unit test. |
| 5 | R0 | `bindgen` names anonymous enums positionally (`_bindgen_ty_2` is libwayland's event mask). Pin `WL_EVENT_READABLE == 0x01` with a `const` check next to every such use. |
| 6 | R0 | Never wrap `wl_display_run` in the depth counter; if you do, every deferred event and every retired box waits until the loop exits (focus after close breaks; pending-drop grows per window drag). |
| 7 | R0 | Event-loop signal callbacks are outside every trampoline: wrap their whole body once, or `on_sigchld`'s outbound calls drain between two `waitpid` iterations. |
| 8 | R0 | Install the signal sources **before** any backend loads; the VNC backend spawns worker threads that otherwise never block SIGTERM, and a process-directed SIGTERM can kill the process before the signalfd sees it. Block SIGUSR1 there too. |
| 9 | R0 | SIGINT goes through plain `sigaction` re-raising SIGUSR2, not through the loop, so gdb still catches Ctrl+C; log `caught signal N` before terminating. |
| 10 | R0 | `weston_compositor_set_xkb_rule_names` stores and later frees the strings: pass C-owned `strdup` copies, and free them yourself only on the failure path. |
| 11 | R0 | `multi_backend` must be set before loading backends: libweston's damage tracking drops the core buffer reference early when it is false, and a second backend then reads freed buffers. |
| 12 | R0 | Pass the renderer choice through unresolved (AUTO stays AUTO); resolving it in the frontend hands VNC `NOOP` under `--backends=vnc,headless`. |
| 13 | R0 | Bind the socket **after** the heads flush and the `init_failed` check; the harness treats the socket's existence as "outputs configured". |
| 14 | R0 | `cargo build --examples` selects only example targets; list `--bins` too or the frontend binary the later smoke legs run never builds. |
| 15 | R0 | weston opens `--log` in append mode: delete the log file before every smoke leg or a stale file satisfies the marker greps. |
| 16 | R0 | `grep -q` exits early and can SIGPIPE the producer under `pipefail` (the rustup check in the ASAN script); use `grep … >/dev/null`. |
| 17 | R0 | Listeners on signals embedded in the `weston_compositor` struct must be `Compositor`-owned and detached before `weston_compositor_destroy`; owned by `Ctx` they are detached after the struct is freed (valgrind invalid write). |
| 18 | R0/R2a | `weston_compositor_add_screenshot_authority` writes `listener->notify` and would overwrite the trampoline: attach by hand with `wsys_wl_signal_add` on `output_capture.ask_auth`, then `mark_attached()`. A hand-attached listener without `mark_attached()` is freed while still linked. |
| 19 | R1 | The compositor-destroy trampoline tears the wrapper down while its own listener frame is on the stack: park boxes in the graveyard, never drop them there. Active grab boxes too — the backends cancel grabs through the embedded vtable **after** the destroy emission. |
| 20 | R1 | `weston_seat_release` frees the pointer before it emits the seat destroy signal: detach the pointer-focus listener from a guard on `weston_pointer`'s own destroy signal. |
| 21 | R1 | The hybrid shell must never call `weston_log_set_handler`: it hijacks the C frontend's `--log` file. |
| 22 | R1 | The compositor user-data slot belongs to the C frontend during the hybrid; root all wrapper state in `Ctx`. |
| 23 | R1 | The shell's background curtain must be created **inside** the `output_created` emission (sync `OutputCreated`); deferring it leaves the Rust frontend with no background at all. |
| 24 | R1 | A sync-tier dispatch that finds the app borrow held outside a drain is a failed non-reentrancy proof: assert in debug, log and queue in release. Never silently defer. |
| 25 | R1 | `wl_fixed_from_double` is the union trick, not `(d * 256.0) as i32`; the truncating form is off by one fixed-point unit from C. |
| 26 | R2a | `-o` values are parsed as TOML before deserialization, so `0xff002244` arrives as an integer: colour and numeric-string keys must accept numbers. Render colours back as `0x%08x`, never decimal, because the grammar is always base-16. |
| 27 | R2a | `$XDG_CONFIG_HOME` and `$HOME/.config` are searched in that order, not either/or; an empty `XDG_CONFIG_HOME` must not mask the home config. |
| 28 | R2a | `--backend`/`--backends` and `[core] backend`/`backends` are one variable; a comma list in any of them loads several backends. |
| 29 | R2a | `verify_xdg_runtime_dir` checks mode **and owner**; do not mask the mode with `0o7777` (sticky/setgid dirs are fine in C). |
| 30 | R2a | The SIGCHLD source must exist before the autolaunch spawn, or a watched client that exits immediately is never noticed. |
| 31 | R2a | Log through the `"log"` scope from the first frontend slice; writing the file directly produces untimestamped lines and no flight recorder. Destroy the scope explicitly before the context or libweston warns and leaks it. |
| 32 | R2b | Register an output only after `weston_output_enable` succeeds; a failed configure/enable sets `init_failed` and `build()` fails after the flush. |
| 33 | R2c | Headless publishes no native mode: a headless mirror source aborts the C oracle; fall back to the source's current mode and mark the test `toml_only`. |
| 34 | R2c | x11 numbers default heads after the named count (`X-1` + `screen1`); wayland numbers them from zero (`WL-1` + `wayland0`). One shared helper flattens this unless parameterised. |
| 35 | R2c | The `[libinput]` hook has no user-data pointer: populate `CtxInner::input_config` before `weston_compositor_load_backend`, because the hook fires for present devices during the load. |
| 36 | R2c | The upstream VNC backend leaks its xkb rule-name strdups and its format array at destroy: keep `valgrind-upstream-vnc.supp` narrow to `vnc-backend.so` frames and know that it also hides any new upstream leak there. |
| 37 | R2c | DRM non-desktop heads: no section → staged; section without `mode` → skipped; C's letter, not its comment. vkms never reports non-desktop, so only the unit matrix pins it. |
| 38 | R2c | `clone-of` sections contribute nothing of their own; resolve the controlling section recursively with C's depth-10 bound and its two error strings. |
| 39 | R2d | `abstract_fd`/`unix_fd` are borrowed from `xwayland.so`: dup for the child, never close. `spawn_xserver` must remove a stale displayfd watch (a respawn can outrun its own EOF in the same dispatch batch). |
| 40 | R2d | `Xwayland` needs `/tmp/.X11-unix` to exist (mode 1777) or the RPM's module segfaults in its bind-error path. |
| 41 | R2e | With `--no-outputs` C's recorder fallback is a wild `container_of` on the empty list head; log `no output to record` and return. |
| 42 | Always | `deny_unknown_fields` plus the harness's ini→TOML translation means every ini section spelling the tests use must map to a model spelling (`[color_characteristics]` → `[[color-characteristics]]`; C's `touchscreen_calibrator`/`calibration_helper` via serde aliases). |
| 43 | Always | Bindings drift and tool drift are indistinguishable unless bindgen is pinned; the regen script asserts the version and CI never skips the drift check. |
| 44 | Always | `cargo metadata`'s default-feature resolve is what the fence walk sees until R4 adds `--all-features`; a feature-gated path to `weston-sys` from a safe crate passes fence 1 until then. Review for it. |

## Appendix C — what could not be re-verified from the record

1. **Test counts are computed, not measured.** The 26 `toml_only` tests
   were counted from the inventory in §12 against the C plan's 85; the
   C plan's own count is itself reconstructed. Treat 111/110/1 as the
   expected shape and record the real numbers in `PROVENANCE.md` at R2d.
2. **The verbatim files come from a tree in which the C frontend still
   shipped the RDP loader, the remoting and pipewire-output plugins,
   `modules=`, the touch calibrator, Super+S and `--idle-time`.** The
   deltas listed in Appendix A (A.3, A.25, A.26, A.28–A.30, A.32) are
   this plan's reconciliation with the C plan's F1–F6; they are
   prescribed, not copied from a working tree, and the refusal wordings
   in §5.4 are the contract.
3. **`compositor.rs`, `grab.rs`, `desktop.rs`, `layoutput.rs`,
   `xwayland.rs`, `screenshooter.rs`, `libinput.rs`, `debug.rs`,
   `curtain.rs`, `layer.rs`, `input_bindings.rs`, `shell_init.rs`,
   `output_policy.rs`, `westonite-spawn/src/lib.rs` and the bodies of
   `westonite-shell/src/lib.rs` and `tests.rs` are not reproduced in
   full.** The plan fixes their contracts (§4, §8–§9, the callback
   inventory, the reproduced headers) and the tests they must pass; the
   agent writes the bodies against the C sources.
4. **The R3 spec (A.21) has not been built.** Its `%changelog` dates and
   packager are placeholders; verify that `cargo build --offline` with
   the vendored `pkg-config`/`cc` build-dependencies resolves inside
   `rpmbuild` (the vendor tarball must be produced from the same
   `Cargo.lock`).
5. **Toolchain versions**: the build image's rust-toolset was 1.96 when
   the workspace was built; edition 2024 needs at least 1.85. The
   nightly used by the ASAN leg is unpinned (R4 pins it).
6. **The pointer-destroy guard (§4.13) and the ref-counted focus
   tracking are described, not reproduced**; their behaviour is pinned
   by `multi_seat_shared_focus_keeps_other_seats_tracking` and the
   valgrind-clean destroy storms.
7. **Real-hardware DRM validation** remains an open item, as in the C
   plan: vkms proves the layoutput path and `[libinput]`, not mode
   setting on a real CRTC, non-desktop heads, or hotplug.
8. **The `test_pipewire_backend_publishes_an_output` daemon route** and
   the pipewire virtual-output probe results are inherited from the C
   plan's record without re-running.
9. **`--wait-for-debugger` and `--debug` wordings**, the `proto` dump
   line format, and the `libinput:` log format are copied from the
   tests that pin them; if the C oracle's wording differs in the tree
   you start from, the oracle wins and the Rust wording follows it.
