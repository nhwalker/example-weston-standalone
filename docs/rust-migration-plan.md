# Plan: Rust migration

Status: **final — ready to execute at R0**. All scoping decisions
(D1–D12) and all memory-model decisions (D13–D21, taken at the
2026-07-24/25 reviews) are folded into the body below; §11 is the
decision log. The former standalone review document
(`rust-migration-memory-model-review.md`) is superseded by this file
and has been deleted; every claim it made that this plan relies on was
re-verified against the C sources in this repo before finalization,
and the corrections that verification produced are folded in (noted
inline as *amendments* A1–A5 where they changed a decided item).

Translates the C sources in this repo (frontend + desktop-shell +
shared, ~9.7k lines) to Rust. libweston 14 stays the compositor
engine, consumed from the EPEL 10 RPMs exactly as today; libwayland
stays the protocol/event-loop runtime underneath it. Nothing about the
runtime split changes — the RPM still owns rendering, input, backends,
and the xwayland module; we still own picking and configuring all of
it. Only the language of *our* layer changes.

Goals, in priority order:

1. **All `unsafe` *and all unsoundness* live behind one fence.**
   `weston`'s public API must be sound for arbitrary safe callers —
   including callers that misuse it, store handles indefinitely, or
   hold handles to objects that have since died. It must not be
   possible to cause undefined behavior from a
   `#![forbid(unsafe_code)]` crate. (A fence around the `unsafe`
   *keyword* alone is not a fence around unsoundness: if the wrapper
   dereferences a possibly-dangling handle a safe crate handed it,
   every memory-safety bug is still authorable from "safe" code. The
   borrow checker prevents use-after-free only for memory Rust owns;
   these objects are owned and freed by C on its own schedule, so the
   liveness check is built once, by hand, inside the wrapper — §3.)
2. **The safe crates get the most straightforward memory model
   available**: plain structs, plain maps, `Copy` ids, `&mut self` —
   no `RefCell`, no `Rc`, no lifetimes, no interior mutability, no
   `unwrap`/`expect` on wrapper results. The shell and frontend are
   the most-edited code in the system; they must stay simple enough
   that a routine change cannot create a memory-model question.
3. **Idiomatic Rust**, not transliterated C: enums over int flags,
   `Option`/`Result` over sentinel returns, RAII over destroy
   listeners where we own the object, snapshot iterators over
   intrusive-list macros.
4. **Behavioral parity** with the current C build — *except the
   configuration interface, which is deliberately re-specified*
   (§5). Parity means: the same set of configurable capabilities for
   libweston and the shell, same runtime behavior and exit codes,
   same installed file layout, same RPM upgrade path — and the smoke
   scripts plus the **black-box e2e suite** (`tests/e2e/`,
   `docs/e2e-test-plan.md`) pass, with only the config-interface
   tests re-specified alongside §5. The e2e suite drives the
   compositor exactly as a user would (VNC input injection +
   framebuffer capture, no test hooks in shipped code), so it runs
   identically against the C build, every hybrid phase, and the
   final Rust build — it is the migration's fixed parity oracle.
   The known intentional divergences are collected in §10.

Non-goals: porting libweston itself; changing the libweston version
(stays 14.0.1/EPEL); adding features during the port (the
maintenance-layer plan is deferred separately — D7); porting the
e2e suite itself (the Python harness and the C `wtest-client`/
`wtest-xclient` test drivers stay as they are — a measuring stick
must not move while the thing it measures is rebuilt). One
carve-out to the frozen-suite rule: the config-interface tests
(config discovery in `test_cli.py`, ini-driven fixtures) are
**re-specified deliberately** when §5 lands — updated to the new
interface as a spec change, never silently loosened; all other
tests stay frozen.

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
- **The shell↔frontend contract is two functions.** `shell.c` calls
  exactly two symbols from `libexec_westonite.so`:
  `wet_get_config()` (`shell.c:328`) and `screenshooter_create()`
  (`shell.c:2232`, declared in `frontend/weston.h:37`). Everything
  else it uses is installed libweston API (`desktop.h`,
  `shell-utils.h`, `config-parser.h`). *(Correction: an earlier
  draft said "one function"; the screenshooter call was missed. It
  matters only for R1 linkage and R3 ownership — §7.)*
- **The config interface is being re-specified anyway** (§5, D9),
  so the C `weston_config_*`/ini machinery does not need to be
  ported or bound at all in the end state — a small temporary
  binding survives only through the hybrid phase R1 (the C frontend
  still parses `westonite.ini` then; the Rust shell reads its one
  key through it).

Current C surface to port (lines at trim T9):

| Component | Files | Lines | Character |
|---|---|---|---|
| frontend | `main.c`, `config-helpers.c`, `weston-screenshooter.c`, `xwayland.c`, `executable.c` | ~5.4k | wide but shallow: config → libweston-API plumbing |
| desktop-shell | `shell.c`, `shell.h` | ~2.3k | deep: callback-driven state machine (focus, layers, xdg lifecycle) |
| shared | `option-parser.c`, `os-compatibility.c`, `process-util.c` + 7 headers | ~1.9k | support code, mostly replaceable by std/rustix |

FFI-idiom counts in the current tree (the audit surface for §3; each
figure was re-counted at finalization): **34** `container_of` sites
(`shell.c` 21, `main.c` 9, `weston-screenshooter.c` 4), **29**
`wl_listener` fields (one a `main()` stack local), **23**
signal/destroy-listener attach sites, **5** binding registrations
with our own callbacks (plus `weston_install_debug_key_binding`,
whose handler is libweston's — 6 registrations total),
**14** `weston_desktop_api` entries.

## 2. Crate architecture

Cargo workspace, replacing the meson tree at the end of the
migration (hybrid during it — see §7). Six long-lived crates: three
may contain `unsafe`, three are unconditionally
`#![forbid(unsafe_code)]` with zero exceptions — plus one R1-only
packaging shim (in the tree below) that is deleted at R3.

```
Cargo.toml                    # workspace
crates/
├── weston-sys/               # UNSAFE. bindgen over the installed RPM headers:
│                             #   libweston.h, desktop.h, shell-utils.h, weston-log.h,
│                             #   windowed-output-api.h, backend-*.h, xwayland-api.h
│                             #   (+ the wayland-server types they embed)
│                             #   (the R1 config-parser/frontend compat is NOT bound here:
│                             #   it lives as hand-declared externs + dlsym in
│                             #   weston::shell_init, deleted at R3)
│                             # links: libweston-14, wayland-server
│                             # + a small `cc`-compiled shim: the static-inline helpers
│                             #   the port actually uses AND the va_list interfaces (§3k)
├── weston/                   # UNSAFE INSIDE, SAFE API. The fence. Hand-written safe
│                             # wrappers: Ctx (registry + dispatch, §3b/§3e), ShellHost
│                             # (§3e), Listener (§3c), Grab (§3f), DesktopApi vtable +
│                             # xwayland/backend-config builders (§3d), Curtain, Layer,
│                             # LogScope, bindings registration, module loading
├── westonite-spawn/          # UNSAFE (one audited module): process spawning —
│                             # CustomEnv (incl. the `ENV=x cmd arg` exec-string
│                             # parser), Fdstr, Command + pre_exec (fd unCLOEXEC,
│                             # setsid, sigmask reset); only async-signal-safe rustix
│                             # calls between fork and exec (risk R-D lives here,
│                             # and only here)
├── westonite-config/         # SAFE. The §5 config interface: serde Config model (TOML),
│                             # clap CLI, -o dotted overrides, defaults→file→CLI resolve
├── westonite-shell/          # SAFE. desktop-shell logic (shell.c port); written against
│                             # the ShellHost trait, receives a typed ShellConfig, never
│                             # touches the config file, the CLI, or `weston-sys`
├── westonite-shell-plugin/   # R1 ONLY, deleted at R3 (D2). Packaging shim that
│                             # produces the hybrid-phase desktop-shell.so:
│                             # crate-type = cdylib, depends on weston + westonite-shell,
│                             # and contains just the `#[unsafe(no_mangle)] extern "C"
│                             # wet_shell_init` entry point (opaque raw pointers in its
│                             # signature — no weston-sys dependency, no other unsafe),
│                             # which hands the compositor pointer to weston's hybrid-r1
│                             # bootstrap. cargo emits libwestonite_shell_plugin.so; the
│                             # build step installs it as desktop-shell.so.
└── westonite/                # SAFE. The frontend binary (main.c port); links
                              # westonite-shell statically at R3 (D2); absorbs the
                              # residual safe helpers of shared/ (socketpair via rustix)
```

*(Amendment A1 to D15: the earlier `westonite-shared` crate is
dissolved rather than kept as a fourth safe crate. Its contents were
process/env/fd-string helpers — which belong with the spawn audit in
`westonite-spawn` — and one socketpair util, which is a single safe
rustix call inlined at its two call sites. Fewer crates, and the
spawn crate's API surface is exactly its audit surface.)*

### Fence rules (D17 — mechanically enforced in CI except where noted)

- **Dependency-graph rule**: the three safe crates must not depend on
  `weston-sys` at all — they reach C only through `weston` and
  `westonite-spawn`. Checked with `cargo metadata` in CI. This, not a
  grep, is what actually prevents "safe" code from holding raw
  pointers.
- **Public-API rule**: `weston`'s and `westonite-spawn`'s public APIs
  contain no raw pointers, no `NonNull`, and no `weston_sys::*`
  types. **Currently review-enforced only**: the planned `cargo
  public-api` snapshot test (which would also give a reviewable diff
  every time the fence moves) is not yet wired into CI — a known gap,
  to be closed by R2. (Single R1 exception: the `hybrid-r1` bootstrap
  entry consumed by `westonite-shell-plugin` takes the raw compositor
  pointer as `*mut c_void`; it and the feature disappear at R3.)
- **Opaque ids**: handle types (§3b) have private fields, no public
  constructor, and no `From`/`Into` conversions to or from pointers
  or integers. Even with `weston` in scope, safe code cannot
  fabricate a handle or unwrap one into something dereferenceable.
- **Unsafe hygiene**: `#![deny(unsafe_op_in_unsafe_fn)]` and
  `clippy::undocumented_unsafe_blocks` in the unsafe crates — every
  `unsafe` block carries a checked `// SAFETY:` invariant comment.
- **No-panic policy in safe crates** (from D13):
  `clippy::unwrap_used` and `clippy::expect_used` denied at crate
  level with no `allow` escapes — see §3b.
- The safe crates spawn no threads and use no async runtime — a
  memory-model rule, not just a dependency rule (§3j).

## 3. The memory model and the FFI designs

This section is normative. It exists so that ~200 call sites do not
each invent an answer, and so the safe crates never contain one.

### (a) Ownership taxonomy and object-death inventory

Our sources use five distinct ownership relationships with C; each
gets exactly one wrapper shape. (Evidence: file:line in this repo.)

| Kind | Examples | Wrapper shape | Safe-side view |
|---|---|---|---|
| 1. C owns, announces death | outputs, heads, seats, surfaces, views | registry id (§3b) | id; `Option<…>` on resolve |
| 2. C allocates on request, **we** destroy | `weston_desktop_surface_create_view` → `unlink_view` → `weston_view_destroy` ordering (`shell.c:218-220`); curtains (`shell.c:1958-1961`); log scopes, recorder | `Owned<T>` RAII in `weston`, `Drop` = destroy in the documented order | an id the shell may explicitly drop |
| 3. **We** allocate, C borrows by address | `wl_listener` (28 fields); grab structs (`shell.c:117-140`); the `weston_desktop_api` vtable; `weston_*_backend_config` | `Pin<Box<…>>` inside `weston`, never exposed | nothing — invisible |
| 4. C struct **embedded by value** in our allocation | `struct weston_layer` inside `struct workspace` (`shell.h:34`) and `desktop_shell::background_layer` (`shell.h:60`) | pinned, address-stable field inside `weston`; must not move after `weston_layer_init` | `LayerId` |
| 5. C data valid for one callback only | event args (`const struct timespec *`, coords), string getters | copied at the fence (§3h) | owned values |

Kind 4 is the silent one: `weston_layer_init` registers an intrusive
list node *at the struct's address*; a movable Rust holder (a `Vec`
that reallocates, a by-value builder) corrupts libweston's layer list
with no compile error. Rule: **every kind-3/kind-4 allocation lives
behind `Pin<Box<…>>` created inside `weston`; the safe crates cannot
name the type.**

Not every libweston object announces its death; the wrapper design
keys off this table (verified against the libweston headers and our
sources — see the R0 re-verification note below):

| Object | Death announced by | Handle |
|---|---|---|
| `weston_compositor` | `destroy_signal` (`weston_compositor_add_destroy_listener_once`) | singleton owned by `weston` |
| `weston_output` / `weston_head` | destroy signal + dedicated add/get-listener API | registry id |
| `weston_seat`, `weston_surface`, `weston_view` | `destroy_signal` | registry id |
| `weston_desktop_surface` | **no signal** — the `surface_removed` API callback | id, invalidated in the `surface_removed` trampoline |
| `weston_pointer` | `destroy_signal` | never stored — scoped access only |
| `weston_keyboard`, `weston_touch` | **no destroy signal at all** | never stored — scoped access only |
| `weston_layer` | no signal — *we* own it | owned/pinned (kind 4) |

Two normative rules fall out:

- **Input sub-objects are never stored.** They come and go with seat
  capability changes and (for keyboard/touch) cannot be invalidated
  on death even in principle. The safe layer re-fetches from a
  `SeatId` at each use — exactly what the C code does
  (`weston_seat_get_pointer()` at `shell.c:1135`) — via a scoped
  accessor that cannot escape the call:
  `fn with_pointer<R>(&self, seat: SeatId, f: impl FnOnce(&PointerState) -> R) -> Option<R>`.
- **`weston_desktop_surface`'s "half-dead" window is explicit.**
  After `surface_removed`, our per-surface state may still be worked
  on while the C object is gone (`shell.c:1288-1293`). The shell
  models it as data captured in the removal event (§3e), not as a
  nulled pointer.

*Reference-tree caveat (found at finalization):* the earlier review
verified header claims against the upstream `weston` **main** tree
(15.0.90-dev), not 14.0.1 — and at least one API our code uses
differs between them (`weston_curtain_params` has a `get_label`
callback in 14, a `char *label` in 15). The table above is grounded
in what *our C sources* actually consume, which is authoritative;
nevertheless **R0 re-verifies every §3 header fact against the
installed EPEL `weston-devel` 14.0.1 headers** (the same headers
bindgen consumes), and the pkg-config version tripwire (§6) keeps it
true across bumps.

### (b) Handles: generational ids resolved through the wrapper (D13)

`Copy` newtypes around `NonNull<T>` — the design in an earlier draft
— are **unsound as a safe API** and are rejected: a stale copy used
from a `forbid(unsafe_code)` crate is a use-after-free with no
`unsafe` on the failing line. Two different objects really can share
a recycled address, so a pointer handle cannot even be *compared*
reliably. The C code pays for this hazard by hand at every site
(`destroy_shell_grab_shsurf()` nulling at `shell.c:178`; the
`focused_surface` invalidation walk with its warning comment at
`shell.c:1277-1287`; the half-dead surface at `shell.c:1291`).

Instead, **stored references are generational ids**:

- `weston` keeps a per-kind slot table: `Vec<Slot>` with
  `Slot = { ptr: Option<NonNull<T>>, generation: u32 }`.
- Safe crates hold `OutputId`, `HeadId`, `SeatId`, `SurfaceId`,
  `ViewId`, `DesktopSurfaceId`, `LayerId`, `CurtainId` —
  `Copy + Eq + Ord + Hash + Debug`, carrying `(slot, generation)`
  and **no pointer** (opaque per §2).
- Resolution is an array index plus a generation compare (~1–2 ns).
  A dead or recycled slot resolves to `None`.
- **Registration and destroy-listener attachment happen in one
  wrapper function**, so an object is never in the table without the
  listener that will invalidate its slot. Invalidation bumps the
  generation; every id minted for the old occupant is permanently
  stale.
- The reverse direction (C pointer → id) is needed only on cold
  paths (hotplug, seat add, surface add) and inside trampolines; a
  pointer-keyed map or linear scan inside `weston` serves it. The
  desktop-surface user-data slot — one of only **two** user-data
  slots the installed API has (`weston_desktop_surface_{set,get}_
  user_data`; `weston_compositor_get_user_data`, which is get-only
  and, during R1, belongs to the C frontend — §7) — carries the id
  for the hottest lookup (`get_shell_surface()`, `shell.c:1198-1206`).
  Everything else has no slot, so **the registry is the rule and the
  slot is a private optimization**: one mechanism, one place to audit.
- **Uniform ids at the shell boundary** *(amendment A2 to D13)*: the
  earlier review proposed passing callback *subjects* as special
  live-borrow types needing no check. That refinement is dropped from
  the safe-crate API: with dispatch deferral (§3e) a subject can in
  principle be dead by handling time, and two handle kinds are more
  model than the shell needs. The shell sees **ids everywhere**, and
  every resolution is checked — at ~1–2 ns per check, against 1–5 ms
  of real work per repaint, uniformity is free (the earlier review's
  own rate estimates: a few thousand callbacks/s at worst; even a
  pessimistic 25 ns check costs ~0.01 % of a core). Live borrows
  survive only *inside* `weston` and in scoped accessors that cannot
  escape a closure.

**Stale-resolution policy (D13): `Option` everywhere, no
exceptions.** The normal handler shape is

```rust
let Some(surface) = host.surface(id) else { return };
```

— a missed cleanup site degrades to skipped work plus one log line,
instead of C's silent use-after-free (or a crash later in unrelated
code). **No `unwrap` or `expect` on a value obtained from `weston`
appears anywhere in the safe crates** (lint-enforced, §2). This is
deliberately stricter than "expect where provable": a panic in a
compositor kills the user's session, and "provable" invariants in
teardown paths are exactly the ones that stop being true after a
refactor. Functions that must yield a value get a documented
fallback: the one place the C code asserts on exactly this,
`get_output_work_area()` (`assert(sh_output)`, `shell.c:266`, after
zero-initializing the rectangle at `shell.c:257-260`), becomes
log-and-return-zeroed-rect — recorded as intentional divergence §10.
The rule bans choosing panic as the *stale-reference* policy; it does
not ban panics for genuine bugs (§3g) or `debug_assert!` on
invariants that are not resolution results.

Where staleness genuinely comes from — this list is why checked
cross-references are load-bearing, not defensive programming; five of
six shapes already live in our C code:

1. Cleanup is itself a callback while other state still points at the
   dying object (`desktop_surface_removed()` walking seats,
   `shell.c:1277-1287`; children fixup `shell.c:213-217`;
   `focus_state_surface_destroy()` hunting a new focus target from
   inside a destroy handler, `shell.c:362-402`).
2. Our objects deliberately outlive the C object (`shell.c:1291`).
3. libweston destroys things in the middle of our outbound calls
   (`weston_seat_break_desktop_grabs()`, `shell.c:238` — frees other
   grabs and runs our handlers before returning).
4. libweston's own destruction is sometimes deferred
   (`wet_output_destroy()`, `main.c:2686-2696`, forcing the destroy
   callback early for DRM).
5. Some objects never announce death (keyboard/touch — table above).
6. Dispatch deferral (§3e) adds one new, by-design window: an event
   drained after its subject died. The id resolving to `None` is the
   designed outcome, not an error.

The registry does not *replace* the dozen scattered hand-cleanup
sites the C code has — it does the same cleanup once, centrally, and
changes the failure mode of a missed site from memory corruption to a
skipped event.

### (c) `Listener`: the `wl_listener`/`container_of` primitive

One primitive owns the pervasive idiom (34 `container_of` sites, 29
listener fields): `Listener` owns `Pin<Box<Inner>>` where `Inner`
embeds the `wl_listener` plus what the trampoline needs; a single
`extern "C"` trampoline per signal shape does the one `container_of`
and hands control to dispatch (§3e). `Drop` detaches. Normative
details the C idioms force:

- **Explicit `attach`/`detach`/`is_attached`.** The C code uses
  `wl_list_empty(&listener.link)` as "am I attached?" state and
  attaches/detaches dynamically (`shell_seat_caps_changed()`,
  `shell.c:1128-1145`). `Drop` must be correct in both states;
  `wl_list_init`-after-remove (the C idempotent-remove idiom) is an
  internal invariant of the primitive, never a pattern safe code
  repeats.
- **Listener identity is never a lookup key.** Three C sites use the
  notify function pointer as a registry key (`get_shell_seat()` via
  `wl_signal_get`, `shell.c:1179-1193`;
  `wet_head_tracker_from_head()`, `main.c:1940-1951`;
  `wet_output_from_weston_output()`, `main.c:2674-2685`). A shared
  trampoline collapses those keys, and per-generic-instantiation
  addresses are unspecified under linker/compiler identical-code
  folding — do not build on them. All three lookups become registry
  lookups keyed by the C object; `weston`'s public API never exposes
  a listener's identity or address. This is a straight
  simplification: `get_shell_seat()` becomes a map lookup.
- **Intra-Rust notification never goes through `wl_signal`.** The
  shell today embeds *its own* `wl_signal destroy_signal` in
  `shell_surface` (`shell.c:89`, emitted at `shell.c:222`) with its
  own grabs as listeners (`shell.c:243, 301`) — Rust→Rust
  notification routed through C machinery. In the port this
  disappears into the shell's own state (registry invalidation plus
  explicit fixups in the dispatch path), deleting an entire unsafe
  surface a transliteration would recreate.

### (d) C vtables and versioned config structs

`weston_desktop_api` (14 entries, `struct_size`d) is filled by static
`extern "C"` trampolines; the shell implements plain handler methods
(§3e). Same pattern for the xwayland API table and the
`weston_*_backend_config` builders (which set
`struct_size`/`struct_version` from the *bound* headers — risk R-C).
The vtable structs are kind-3 allocations: pinned inside `weston`,
invisible to safe code.

### (e) Dispatch: two-tier, one borrow at the edge (D14, amended)

The single most consequential design decision. libweston callbacks
can re-enter shell code synchronously (signal emission during
teardown, grab cancellation during grab start, focus changes during
activation). An earlier draft accepted reentrancy and parked all
mutable state in `RefCell`s inside the safe crates — that puts borrow
discipline in application code, converts missed cases into
`BorrowMutError` session-killing aborts, and is the opposite of goal
2. Decided instead:

- **The safe crates carry no interior mutability at all.** The shell
  is `struct Shell { … }` of plain data, with
  `fn on_event(&mut self, host: &mut dyn ShellHost, ev: ShellEvent)`
  handlers. The one `RefCell` holding it lives in `weston`, borrowed
  once per dispatched event at the edge and released before the next.
- **Deferrable tier** (the default; most callbacks): the trampoline
  captures a small `ShellEvent`/`FrontendEvent` value and enqueues
  it. **Events carry eagerly-captured payload, not just ids**: a
  removal event includes what the handler will need (e.g. the title
  for logging, the seats it was focused on), because by handling time
  the id resolves to `None` by design.
- **Sync tier** (the audited exception): callbacks where C reads a
  return value or out-parameter immediately, or where deferral would
  change protocol-visible ordering. Named exhaustively (this list is
  closed; growing it requires a PR touching this section):
  - `weston_desktop_api::get_position` — out-params (`shell.c:1597-1605`)
  - `weston_curtain_params::get_label` (`shell.c:1926-1930`)
  - log handlers `vlog`/`vlog_continue` returning a char count
    (`main.c:214, 241`, installed at `main.c:4511`) — via the shim (§3k)
  - xwayland `spawn_xserver` — must fork/exec and return the
    `wl_client *` synchronously (`frontend/xwayland.c:105`)
  - head-changed / `simple_output_configure` — must configure the
    output inside the flush (`main.c:129-134`)
  - grab callbacks — must move views within the input frame (§3f)
  - `weston_desktop_api::{surface_added, surface_removed, committed,
    move, resize}` — added/removed bracket the C object's life;
    committed must map the surface inside commit processing;
    move/resize read pointer button/serial state that is only
    meaningful inside the request frame (`shell.c:1400-1455`)
- **Sync-tier borrow rule** *(amendment A3, sharpening D14)*: a
  sync-tier handler may take the app borrow **only if the callback
  inventory records a proof that it cannot be invoked while the
  borrow is held** (i.e. it is only ever invoked from client-request
  dispatch, commit processing, or event-loop sources — never
  synchronously from any outbound call the app makes). Handlers
  without such a proof (labels, `get_position`, grab motion, log)
  must answer from wrapper-held or callback-local data only. The
  destroy notifications are split accordingly: **registry
  invalidation happens synchronously in the trampoline** (so no
  later resolution can touch the dying object), while the policy
  reaction is a deferred event.
- **Drain at the edge, by depth counter** *(amendment A4 to D14)*:
  `weston::Ctx` keeps a dispatch-depth counter, incremented around
  every trampoline body **and every outbound FFI call**. Whoever
  returns the counter to zero drains the queue — each event taking
  the app borrow once — and then runs the pending-drop list (§3f).
  Consequences, each load-bearing:
  - In the common (non-reentrant) case a "deferred" event runs at
    the same stack boundary where the C code ran its handler —
    ordering-preserving, so deferral is invisible to the e2e oracle.
  - A nested trampoline (fired synchronously by one of our outbound
    calls while an event is being handled) finds depth > 0 and
    enqueues; the outer drain loop processes it next. The
    reentrancy that was UB in C and would be a `BorrowMutError`
    abort under the rejected design becomes an ordinary queued
    event.
  - Events enqueued during outbound calls made *outside* any
    trampoline (e.g. `weston_compositor_flush_heads_changed` called
    from startup code — the case the earlier "drain with no C frame
    live" wording left unanswered) drain when that outbound wrapper
    returns to depth zero. No idle-source scheduling, no unanswered
    "who drains?".
  - At compositor teardown, events left in the queue after
    `shell_destroy` are discarded — the loop is exiting and policy
    reactions are moot; the sync invalidation half already ran.
- Any behavior difference caused by deferral that the e2e suite can
  observe is a **parity bug**, not an accepted divergence under D6.
- **Was a known gap (R2d/R2e review rounds), FIXED 2026-07-28.**
  `Compositor::run` used to wrap `wl_display_run` in `with_depth`, so
  the counter never returned to zero *inside* the event loop:
  trampolines bottomed out at depth 1 and the "common case" above
  never fired.  Two consequences, both measured before the fix:
  - **Ordering.** Every deferred-tier event (busy-cursor
    PingTimeout/Pong, GrabEnded, the destroy-policy Gone events) was
    held until the loop exited and then delivered in one batch.  The
    user-visible half was focus-after-close: `TrackedSurfaceGone`
    drives `focus_surface_gone` (C `focus_state_surface_destroy`), so
    closing the focused window left the survivor unfocused for the
    rest of the session.  `test_focus_moves_to_survivor_when_
    focused_window_closes` was written against the *unfixed* build to
    pin this down — it passed on the C oracle and failed on the Rust
    frontend, i.e. a genuine parity bug, not a re-spec.
  - **Memory.** The same condition gates the *pending-drop* list
    (§3f), so every box retired through `defer_drop` — ended grabs,
    retired registry listeners — was held for the compositor's whole
    lifetime.  Instrumenting `defer_drop` showed the list growing
    monotonically (`len=1,2,3` at `depth=2`) across a single
    move-grab test, i.e. growth per ordinary window drag.  (Nothing
    was unsound: the boxes stayed owned and reachable.)

  The fix is to *not* wrap the one outbound call that is itself the
  loop, so the loop's base depth is zero and each trampoline's own
  wrap goes 0→1→0, draining at its edge — where the C code ran the
  same handler.  Audit performed with it: the only bare-bodied entry
  points were the two event-loop signal callbacks (`on_sigchld`,
  `on_term_signal`), which now wrap their own bodies so one handler
  means one drain (otherwise `on_sigchld`'s inner outbound calls
  would drain between two `waitpid` iterations); everything else
  reaches Rust through `Listener::trampoline`, `with_ctx`,
  `guard_ctx` or the binding/xwayland/screenshooter wrappers, all of
  which already wrap, plus label/log/no-op vtable entries that touch
  no `Ctx` state and must stay unwrapped (the log sink especially —
  draining from inside it would run app handlers on a log call).

  Risk was lower than it looks: the **hybrid** configuration has
  dispatched our trampolines from C's own `wl_display_run` since R1,
  i.e. at base depth zero, so drain-at-the-edge and §3f
  free-at-the-edge have been under destroy-storm stress all along —
  the fix makes the pure-Rust frontend behave the way the hybrid
  already did.  `rust-stress-test.sh` now takes `WESTONITE_BIN` so
  the storms run against both.

**The `ShellHost` trait (D20).** The shell reaches the wrapper only
through a trait implemented by `weston` (queries returning
`Option<plain data>`, commands like `activate`, `move_to_layer`,
`start_move_grab`, `set_size`…), with a mock implementation for
tests. `westonite-shell`'s state machine — focus, stacking, grab
policy, xdg lifecycle — is thereby unit-testable in `cargo test`,
including the focus-race and teardown-ordering cases §3b lists as
the risky ones. For the deep half that lands first (D4) this is the
difference between testing the logic directly and only ever reaching
it through VNC. Cost accepted: one trait boundary kept in sync with
`Ctx`. The frontend gets the same treatment only where it has real
logic (output layout/clone-mirror resolution); its plumbing modules
do not need a mock boundary.

**Callback inventory table (R0 deliverable — lives at
`docs/callback-inventory.md`).** Every `wl_listener` field (29),
every attach site (23), the 6 bindings, and every
`weston_desktop_api` field of the bound 14.0.1 struct (the "14
entries" of §1 count struct fields; the C vtable at
`shell.c:1607-1619` installs 10 — unfilled fields get one-line "not
installed" rows so the counts reconcile). Fixed columns, one row per
callback: symbol/API entry; kind (listener / binding / desktop-api /
grab / log); C attach site (file:line); tier (sync/deferred); the
event it enqueues or the data it answers from; reentrancy hazards;
the non-reentrancy proof (A3 — required for sync-tier entries that
borrow app state, `—` otherwise); port status. The table is small
enough to be exhaustive and it is the audit surface for the whole
fence — a reviewer checks the "no unsoundness outside `weston`"
claim against it, not against 9.7k lines.

### (f) Grabs: the sixth primitive, and the sharpest

Grabs combine every hard property at once and were missing from the
earlier five-primitive list. In C: our allocation embeds the C grab
struct and is handed to libweston by address (`shell.c:117-140`,
`weston_pointer_start_grab` at `shell.c:247`); **the allocation is
freed from inside its own callback** at nine sites (`shell.c:660,
671, 798, 815, 881, 914, 1523-1524` pointer; `shell.c:530, 564`
touch); starting a grab can synchronously cancel and free a
*different* grab (`weston_seat_break_desktop_grabs`, `shell.c:238,
296`); the grab holds a weak back-reference nulled on surface death
(`shell.c:177-186`); and the C free relies on the embedded struct
sitting at offset 0. The design:

- A `Grab` primitive in `weston` owning `Pin<Box<GrabInner>>`, vtable
  filled by static trampolines (§3d shape). Offset math in exactly
  one place; never offset-0 casting.
- **Grab-local state lives in the grab box** (surface id, delta,
  edges, active flags), so motion/button trampolines touch only that
  box + `Ctx` — sync tier with **no app borrow** (A3), which is what
  makes grab handling safe against every reentrancy shape above.
  Policy consequences (clear the grabbed flag, activate on click) are
  enqueued events.
- **Deferred drop**: ending a grab moves the box into `Ctx`'s
  pending-drop list, drained at depth zero (§3e) — never freed
  inside the frame C is dispatching on, including the
  cancel-during-start case.
- The back-reference is a `SurfaceId`; "grabbed surface died
  mid-grab" is a `None` resolution, not a nulled pointer.
- Safe-side view: the shell stores
  `enum ActiveGrab { None, Move {…}, Resize {…}, Busy {…} }` in plain
  state, mirrors of the policy only. It never sees the allocation,
  never frees anything, and cannot express "free while dispatching".

This is the single richest test of the wrapper design — a good part
of why the shell goes first (D4) — and R1's exit criteria include a
grab stress case (start/cancel/destroy races, §7).

### (g) Panics across FFI (D16)

`panic = "unwind"` in all profiles, `catch_unwind` at every
trampoline → `weston_log` with callback context → deliberate
`abort()`. No unwinding ever crosses the C boundary. (An earlier
draft specified `catch_unwind` *and* `panic = "abort"` together —
inconsistent: with abort the catch never runs and the diagnostic is
lost exactly where it matters, in release.) Rejected alternative:
`panic = "abort"` + `set_hook` — smaller, but loses per-callback
context. Cost accepted: unwind tables. Two required details: the
handler must also be installed by `wet_shell_init` itself (R1's
cdylib is loaded by the C frontend, which installs nothing), and the
child side of `pre_exec` in `westonite-spawn` must abort, never
unwind.

### (h) Boundary value rules

Fixed once, here, so call sites do not decide:

- **No `weston-sys` type in any safe-facing API** (§2). POD geometry
  (`weston_coord_global`, `weston_geometry`, `pixman_rectangle32_t`)
  is re-declared as plain Rust types and converted at the fence —
  never transmuted without layout assertions in `weston-sys` tests.
- **Nullable returns become `Option`** (`weston_seat_get_pointer/
  keyboard/touch`, `weston_head_get_output`, our own
  `get_default_view()`; the bindgen `*mut T` hides legitimate NULLs
  — the wrapper documents nullability per function in its SAFETY
  notes).
- **Strings are owned at the fence.** `weston_head_get_name`,
  `weston_desktop_surface_get_title/get_app_id`: client-controlled,
  possibly NULL, not guaranteed UTF-8, valid only until the object
  changes. Safe crates get `Option<String>` via `from_utf8_lossy` —
  never a borrowed `&str` whose lifetime is really "until the next
  commit".
- **Int flags become `bitflags`/enums at the fence** (activate
  flags, resize edges, transforms, modifier masks); no `u32` flag
  reaches the shell.

### (i) Boundary cost rules (D19)

Handle resolution is not where per-event cost lives — **boundary
marshalling is**, at 100–1000× a liveness check. Four rules, designed
in rather than profiled out:

- **Cache strings; do not fetch per event.** Title/app-id fetched on
  the change notification, cached in shell state — never per commit.
- **No `Vec` allocation per list snapshot** (§3l): `SmallVec`-style
  inline capacity sized to realistic counts (outputs, seats, a
  window's children), or a scratch buffer reused across callbacks.
- **No allocation per deferred event**: a reusable `VecDeque` of a
  small fixed-size event enum (box the rare large variant).
- **Format log messages lazily**, behind the scope-enabled guard the
  C code already uses (`weston_log_scope_is_enabled()`; `main.c:221`
  flight recorder, `main.c:283` protocol scope). The safe logging
  macros preserve the guard. Outbound logging always formats in Rust
  and passes the result as a `%s` *argument* — client-controlled
  text (titles, config values) must never reach C as a format
  string.

### (j) Globals and threads

`main.c` keeps process-global mutable state (`weston_logfile`,
`log_scope`, `protocol_scope`, `cached_tm_mday` — `main.c:160-163`).
Rules: no `static mut` anywhere; this state lives in `weston` behind
its own cell, handles outward. Wrapper types are `!Send + !Sync`
(libweston is single-threaded), and the composition argument that
makes the id design defensible belongs in `weston`'s crate docs:
**ids are inert data and may be `Send`, but resolving one requires
`&Ctx`, which is `!Send` — so no other thread can reach C state even
if it obtains an id.** `debug_assert!(on_main_thread())` at every
`Ctx` entry point, kept enabled in CI builds. Safe crates spawn no
threads (§2).

**How trampolines reach `Ctx` (D21).** One private thread-local
slot in `weston` (`Cell<Option<NonNull<CtxInner>>>` or equivalent),
set when the `Ctx` is created (`wet_shell_init` in R1, `main` at R3)
and cleared when it is destroyed. Every trampoline — listener (§3c),
vtable (§3d), grab (§3f), and the Rust side of the log shim (§3k) —
resolves `Ctx` through this slot; `Inner` boxes carry no `Ctx`
pointer. One mechanism because at least one registration API has no
user-data pointer at all (`weston_log_set_handler`'s handlers,
installed at `main.c:4511`) and so needs a thread-local regardless —
a second, per-box mechanism would only add audit surface. The slot is
not `static mut` (per-thread, safe accessors); its soundness rests on
the single-thread rules above: `!Send` wrapper types mean no other
thread can set or read it. A trampoline firing while the slot is
empty would be a teardown-ordering bug inside `weston` itself (every
attachment is bounded by `Ctx`'s lifetime — `Listener::drop`
detaches); it `debug_assert!`s and returns a benign default in
release.

### (k) The C shim: static inlines **and** variadics

The `cc`-compiled shim in `weston-sys` covers (1) the `static
inline` helpers in the installed headers that the port actually uses
(or trivial Rust reimplementations, e.g. timespec math — the exact
set is enumerated at R0 from the 14.0.1 headers), and (2) the
**`va_list` interfaces**, which stable Rust cannot define: the
`weston_log_set_handler` pair (`vlog`/`vlog_continue`) and the
pre-`weston_log`-init `custom_handler` (`main.c:172-245`) must be C
functions that forward to `weston_log_scope_vprintf` or `vsnprintf`
into a buffer and call into Rust with a length-delimited string.

### (l) Iterator invalidation across the fence

The C code walks libweston's intrusive lists while mutating them and
survives via `wl_list_for_each_safe` — or via a deliberate
restart-loop where even that is insufficient
(`desktop_shell_destroy_layer()`, `shell.c:2047-2084`, whose comment
explains that `_safe` cannot handle removal of the *next* item).
Rule: **all C-list traversals exposed to the safe layer are
snapshots** (`Vec<Id>`/inline small-vec of ids), taken while no safe
code runs; a lazily-borrowed C-list iterator is never live across
any call that can mutate the list. With checked ids this is
naturally correct: a snapshot entry that dies mid-iteration resolves
to `None` and is skipped — the restart-loop pattern becomes an
ordinary loop.

## 4. Component-by-component mapping

| C source | Rust destination | Approach |
|---|---|---|
| `shared/option-parser.c` | deleted — replaced by `westonite-config` (clap) | The re-specified CLI (§5, D10) supersedes weston's option parser. Positional-args-become-autolaunch behavior is preserved (clap trailing args). |
| `shared/os-compatibility.c` | deleted | EL10 glibc has `memfd_create`, `mkostemp`, `strchrnul`, `posix_fallocate`; `os_socketpair_cloexec` becomes a safe rustix `socketpair(.., SOCK_CLOEXEC)` call inlined at its call sites (xwayland, screenshooter fd path). |
| `shared/process-util.c` | `westonite-spawn` | `CustomEnv` (incl. the `ENV=x cmd arg` exec-string parser), `Fdstr`; spawn via `Command` + `pre_exec` (fd unCLOEXEC, setsid, sigmask reset — async-signal-safe rustix only), replacing the fork/exec block in `main.c:421-510`. |
| `shared/*.h` (helpers, timespec, xalloc, string-helpers, fd-util) | deleted | std (`Duration`, `?`, `String`), rustix; `xalloc` is irrelevant in Rust. |
| `frontend/executable.c` | deleted | The `westonite` bin *is* the entry point at R3 (D2); the 5-line C `main` survives only through the hybrid phases. |
| `frontend/main.c` | `westonite` bin, split into modules | CLI/config parsing moves to `westonite-config` (§5); the bin keeps `log.rs` (weston-log ctx, flight recorder, scopes), `backend/{drm,headless,x11,wayland,rdp,vnc,pipewire}.rs`, `output.rs` (head tracking, clone/mirror, color mgmt — the big one), `input.rs`, `autolaunch.rs`, `process.rs` (sigchld + wet_process list), `xwayland.rs` — each consuming its typed slice of the resolved `Settings`. |
| `frontend/config-helpers.c` | deleted | Typed serde deserialization makes the string→typed-value getters moot. |
| `frontend/weston-screenshooter.c` | `westonite::screenshooter` | Ported 1:1 (D1). Semi-dead at runtime (Super+S spawns a client we don't ship; Super+R wcap recorder works) — behavior preserved as-is. **Ownership fix at R3**: today the *shell* calls `screenshooter_create()` (`shell.c:2232`); in the end state the frontend initializes its own screenshooter after `shell_init`, and the shell no longer references it. Same bindings registered at the same point in startup — behavior-identical, recorded under §10 as a structural (not behavioral) change. |
| `frontend/xwayland.c` | `westonite::xwayland` | Socketpairs via rustix, lazy spawn via `westonite-spawn`, SIGUSR1 via `wl_event_loop` signal source (bound). The C code's lazy `[xwayland] path` config read (`xwayland.c:153-155`) becomes a plain field of the resolved `Settings` (§5). |
| `desktop-shell/shell.c` | `westonite-shell` crate | `state.rs` (Shell root state), `surface.rs` (per-surface state + desktop-api handlers), `focus.rs`, `grabs.rs` (policy enums only — §3f), `background.rs` (curtains/output tracking), `workspace.rs`. All against `ShellHost`. Exported entry: `wet_shell_init`-compatible `extern "C"` in the R1-only `westonite-shell-plugin` cdylib (§2 — entry point there, bootstrap in `weston`, logic in this crate), plain fn once static at R3. |

**A latent C bug the port must not transliterate** (found at
finalization): `has_keyboard_focused_child_callback()`
(`shell.c:955-970`) passes `&has_keyboard_focus` — where
`has_keyboard_focus` is already a `bool *` — as the recursion's
user_data, so at child-nesting depth ≥ 2 the callback writes `true`
through a `bool **` misread as `bool *`, corrupting a stack pointer
byte. The Rust version implements the obviously-intended recursion
(any transitive child focused ⇒ activated). Recorded in §10: where
the C behavior at depth ≥ 2 is UB, "parity with the intent" is the
bar, with an e2e/unit case pinning the intended behavior.

## 5. Configuration interface (re-specified — D9–D12)

Background (verified against the C sources and the upstream tree):
libweston itself never reads the CLI or the ini — it is configured
purely through API calls and versioned config structs assembled by
the frontend. The ini leaked only *within our own code* (the shell
reads `[shell] background-color` via `wet_get_config`; xwayland
reads `[xwayland] path` lazily) and *out of the process* via the
`WESTON_CONFIG_FILE` env export. With config parity dropped, the
interface is re-specified rather than ported:

- **Model** (D9): one `Config` struct in `westonite-config`
  (serde `Deserialize`), covering the **entire current option
  surface** — every `[section]` key and CLI option in
  `docs/frontend-capabilities.md` / `docs/desktop-shell-capabilities.md`
  maps to a field (the completeness checklist, §9 R-G). File format
  is **TOML** (`westonite.toml`, same XDG search order as the ini
  had; `--config`/`--no-config` kept). Repeated `[output]` sections
  become `[[output]]` array-of-tables. Unknown keys and type errors
  are **startup errors with line/column spans**
  (`deny_unknown_fields`) — replacing weston's silent-typo behavior.
  Key spelling is **kebab-case** throughout (one
  `#[serde(rename_all = "kebab-case")]`) — matching the ini's
  hyphenated keys, so the D11 mapping table is near-diagonal: most
  entries change section syntax only, not key names.
- **CLI** (D10): clap derive. Ergonomic flags for the scalar/global
  settings (today's CLI surface), plus a generic
  `-o`/`--set key.path=value` dotted override that patches the
  config tree before deserialization — 100 % CLI coverage of the
  file surface without bespoke flags for structured sections.
  Positional trailing args remain the autolaunch command.
- **Resolution**: defaults → file → `-o` overrides → flags, into an
  immutable `Settings` resolved once at startup. No lazy config
  reads anywhere; consumers receive typed slices (`ShellConfig`,
  `XwaylandConfig`, per-backend structs). The shell never sees the
  file or the CLI.  (This clause originally also promised third-party
  `modules=` plugins their argv and nothing else; `modules=` is
  dropped as of 2026-08-01 — see D2 — so the `wet_get_config`
  contract simply ends at R3 with no consumer to carry.)
- **Don't over-model**: values that are really weston/libweston
  string grammars (modelines `1920x1080@60`, XKB rule names,
  `gbm-format`, ICC paths, transform names) stay strings or thin
  enums wrapping the existing parse points; §5 re-specifies
  *structure and validation*, not weston's value syntaxes.
- **Migration** (D11): docs only — an annotated
  `westonite.toml.example` plus an ini→TOML mapping table in the
  README/man material. No converter tool, no dual-format fallback.
  If a legacy `westonite.ini` is found where the TOML is expected,
  startup logs a one-line hint and otherwise ignores it.
- **Env export** (D12): the `WESTON_CONFIG_FILE` setenv is
  **dropped** — we ship no clients that read it and no stock client
  could parse TOML. libweston-consumed env vars
  (`WESTON_MODULE_MAP`, `WESTON_LIBINPUT_LOG_PRIORITY`, …) are
  untouched.
- **Safety dividend**: the config layer becomes 100 % safe code and
  `weston-sys` drops the `config-parser.h` bindings at R3; the only
  remnant is the small R1-only binding the hybrid phase needs (§2).

## 6. Build & packaging

- **Build**: cargo replaces meson at the end state. `weston-sys`
  resolves the RPM headers via `pkg-config libweston-14` in
  `build.rs`. During the hybrid phases meson keeps building the
  not-yet-ported C parts and cargo the Rust parts (meson `custom_target`
  or just the CI script invoking both).
- **bindgen**: pre-generated `bindings.rs` is **committed** (D5),
  with a regen script + CI sync check. RPM builds get fewer
  BuildRequires and full reproducibility; the headers only change
  when EPEL bumps weston, which is exactly when we re-run the script
  (tripwire: build.rs asserts the libweston-14 pkg-config version
  matches the bindings' recorded version). `clang-libs`/bindgen are
  needed only by the regen script, not by builds.
- **Toolchain**: EL10 AppStream `rust-toolset` (rolling, recent
  rustc). MSRV = whatever the build image's rust-toolset ships;
  edition 2024.
- **Dependencies policy** (D8): the approved runtime set is
  `libc`, `rustix`, `bitflags`, `thiserror`, plus — approved with
  D9/D10 — `serde`, `serde_derive`, `toml`, `clap` (+ `cc` at build
  time; bindgen only in the regen script). Further crates are
  allowed but **each addition requires explicit owner approval**
  before it lands (record the approval in the PR/VENDOR-log entry).
  No tokio, no wayland-rs. The §3 primitives (slot table, event
  queue, small-vec) are hand-rolled in `weston` (~a few hundred
  lines total) rather than pulling
  `slotmap`/`smallvec` — fewer supply-chain surfaces in the fence
  crate; revisit per-crate under D8 only if the hand-rolled versions
  grow past their audit value. RPM builds offline from a
  `cargo vendor` tarball (`Source1`), per standard EL Rust packaging
  practice.
- **RPM/installed layout unchanged**: `/usr/bin/westonite`,
  `%{_libdir}/westonite/*.so` while the cdylib phases last, same
  `Requires: weston-libs`. Spec swaps meson/gcc BuildRequires for
  rust-toolset + vendored crates.
- **CI**: existing image-build → build+smoke+e2e → rpmbuild →
  pristine install (incl. installed-RPM e2e subset) pipeline
  unchanged in shape; adds `cargo fmt --check`,
  `clippy -D warnings`, `cargo test`, and the fence checks from §2:
  dependency-graph check, bindings-drift check,
  `unwrap_used`/`expect_used`/`undocumented_unsafe_blocks` lints,
  and the sanitizer legs (D18, §7): nightly-ASAN for the pure-Rust
  smoke plus valgrind for the hybrid destroy-storm.  (The `cargo
  public-api` snapshot is a known gap — see §2; UBSAN is not run.)

## 7. Phasing (strangler fig — a working compositor at every step)

The binary and the shell `.so` are separately loadable, so C and Rust
halves can be mixed and smoke-tested at every phase boundary.

- **Phase R0 — foundation** ✅ *(done — see PROVENANCE.md log)*: workspace, `weston-sys` (bindings for
  the full header set, **verified against the installed 14.0.1
  headers** — §3a caveat), `weston` with the primitives of §3:
  registry + ids, `Listener`, vtable builders, two-tier dispatch
  with the depth-counted drain, `Grab`, panic barrier, `ShellHost`.
  Deliverables beyond code: the **callback inventory table** (§3e)
  and the **fake-C-object unit harness** (a few `wl_signal`s and
  structs in the shim so `Listener`, `Grab`, the registry, and the
  trampolines are testable without a compositor — D18). The
  sanitizer legs land here, not at R4. Exit criterion: a throwaway
  Rust binary brings up compositor + headless backend + noop
  renderer, runs the event loop, exits 0 on SIGTERM (mirrors the
  Phase-1 smoke test) — run plain, under valgrind, and under
  nightly-ASAN (address only; UBSAN has no stable Rust story and is
  not run).
- **Phase R1 — shell in Rust, frontend still C** ✅ *(done — see PROVENANCE.md log)*: port `shell.c` →
  `westonite-shell`, shipped as `desktop-shell.so` via the
  `westonite-shell-plugin` cdylib (§2 — entry point there, bootstrap
  in `weston`; the build step renames cargo's artifact on install),
  linking `libexec_westonite.so`
  for **both** contract symbols (§1): `wet_get_config` — the C
  frontend still parses `westonite.ini`, so the shell reads its one
  key through a temporary compat path (hand-declared externs +
  `dlsym(RTLD_DEFAULT)` in `weston::shell_init`, so neither the
  bindings nor the cdylib carry a link-time dependency) — and
  `screenshooter_create`, called from init exactly as the C shell
  does (its Rust replacement waits for R2e). Deliberately first: the
  smaller but *deeper* half — it exercises every §3 primitive and
  hardens the wrapper design while the battle-tested C frontend
  still drives startup. R1-specific hazards, named because they
  appear to work in testing and corrupt under a different load
  order: the compositor user-data slot **belongs to the C frontend**
  (`wet_get_config` reaches it via `weston_compositor_get_user_data`,
  `main.c:363`) — the Rust shell must never read, write, or assume
  it, rooting its state in its own `wet_shell_init` allocation; the
  panic handler must be installed by `wet_shell_init` (§3g); the
  borrowed `weston_config *` is read once at init into `ShellConfig`
  and never retained (so R3's removal is a deletion, not a
  refactor); `weston_desktop_api.struct_size` is set from the bound
  headers (risk R-C, and R1 is when both halves coexist). Exit
  criteria: smoke scripts + the e2e shell suites
  (`test_shell_windows.py`, `test_shell_background.py`,
  `test_children.py`) green; **shell unit tests against the mock
  `ShellHost`** (D20) covering focus churn and teardown ordering;
  the **destroy-storm stress test** (output hotplug loops, client
  kill loops, grab start/cancel/destroy races — exercising exactly
  the invalidation paths §3b/§3f introduce) green under valgrind
  (D18: ASAN cannot instrument the hybrid's C half; the pure-Rust
  legs run under nightly-ASAN instead — see PROVENANCE.md).
- **Phase R2 — spawn + frontend in Rust** (slices, each ending
  green — "green" means smoke **and** the full e2e suite): R2a ✅
  *(done — see PROVENANCE.md log)* core
  startup, logging (incl. the va_list shim), headless, **and the §5
  config interface** (`westonite-config`: TOML + clap + `-o`) — the
  one slice where e2e tests change by design: the
  config-discovery/CLI tests in `test_cli.py` are re-specified to
  the new interface in the same PR series, while
  `test_lifecycle.py`/`test_children.py` behaviors (autolaunch —
  via `westonite-spawn` — child handling, shutdown) must pass
  unmodified; the C build stops being the oracle for
  config-interface behavior from this slice on, and remains the
  oracle for everything else. R2b output management + DRM (the
  largest block: heads, hotplug, clone/mirror, color mgmt;
  `test_outputs.py` covers the headless/VNC-reachable subset — DRM
  paths remain hardware-verified), **split in two on landing**:
  *R2b-headless* ✅ *(done — see PROVENANCE.md log)* is the
  e2e-verifiable half — the output policy (`[[output]]`
  mode/scale/transform/off, `--scale`/`--transform`/
  `--no-outputs`/`--refresh-rate`) and all three
  `simple_heads_changed` branches, proven against the headless
  subset of `test_outputs.py`; the DRM-bound remainder
  (clone/mirror-of, color management, the DRM output attributes)
  stays fail-loud and lands with R2c, where VNC gives the e2e
  control plane the reach to verify mirror-of instead of asserting
  it by inspection. R2c remaining backends
  (x11/wayland/rdp/vnc/pipewire — VNC is load-bearing for the whole
  e2e control plane, so it lands first in this slice) plus the
  remoting/pipewire virtual-output plugin loaders.  *R2c-vnc* ✅
  *(done — see PROVENANCE.md log)*: the VNC backend + multi-backend
  loading + the `[keyboard]`/repaint-window compositor init; the
  full e2e suite minus `test_xwayland.py` now runs against
  `westonite-rs` (51 passed).  *R2c-mirror* ✅ *(done — see PROVENANCE.md log)*: the mirror-of
  machinery (remote-head deferral, output-created enable,
  source-mode modeline, resize propagation), e2e-verified with VNC
  mirroring headless.  Remaining in R2c: x11/wayland/rdp/pipewire
  loaders.  *R2c-nested* ✅ *(done — see PROVENANCE.md log)*: x11,
  wayland and pipewire, all three **e2e-tested rather than
  inspection-verified** — the earlier "no container reachability"
  assessment was wrong for these three, and testing them needs no VM
  and no GPU: the wayland backend nests inside a headless westonite,
  the x11 backend runs as a client of the Xwayland *we* spawn (EL10
  ships no X.org server at all, so there is no Xvfb route), and
  pipewire needs only its daemon.  **RDP is dropped as a product
  decision** (2026-07-29): westonite does not ship it, `--backend=rdp`
  says so, and VNC is the supported remote path.  *R2c-drm-harness* ✅
  *(done — see PROVENANCE.md log)*: DRM is the one backend that
  genuinely needs a KMS device, so it gets a test environment before
  it gets a port.  Four throwaway CI probes tried vkms on the runner
  itself and were abandoned as too brittle (2026-07-31); the
  replacement is a VM that ships its own kernel inside our image
  (`containers/Containerfile.drm-vm`, `scripts/drm-vm-test.sh`), so
  the only thing asked of a runner is `/dev/kvm` — and its absence
  merely falls back to TCG.  The C oracle brings a full atomic-KMS
  output up on vkms in it, which is what made the port worth
  starting; **docs/drm-testing.md** records the route, the probe
  findings it replaced, and — load-bearing — what vkms does *not*
  prove (a one-time real-hardware validation is still an R3 gate).
  *R2c-drm* ✅ *(done — see PROVENANCE.md log)*: the DRM backend
  itself — `load_drm_backend`, `drm_heads_changed`, and the
  **layoutput machinery** (`wet_layoutput`/`wet_output`/
  `wet_head_array`, `drm_try_attach`/`drm_try_enable`'s
  attach-then-enable-with-undo, `drm_process_layoutput(s)`,
  `require-outputs`), plus `drm_backend_output_configure` and the
  `clone-of` section resolution the earlier slices had deferred.
  Verified by `tests/e2e/test_backend_drm.py` running against
  `westonite-rs` inside the VM harness, and the `drm-vm` CI job is a
  gate from here on.  **The remoting plugin is dropped as a
  product decision** (2026-07-31), like RDP: `[[remote-output]]` is a
  startup error naming the decision.  It is *not* deprecated upstream
  (weston 15.0.90 still builds it by default, and `screen-share` — the
  thing that IS deprecated, pending removal — westonite never shipped,
  its replacement being the `mirror-of` machinery ported at
  R2c-mirror); westonite simply does not ship GStreamer/RTP output
  streaming, and VNC is its remote path.  **The pipewire virtual-output plugin
  is dropped too** (2026-07-31), on the same grounds.  Weston keeps
  these as two independent build options and they are different
  features — `backend-pipewire` ("PipeWire backend: screencasting via
  PipeWire", **ported** at R2c-nested) versus `pipewire` ("Virtual
  remote output with Pipewire on DRM backend", dropped).  Neither is
  deprecated upstream; both are still default-on in 15.0.90.
  *R2c-color* ✅ *(done — see PROVENANCE.md log)*: the colour
  slice — `[core] color-management`, and per output `icc-profile`,
  `eotf-mode`, `colorimetry-mode`, `color-characteristics` (with C's
  entirely-or-not-at-all group rule) and `allow-hdcp`, which had been
  missing from the config model outright.  `vrr-mode` and `max-cll`
  were removed instead: neither is read anywhere in 14.0.1.
  *R2c-input* ✅ *(done — see PROVENANCE.md log)*: `[libinput]` —
  `configure_input_device` and its `_accel`/`_scroll` helpers, reached
  through the DRM backend's `configure_device` hook (the only backend
  that has one).  Split as usual: parsing and validation in the
  frontend at startup, application per device in the fence.  Where C
  warns per device and leaves a bad `accel-profile`/`scroll-method`/
  `scroll-button`/out-of-range `accel-speed` silently inert, the Rust
  frontend refuses at startup; *capability* gating stays silent, as in
  C, because "this device cannot rotate" is a runtime fact rather than
  a config mistake.  `disable-while-typing` was missing from the config
  model outright.  `touchscreen-calibrator` / `calibration-helper` are
  a different feature (the `weston_touch_calibration` protocol plus a
  `system()` helper, enabled backend-independently) and stay fail-loud.
  The VM harness grew a virtio keyboard and mouse for this, plus a
  udevd run — libinput's udev backend enumerates on the `ID_INPUT`
  property, which only udevd sets — and `-vga none`, since udevd's
  coldplug otherwise modprobes q35's emulated Bochs VGA and weston
  picks that card over vkms.
  *R2f-logging* ✅ *(done — see PROVENANCE.md log)*: the weston-log
  stack — the `"log"` scope, the file subscriber, the flight recorder
  and its Super+D dump binding — plus the two flags that were still
  *warn-and-ignore*, `--logger-scopes` and `--wait-for-debugger`.  With
  those ported there are **no remaining exceptions to the fail-loud
  rule**.  The Rust frontend had been bypassing libweston's log
  machinery entirely (formatting in the shim, writing to a Rust
  `File`), which is why its lines carried no timestamps; routing
  through the scope fixes that and is what makes subscribers possible
  at all.  The log context moved from `Compositor` to the **frontend**,
  since C creates it before the first log line and destroys it after
  the display.
  *R2g* ✅ *(done — see PROVENANCE.md log)*: the last three gaps, found
  by a mechanical audit of every `weston_config_section_get_*` key (67)
  and `WESTON_OPTION_*` flag (48) in main.c against the config model
  and the clap struct.  `--debug` (weston-debug protocol, allow-all
  screenshot authority, and the always-on `proto` protocol dump),
  `[core] output-decorations`, and `[core] use-gl`/`use-pixman` are
  ported; `[libinput] touchscreen-calibrator` is dropped by product
  decision.  **The Rust frontend now reads every config key and CLI
  flag the C frontend does.**  **Virtual-output probe (2026-07-31)**, run with the C
  oracle before porting as usual: both plugins are gated on the DRM
  backend (`load_additional_modules`, main.c:1066), so the VM harness
  is the only place they can run at all.  Remoting *works there today*
  — a `[remote-output]` with `gst-pipeline=` creates, enables and
  advertises a `remoting-<name>` output using only the gstreamer
  elements the guest already has (the sink element must be named
  `sink`; that is how the plugin finds it).  The default `host=`/`port=`
  pipeline needs `rtpbin`/`jpegenc`/`rtpjpegpay`/`udpsink`, which the
  guest lacks.  `[pipewire-output]` **segfaults the compositor** with no
  daemon running (e2e plan §6) and needs `pipewire` in the guest rootfs
  before it can be tested at all. R2d xwayland ✅ *(done — see PROVENANCE.md
  log)*: the `frontend/xwayland.c` glue (module load + plugin API,
  lazy `spawn_xserver` through `westonite-spawn`, `-displayfd`
  readiness watch, SIGCHLD-driven `xserver_exited` respawn,
  destroy-before-compositor teardown) — `test_xwayland.py` now runs
  against `westonite-rs`, so the Rust frontend runs the ENTIRE e2e
  suite and the C oracle covers only the not-yet-ported backend
  loaders. R2e screenshooter/recorder ✅ *(done — see PROVENANCE.md
  log)*: `frontend/weston-screenshooter.c` whole-file port
  (Super+S client spawn + slot gate, Super+R wcap recorder, capture
  authority on `ask_auth`), with `screenshooter_create` ownership
  moved to the frontend as §4 planned for R3 — the Rust build()
  calls it after the shell attaches, same registrations at the same
  startup point.  e2e-verified over VNC QEMU-extended key events;
  capture *completion* is blocked by an RPM-side abort found live
  (e2e plan §6). The C `main.c` stays in-tree, buildable via meson,
  until R2 completes — it is the reference oracle for behavioral
  diffs.
- **Phase R3 — decommission C**: delete C sources + meson, drop the
  `hybrid-r1` bindings from `weston-sys` (the shell now receives its
  typed `ShellConfig` from the Rust frontend and links statically —
  D2), switch spec + CI to cargo-only, RPM install test in pristine
  container, docs rewrite (README incl. the ini→TOML mapping table
  (D11), VENDOR.md provenance model — §8).
- **Phase R4 — idiom & hardening pass**: with parity locked in,
  refactor away remaining C-shaped code, clippy pedantic triage,
  SAFETY comment audit, re-baseline the public-API snapshot.
  (Sanitizers and stress tests are *not* deferred to here — they
  land at R0/R1 per D18, when the primitives are new and wrong
  offsets or early frees are cheapest to catch; Miri cannot cross
  FFI, so ASAN + the fake-C-object tests are the only tools that
  validate the unsafe crates.)

Each phase = one PR series with the same discipline as the port
phases: build container, smoke scripts + e2e suite, VENDOR.md-style
log entries.

## 8. Provenance & upstream-rebase model

VENDOR.md's "verbatim import + discrete patches" model dies with the
C: a translation cannot be rebased with `git diff`. Replacement:

- A `PROVENANCE.md` table mapping every Rust module → source C file
  @ upstream tag (`14.0.1` + P0) at translation time.
- Rebase procedure when EPEL moves (e.g. 14.0.2 → 14.1): diff the
  upstream C between tags, walk the hunks, hand-apply semantic
  equivalents to the mapped Rust modules, log per-hunk disposition.
  More manual than today — this is the *structural cost* of the
  migration and is accepted explicitly (the frontend↔libweston
  skew risk R4 from PLAN.md §9 carries over unchanged).
- `weston-sys` bindings are regenerated per libweston bump
  (scripted), and §3's header-fact table is re-verified then (the
  same tripwire — §6).

## 9. Risks

- **R-A Reentrancy**: handled structurally by §3e (depth-counted
  two-tier dispatch); the residual risk is *ordering* drift where a
  nested C callback becomes a queued event. Mitigation: the e2e
  window-management suite exercises focus churn, grabs, and
  activation races under CI from R1; any observable difference is a
  parity bug (§3e).
- **R-B Behavior drift** in the behaviors config *selects* (not the
  config syntax itself, which is re-specified — §5): mitigated by
  the e2e autolaunch/lifecycle/output/shell tests pinning
  user-visible behavior, and keeping the C build alive as an oracle
  for everything except the config interface until R3.
- **R-C ABI drift on libweston bumps**: backend-config structs use
  `struct_size`/version fields — the builders in `weston` set them
  from the bound headers, and the pkg-config version tripwire (§6)
  catches silent header changes.
- **R-D `pre_exec`/fork-safety**: only async-signal-safe rustix
  calls between fork and exec; confined to `westonite-spawn`, whose
  entire contents are that audit (§2).
- **R-E Toolchain/packaging friction**: EL10 rust-toolset version
  and vendored-crate spec mechanics; de-risked in R0 by building the
  foundation crate straight into an RPM in the existing container
  CI.
- **R-F Future protocol needs**: if a later feature needs a custom
  Wayland global again (e.g. reviving the shell client), the options
  are scanner-generated C + FFI, or wayland-rs's `sys` backend on
  the foreign `wl_display`. Out of scope now; noted so nobody
  assumes the no-scanner simplification is free forever.
- **R-G Config surface completeness**: the §5 re-spec must expose
  *every* option the C code exposes — a silently dropped key is a
  regression the type system can't see. Mitigation: a checklist
  mapping every `[section]` key and CLI option from the two
  capability inventories to a `Config` field, reviewed as part of
  the R2a PR series; `deny_unknown_fields` catches the reverse
  direction (phantom keys) for free.
- **R-H Handle soundness**: `Copy` pointer handles would let the
  `forbid(unsafe_code)` crates author use-after-free. Handles are
  generational ids resolved through the wrapper's registry (§3b);
  the opaque-id and public-API fence rules (§2) make the property
  unforgeable by callers; the destroy-storm test (D18) exercises it.
- **R-I Wrapper-internal discipline**: with all unsoundness confined
  to `weston`, that crate's internal rules (no registry borrow held
  across FFI, drain only at depth zero, pinned kind-3/4 allocations)
  are now the load-bearing ones. Mitigation: the crate is small by
  design, the callback inventory is its audit index, the
  fake-C-object tests pin each primitive, and ASAN runs from R0.

## 10. Intentional divergences from C behavior (D6 record)

Cosmetic text (`--help` wording, log phrasing) may differ where Rust
idioms make it natural; the configuration interface is re-specified
wholesale (§5). Beyond those, the known intentional divergences —
each must be listed in its porting PR, and this list is the running
record:

1. `get_output_work_area()` on a stale output: C asserts (aborts
   under `!NDEBUG`, reads through the pointer under `NDEBUG`); Rust
   logs and returns the zeroed rectangle (§3b).
2. Reentrant callback shapes run as queued events at the same stack
   boundary instead of nested (§3e) — must be e2e-invisible; any
   observable difference is a parity bug, not a divergence.
3. `has_keyboard_focused_child` at child depth ≥ 2: C behavior is UB
   (the `bool **` bug, §4); Rust implements the evident intent.
4. Screenshooter initialization moves from shell to frontend at R2e
   (§4) — behavior-identical, structural only.
5. The stale-resolution policy replaces roughly a dozen scattered
   C null-check/cleanup sites with one central check; a *missed*
   cleanup that C would have crashed on becomes a logged no-op
   (§3b). New sites appearing in review with an `unwrap` are build
   failures, not judgement calls.

## 11. Decision log

Scoping decisions (2026-07 plan review):

- **D1 — Port everything 1:1, no pre-trim.** The full current
  capability surface (all seven backends' config, clone/mirror,
  color management, `--backends`, screenshooter, idle-time plumbing)
  is translated as-is. The drop-assessment questions in
  `docs/frontend-capabilities.md` stay open independently — trims
  can still happen later, in Rust. *(Scope note: "1:1" applies to
  capabilities, not to the configuration interface, which D9–D12
  re-specify — every option survives, its spelling changes.)*
- **D2 — Shell linkage: cdylib during migration, static at R3.**
  `--shell` swapping of our own shell does not survive (document in
  README at R3).  The hybrid-phase cdylib is produced by the R1-only
  `westonite-shell-plugin` packaging crate (§2), deleted at R3.
  - **Amended 2026-08-01 — `modules=` dlopen does *not* survive
    either.**  D2 originally kept third-party `wet_module_init`
    loading.  Reversed by product decision: nothing westonite ships
    uses it (the shell is linked in, and the two plugins that loaded
    this way — remoting and pipewire-output — are themselves dropped),
    and keeping it would mean a stable `wet_module_init` ABI, a dlopen
    path through the fence for arbitrary C, and resolving the
    `wet_get_config` contract D9 deliberately ends at R3.  `--modules`
    and `[core] modules` are now a fail-loud refusal in their own
    words.  Not permanent — revisit if a real plugin need appears —
    but out of scope for R3, which it no longer blocks.
- **D3 — superseded by D9–D12.** The ini/`weston_config` machinery
  is not ported; a minimal binding exists only during R1 (§2, §7)
  and is deleted at R3. The known 14.0.1 `binding-modifier none`
  parser limitation becomes moot.
- **D4 — Port order: shell first** (§7 R1).
- **D5 — Bindings committed**, regenerated by script on libweston
  bumps (§6).
- **D6 — Parity bar: behavioral, except the config interface.**
  Divergence record lives in §10.
- **D7 — Maintenance-layer feature deferred indefinitely.** Not a
  milestone of this plan; rescheduled (if at all) only after R3/R4.
- **D8 — Crate policy: anything vendorable, per-crate approval**
  (§6).
- **D9 — Config re-specified as typed TOML, kebab-case keys** (§5).
- **D10 — CLI: clap flags + dotted `-o` overrides** (§5).
- **D11 — ini migration: docs only** (§5).
- **D12 — `WESTON_CONFIG_FILE` export dropped** (§5).

Memory-model decisions (2026-07-24/25 review; rejected alternatives
recorded so they are not silently revisited; A1–A5 are the
finalization amendments, each verified against the sources):

- **D13 — Handle model: generational ids + slot table; `Option`
  everywhere, no `unwrap`/`expect` in safe crates** (§3b).
  *Rejected*: `Copy` pointer newtypes (unsound); `Rc`/`Weak`
  upgrade-per-use and the invalidatable `Rc`-cell (equally sound,
  wrapper-internal — retained as a fallback ergonomics option, not
  a safety question); branded/`GhostCell` lifetimes (viral); shadow
  state (duplicated state, sync risk); `expect`-where-provable (a
  panic kills the session; teardown-path invariants rot).
  *Amendment A2*: uniform ids at the shell boundary; the reviewed
  "live borrowed subject" handle kind stays wrapper-internal.
- **D14 — Reentrancy: two-tier dispatch; no interior mutability in
  safe crates; named sync tier with per-entry audit** (§3e).
  *Rejected*: `RefCell`-discipline-by-review in the shell (abort
  failure mode, discipline in the most-edited code); sync-in/
  deferred-out middle option (leaves per-call-site reentrancy
  judgement). *Amendments A3/A4*: sync-tier borrow rule with
  recorded non-reentrancy proofs; depth-counted drain-at-edge
  replacing "drain with no C frame live" (which reordered
  observably and left non-trampoline outbound calls undrained);
  destroy notifications split into synchronous invalidation +
  deferred policy with eagerly-captured payload; queue discarded at
  teardown.
- **D15 — spawn unsafety isolated in `westonite-spawn`** (§2).
  *Rejected*: in `weston` (mixes fork-safety and FFI audits); an
  `allow(unsafe_code)` carve-out inside a safe crate. *Amendment
  A1*: `westonite-shared` dissolved (helpers → `westonite-spawn`,
  socketpair inlined); six crates total, three safe.
- **D16 — Panic policy: `panic = "unwind"` + `catch_unwind` at every
  trampoline + `weston_log` + deliberate `abort()`** (§3g).
  *Rejected*: `panic = "abort"` + `set_hook` (loses per-callback
  context in release, where it matters).
- **D17 — Fence enforcement: all four structural checks + the
  no-panic lints** (§2; the `cargo public-api` snapshot remains a
  known gap, review-enforced until it lands). *Rejected*: any
  subset; the original "CI grep".
- **D18 — Validation at R0/R1, not R4: sanitizer smoke
  (nightly-ASAN for pure-Rust legs, valgrind for the hybrid — see
  §7), destroy-storm stress, fake-C-object unit tests, registry
  `debug_assert`s on in CI** (§7). *Rejected*: sanitizers as
  optional R4 hardening.  (UBSAN was in the original wording but is
  not run: Rust has no stable UBSAN story and the C half is covered
  by valgrind.)
- **D19 — Boundary cost rules** (§3i): cached strings, no per-event
  allocations, scope-guarded lazy log formatting — where per-event
  cost actually is (100–1000× a handle check).
- **D20 — Shell testability via the `ShellHost` trait + mock**
  (§3e). *Rejected*: a partial-subset trait (boundary judged per
  call site); calling `weston` directly (e2e-only verification of
  the deepest logic). *Amendment A5*: the trait carries ids + plain
  data only, matching A2.
- **D21 — Trampolines reach `Ctx` through one thread-local slot**
  (§3j; 2026-07-25 implementability review). *Rejected*: a `Ctx`
  pointer captured in each `Inner` box — the no-user-data
  registrations (the log handlers) need a thread-local anyway, so
  per-box pointers would mean two mechanisms where one suffices.
