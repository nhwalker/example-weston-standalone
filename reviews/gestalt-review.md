# Gestalt review: the Rust port as a whole

Reviewed at `main` = `4fc7ae6` (post-#49, all prior review-series findings
closed), 2026-08-02. Complements the PR-by-PR series (`reviews/pr-0*.md`):
where those reviewed each slice as of its merge, this review looks at the
finished surface *across* slices — the soundness fence as actually shipped,
cross-cutting drift, the wiring gaps no single PR could see, and whether the
"clear, maintainable, thoroughly tested, safe" claim survives adversarial
re-verification. Ground rules as in `reviews/README.md`; finding IDs are
`GES-Cx` (correctness), `GES-Tx` (testing), `GES-Sx` (style/maintenance).

**Nothing in the docs was trusted without proof.** Every finding below was
verified against the code at the cited lines, and C-parity claims against
the C sources (this repo's trimmed `frontend/`+`desktop-shell/`, and
upstream weston at tag `14.0.1` — whose commit, `61f2248de`, matches
VENDOR.md's provenance claim exactly; verified). Where a claim could be
tested by execution, it was (see §1).

## Verdict

The port is of genuinely high quality — the memory-model design (generational
ids, two-tier dispatch, panic barrier, listener discipline) is coherent,
consistently executed, and holds up under hostile reading; the paper trail is
honest (every "fixed in #NN" claim sampled was true); and the test pipeline
is far stronger than typical for a project this size. **It is not, however,
finished to its own stated standard.** This review finds:

* **1 blocker**: the `compositor->exit` hook is never installed — a NULL
  function-pointer call reachable from ordinary use of three shipped
  backends (GES-C1).
* **2 findings that falsify the plan's headline claims as currently
  written**: an exit-code parity gap (GES-C2) and two soundness holes in
  the "sound for arbitrary safe callers" fence (GES-C3, GES-C4).
* A cluster of config-surface correctness bugs that violate the port's own
  fail-loud rule (GES-C5, C8, C14, C15).
* Build-system land mines around the bindings tripwire (GES-C6, C16).
* Real test gaps concentrated exactly where the risk is (GES-T1–T9).

None of this contradicts the overall judgment: the hard parts were done
carefully, and the defects found are almost all in the *wiring and
bookkeeping* around them. But for a safety-critical system, "the hard part
is right" is not the bar; the bar is that we can prove the whole thing, and
today we cannot — see §5 for what remains unprovable from this repo alone.

## 1. What was proven by execution (not just read)

This review was run in a sandbox without the EPEL build container, so
libweston **14.0.1 was built from the vendored source tag** and the
workspace exercised against it:

* `cargo build --locked --workspace` — clean.
* All **69 unit tests pass** (26 `weston --features testsupport`,
  9 `westonite-shell`, 27 `westonite-config`, 6 `westonite-spawn`,
  1 `westonite`).
* `cargo clippy --locked --workspace --all-targets --features
  weston/testsupport -- -D warnings` — clean. `cargo fmt --check` — clean.
* `scripts/rust-fence-check.sh` — passes (fences 1–2; fence 3 skipped
  locally, forced in CI via `FENCE_CHECK_BINDINGS=1`, verified wired at
  `ci.yml:94`).
* `r0-smoke` **and the real `westonite-rs --backend=headless`** boot a live
  compositor, bind the socket after the heads flush, and exit 0 on SIGTERM.
* The same headless bring-up/teardown cycle **under valgrind: 0 errors,
  0 definite leaks**.

Caveats: this is a from-source libweston on Ubuntu, not the EPEL RPM; the
e2e suite and DRM VM were not run here (no container). Those legs were
audited as code (§4), not executed.

## 2. Correctness findings

### GES-C1 — blocker: `compositor->exit` is never installed; NULL call on every libweston-initiated exit

`CompositorBuilder::build()` never writes `(*compositor).exit` (no
assignment anywhere in `crates/weston/` — verified by exhaustive grep; the
only mention of the field is its bindgen declaration). libweston zallocs
the struct and `weston_compositor_exit()` calls the hook **unconditionally**
(`libweston/compositor.c:10156`, tag 14.0.1: `compositor->exit(compositor);`
— no NULL check). C installs it at `frontend/main.c:4682`
(`wet.compositor->exit = handle_exit`).

Reachable callers in backends this port loads:

* `backend-x11/x11.c:1277` — last X11 output window closed, i.e. **the
  normal way a user quits a nested session**;
* `backend-wayland/wayland.c:1205/2733/2748` — parent compositor gone or
  socket error;
* `backend-drm/drm.c:165` — fatal DRM error, via
  `weston_compositor_exit_with_code`.

Failure scenario: `westonite-rs --backend=x11`, click the window's close
button → call through NULL → SIGSEGV with no teardown, where C exits
cleanly. No test catches this because the e2e suite drives headless/VNC and
terminates via signals (`wl_display_terminate` path), never via
`weston_compositor_exit`.

Fix: one `extern "C"` handler (panic-barrier-wrapped, `wl_display_terminate`)
assigned in `build()`, plus an e2e or unit leg that exercises a
backend-initiated exit. Found independently by two reviewers on different
scopes; verified at all three layers (no assignment / unconditional call /
reachable callers).

### GES-C2 — major: compositor exit codes are silently discarded

`Compositor.exit_code` is set to 0 once (`compositor.rs:570`) and returned
verbatim from `run()` (`compositor.rs:1506`); nothing ever reads
`(*compositor).exit_code` back. C does: `ret = wet.compositor->exit_code`
(`frontend/main.c:4785`), fed by `weston_compositor_exit_with_code`
(e.g. DRM fatal → `EXIT_FAILURE`). Even with GES-C1 fixed, a DRM session
error that C reports as exit 1 exits 0 here. This directly falsifies the
plan's parity claim "same runtime behavior and exit codes"
(`docs/rust-migration-plan.md`, goal 4). Fix: copy the field back after
`wl_display_run`; the binding exists.

### GES-C3 — major (unsoundness): `activate_input` dereferences a view pointer across a drain that can free it

`host.rs:339–362`: the target `view` pointer is resolved, then
`with_depth(|| weston_view_activate_input(...))` runs, then
`view.as_ref().surface` is dereferenced (`host.rs:361`) and handed to
`track_surface`. When `activate_input` is called from **top level** (safe
code outside any trampoline/drain — which the public `Ctx` API permits),
the `with_depth` exit at depth 0 **drains the deferred queue**
(`ctx.rs:335–344`); a drained handler may legitimately call
`destroy_surface_state(id)` (safe API → `weston_view_destroy`,
`desktop.rs:173–183`) on that very surface. Control then returns and reads
`(*view).surface` from freed memory — and attaches a destroy listener to
whatever it finds.

On the dispatch paths (inside a trampoline: depth ≥ 1; mid-drain:
`draining` set) the drain is suppressed, so today's shell/frontend do not
hit this. But the crate's own bar is *sound for arbitrary safe callers in
any order* (`weston/src/lib.rs:3–7`), and this is a safe-code-reachable
UAF by that standard. The SAFETY comment at `host.rs:360` ("the view is
live inside this activation frame") is exactly the claim the drain breaks.
Fix: re-resolve through the registry after the FFI call (return `None` if
the object died mid-drain) instead of trusting the pre-call pointer.

### GES-C4 — major (unsoundness): `shell_init` is a safe `pub fn` that dereferences an arbitrary raw pointer

`weston::shell_init::shell_init(ec_opaque: *mut c_void, ...) -> bool`
(`shell_init.rs:358`) is **safe** and casts-then-dereferences its argument
(`shell_init.rs:383` onward: listener attachment through the pointer).
Creating an arbitrary `*mut c_void` is safe Rust, and the
`#![forbid(unsafe_code)]` crate `westonite` enables the `hybrid-r1` feature
that exposes this module (`crates/westonite/Cargo.toml`). So a safe crate
in the shipped build graph can cause UB with no `unsafe` anywhere —
violating plan goal 1 and the fence crate's own Cargo.toml description
("sound for arbitrary safe callers"). The fix costs nothing: mark it
`unsafe fn` with a `# Safety` contract (its only legitimate caller,
`westonite-shell-plugin::wet_shell_init`, is already an
`unsafe extern "C" fn` in an unsafe-classified crate), or split the
feature as the Cargo.toml comment already anticipates. Note this is
precisely the bug class the un-wired `cargo-public-api` gate (GES-T9)
exists to catch.

### GES-C5 — major: `[core] require-input` is parsed, never applied, and the effective default is inverted

C sets `wet.compositor->require_input` from config with **default true**
(`frontend/main.c:4641-4642`); zalloc leaves the field false, and the Rust
port's only write is `--continue-without-input` clearing it
(`compositor.rs:1329–1334`) — clearing an already-false flag. The parsed
`Settings.require_input` (`westonite-config/src/resolve.rs:106`, model at
`model.rs:191`) is consumed by nothing. Consequences: a DRM machine with
zero input devices *starts with a warning* where C refuses to start
(`libweston/libinput-seat.c:308–314`); `require-input = true` in config is
a silent no-op; `--continue-without-input` is a de-facto no-op. The key is
advertised in `westonite.toml.example:27`. This is the exact failure mode
the frontend's own fail-loud rule (`main.rs:325–328`) exists to prevent.

### GES-C6 — major (build system): the bindings version tripwire is bypassed by build-script caching

`weston-sys/build.rs:77–81` emits `cargo:rerun-if-changed=` for only the
shim files and `bindings.rs`. Per cargo semantics, emitting *any*
`rerun-if-changed` replaces the default rerun policy, and the pkg-config
crate adds only `rerun-if-env-changed` lines — the `.pc` files and header
directories are **not** watched. So with a warm `target/`, upgrading the
installed weston RPM in place reruns nothing: the modversion assert
(`build.rs:38–44`) and the `have_listenfd` probe never re-execute, and
stale bindings are silently linked against new headers — exactly the
struct-layout-UB scenario (risk R-C) the tripwire exists for, and a direct
contradiction of `weston-sys/src/lib.rs:9-11` ("build.rs enforces...") and
`regen-bindings.sh`'s "at every build". CI escapes because images are
rebuilt per run; a persistent dev container does not. Fix: emit
`rerun-if-changed` for the resolved `.pc` path and include dir, or drop
the lines entirely (the script is cheap).

### GES-C7 — moderate: `build()`'s color-manager failure path skips all cleanup

`compositor.rs:545–549` returns `Err(ColorManager)` before `comp` exists,
so no Drop runs: unlike the adjacent xkb failure path
(`compositor.rs:514–524`, which destroys compositor + display and calls
`ctx.teardown()`), this path leaks both C objects **and leaves the
thread-local `Ctx` installed** — a subsequent `Ctx::new` on the thread
trips the debug assert (`ctx.rs:218`) or silently steals the slot in
release. Reachable by `color_management(true)` with a broken color-manager
install.

### GES-C8 — moderate: `[keyboard] numlock-on` is parsed and silently dropped

Model field `model.rs:278`, advertised in `westonite.toml.example:63` and
`docs/frontend-capabilities.md:89`; zero consumers in the workspace
(verified by grep). C applies it after module load
(`frontend/main.c:4740–4752`). Violates the fail-loud rule.

### GES-C9 — moderate: mirror-source exclusion checks VNC only; the comment claims C's full RDP/VNC/PIPEWIRE set

C ignores output-created events from **all** remote backends
(`frontend/main.c:2617–2623`: RDP, VNC, PIPEWIRE). The Rust
`mirror_on_output_created` (`compositor.rs:1887–1894`) tests only
`BackendKind::Vnc` — while its own comment states the three-backend rule.
With `vnc + pipewire` both loaded (the builder allows it), a `mirror-of`
naming a pipewire output takes a path C refuses. The frontend-side section
restriction (`main.rs:537–553`) happens to mask this today, but that
coupling lives two crates away from the comment that misdescribes the code.

### GES-C10 — moderate: a `Pong` whose first listed surface died strands busy-cursor grabs

`westonite-shell/src/lib.rs:143–147` takes `surfaces.first()` and calls
`end_busy_grabs_for_client_of(*first)`; the fence side early-returns if
that one id no longer resolves (`grab.rs:824–827`). C matches on the
*client*, which is alive at pong time (`shell.c:1498–1527`), so it always
ends the grab. If the first-listed surface died before the deferred event
drained (others still live), or all died, the busy cursor persists until
the next pointer-focus recomputation — indefinitely on an idle pointer.
Fix: iterate the vector until an id resolves.

### GES-C11 — moderate: `WAYLAND_SERVER_SOCKET` single-client mode silently dropped

C (`frontend/main.c:4686–4708`): inherited-fd mode, "Running with single
client", terminate on primary-client destroy. The Rust port has no env
check, no refusal, no diagnostic (verified: zero mentions in `crates/`,
`docs/`, `tests/`, `reviews/`); it is absent from the §10 divergence
record. A launcher passing the fd gets a compositor that ignores it and
binds a fresh socket; the intended client never connects.

### GES-C12 — moderate: default socket naming and `WAYLAND_DISPLAY` semantics diverge, undocumented

C loops `wayland-1..32` and `setenv("WAYLAND_DISPLAY", ...)` in the
compositor's own environment (`frontend/main.c:950–955`); Rust uses
`wl_display_add_socket_auto` (from `wayland-0` up, `compositor.rs:804`) and
sets `WAYLAND_DISPLAY` only on the autolaunch child (`main.rs:985–987`).
Scripts keyed on C's `wayland-1` default or on the compositor environ
diverge silently. Not in §10.

### GES-C13 — moderate: the child-spawn `seteuid(getuid())` privilege drop is missing

C's `wet_client_launch` child block runs `seteuid(getuid())` and
`_exit`s on failure ("Do not launch clients with wrong euid",
`frontend/main.c:~469`) — and this path serves **all** spawned clients,
including Xwayland and the screenshooter. `westonite-spawn`'s `pre_exec`
(`lib.rs:198–220`) does sigmask/setsid/CLOEXEC only. The crate doc frames
`seteuid` as needed only "when the helper-client path lands"
(`lib.rs:20–24`) — wrong, since `xwayland.rs` and `screenshooter.rs`
already spawn through this crate. Benign while the binary is never setuid
(the RPM installs none), but it is a security-relevant narrowing absent
from §10.

### GES-C14 — moderate: silent u32→i32 truncation-to-default on config values crossing the fence

Six sites use `and_then(|v| i32::try_from(v).ok())`, mapping out-of-range
values to `None` → **the default**, with no diagnostic: repeat-rate/-delay
(`main.rs:143–144`), `[vnc] refresh-rate` (`:174–175`), `--output-count`
(x11 `:183–186`, wayland `:192–195`), `[pipewire] num-outputs`
(`:201–204`). `-o keyboard.repeat-rate=4000000000` starts cleanly at rate
40. Violates the fail-loud contract.

### GES-C15 — moderate: phantom config keys — parsed, dead, no refusal

* `[[output]] position` (`model.rs:379`): no reader in Rust **or** in the
  C frontend. Dead surface presented as live.
* Per-output `pixman-shadow` (`model.rs:387`): C reads it from `[core]`
  only (`frontend/main.c:3418`); the per-output copy reaches no `OutputRule`.
* `[shell] client` (`model.rs:259`, resolved at `resolve.rs:454`): no
  consumer, no refusal.
* The entire `[rdp]` section (nine keys, `model.rs:335–346`) parses and
  resolves while the RDP backend is permanently refused (`main.rs:313–318`)
  — and `westonite.toml.example:140–141` advertises it. This is the same
  silent-no-op shape the `[[remote-output]]` refusal comment condemns.

### GES-C16 — moderate: the bindings gate races EPEL, and release respins pass the tripwire

The modversion assert compares version *strings* only: an EPEL **release
respin** (14.0.1-2 with a backported header change) passes. The one
content-level check (fence 3, regenerate-and-diff) runs only in the `rust`
CI job; the `build-and-test` job that produces and install-tests the
shipped RPM has no drift gate, and each of the four CI jobs builds its
container image independently (`ci.yml:17/82/130/171`), so an EPEL push
landing mid-run can hand the RPM job different headers than the ones
fence 3 validated. `Containerfile.build` pins no weston-devel version.

### GES-C17 — moderate (latent, stated-invariant violation): `resize_finish` holds a `RefCell` borrow across FFI

`grab.rs:313–320`: `if let Some(ds) = ctx.inner.desktop_surfaces.borrow()
.resolve(...)` — the scrutinee temporary (the `Ref`) lives through the
success arm, so the shared borrow is held across
`weston_desktop_surface_set_resizing`/`set_size`. This is the one outlier
from the rule `ctx.rs:4–8` states ("no borrow of any field is held across
an FFI call"); every sibling site uses the statement-scoped `let … else`
form. Harmless with today's libweston (those calls only schedule idle
configures) but the invariant exists precisely so nobody has to know that.
Fix: bind the `NonNull` out of the borrow first.

### GES-C18 — moderate (land mine): `add_destroy_listener_once` dedupe is keyed on the shared trampoline

C dedupes by handler *function* (`compositor.c:10064`:
`wl_signal_get(&destroy_signal, destroy_handler)`), and every wrapper
`Listener` shares the single `trampoline` (`listener.rs:50/93`). So the
"once" guard (`shell_init.rs:426–434/501–507`) means "no other wrapper
listener anywhere on the compositor destroy signal" — an invariant that is
true today, enforced nowhere, and documented nowhere. The failure mode is
nasty: a future listener on that signal makes the hybrid path return
"already initialized" success **without wiring the shell**, and the native
path fail spuriously.

### GES-C19 — minor: a panic escaping the drain wedges the dispatch machinery

If the app's `handle` panics inside a drain initiated from a top-level safe
call (trampoline-initiated drains abort via the barrier), the unwind skips
`draining.set(false)`/the depth decrement and drops the taken app box
(`ctx.rs:335–376`). Any caller that catches the panic owns a permanently
wedged `Ctx` (drains disabled, app gone). Not UB; cheap to fix with drop
guards, or document "no catch_unwind above the fence" as a contract.

### GES-C20 — minor: `change_focus_count` crossing test diverges from C for negative counts

Rust syncs on `(before==0) != (after==0)` (`westonite-shell/lib.rs:528–540`);
C syncs only on `--count == 0` / `count++ == 0` (`shell.c:1005–1019`) —
they differ on 0→-1 and -1→0. Traced invariants suggest negative counts
are unreachable, but that's neither tested nor documented as intentional
dead-code divergence.

### GES-C21 — minor: assorted CLI/config divergences (each verified, none in §10)

C short flags `-i/-l/-f` dropped; repeated string flags error where C
last-wins; bare positional autolaunch accepted where C fatals
(`cli.rs:190–191` vs `main.c:4754+`); `[autolaunch] path = ""` fatals where
C skips; `wait_for_debugger` SIGSTOPs before signal sources exist
(`main.rs:104` vs `main.c:4544/4589` — SIGTERM in the stopped window takes
default disposition); `-o` structural clobbers (an override like
`-o core.0.x=1` silently replaces a file table before erroring
misleadingly, `overrides.rs:41–68`); the `-o shell.background-color=12345678`
bare-integer-is-decimal footgun that `config-migration.md:40` actively
encourages (the hex parser itself is correct — PR19-C6 re-verified fixed,
including 0x/overflow/negative edge cases); exit codes >255 mapped to 1 vs
C's mod-256.

## 3. Testing findings

The pipeline's shape is excellent (see §6), but coverage is lopsided in
exactly the wrong direction: the *safe* crates are well-tested while the
most dangerous unsafe surfaces ride on end-to-end coverage alone.

### GES-T1 — major: `grab.rs` (935 lines) has zero unit tests

The most dangerous shapes in the codebase — pinned self-referential grab
boxes with embedded C vtables, superseded-grab retirement
(`grab.rs:571–588`), the dangling-pointer-guarded busy-grab snapshot walk
(`grab.rs:846–897`) — have no unit tests; e2e covers pointer move/resize/
busy-click only, and the valgrind destroy-storm starts no grabs
(`wtest-client` is not run `--interactive` in `rust-stress-test.sh`).
The testsupport harness (a real `wl_signal` fake) would support grab-shaped
tests; it is used only for listener list surgery today.

### GES-T2 — major: touch input has zero coverage of any kind

`TouchBinding`, `TouchFocusView`, and the whole touch move grab
(`grab.rs:420–484, 700–731`) are executed by no unit test, no e2e test, no
smoke leg. Verified by test-name inventory across the workspace and
`tests/e2e/`.

### GES-T3 — major: the track/untrack refcount contract is unit-tested only against a mock restatement of itself

`westonite-shell`'s tests assert against the mock's re-implementation
(`tests.rs:31–42, 93–99`); the real `track_surface`/`untrack_surface`
(`compositor.rs:2586–2665`) is exercised only transitively (one e2e
focus-survivor test + valgrind storms). A wl_signal-level fake test would
be cheap — the harness exists.

### GES-T4 — major: transient/child-surface policy has no compositor-level test

The one place the port deliberately fixes C UB (the `bool**` bug — claim
verified real in both trimmed and upstream shell.c) rides on mock unit
tests only. `test_children.py` is child *processes*; the e2e plan itself
records the deferral (E3).

### GES-T5/T6 — moderate: output hotplug/move/gone and DRM multi-head grouping untested

No test creates, moves, or destroys an output at runtime
(`OutputGone`/`OutputMoved`/`reposition_all`, curtain moves). vkms's single
connector means `layoutput.rs`'s 588 lines of head-grouping/clone logic are
unreachable by the DRM VM leg — acknowledged by `drm-testing.md`, still a
gap until the R3 real-hardware validation.

### GES-T7 — moderate: CI never runs 2 of the 70 unit tests

`rust-smoke.sh:33–36` tests four crates; `cargo test -p westonite`
(`simple_mode_grammar`) and `-p weston-sys` never run anywhere in CI. The
weston-sys one is the *documented* safety net for the "--as-needed dropped
the library" incident its own comment describes — an advertised net that is
currently dead code.

### GES-T8 — minor: ASAN covers startup/shutdown + autolaunch only, not the e2e suite

### GES-T9 — minor (admitted): the `cargo-public-api` snapshot gate is not wired

`rust-fence-check.sh:13–14` says so honestly. GES-C4 is exactly the bug
class it would catch; wire it.

## 4. Style / maintenance findings

### GES-S1 — the fence's two central audits are manual disciplines

* The **entry-point audit** (every `extern "C"` body wrapped in
  `panic_barrier::guard`/`with_depth`, ~56 functions) lives in a comment —
  which itself admits its methodology "missed two of these"
  (`compositor.rs:1495–1504`) and omits one current entry point
  (`protocol_log_fn`, `debug.rs:62`) from the exemption list.
* The **A3 non-reentrancy proofs** live in `docs/callback-inventory.md`,
  which has already drifted once (the PR17-C2 correction note is candid
  about this).

Both are mechanizable at the grep/syn level (e.g., fence 4 in
`rust-fence-check.sh`: every `unsafe extern "C" fn` in `crates/weston/src`
must contain `guard(`/`with_depth(`/`with_ctx(` or carry an
`// audit-exempt:` marker). Given the fence's soundness argument *rests* on
these disciplines, mechanizing them is the highest-leverage maintenance
investment available.

### GES-S2 — the §10 divergence record materially undersells the actual divergence set

Verified-real divergences missing from `rust-migration-plan.md` §10:
shell-before-flush ordering (documented only at `with_shell`); GES-C11,
C12, C13; multi-backend CLI geometry honored where C drops it; clap exit 2
vs C exit 1 on bad flags; exit-code >255 mapping; mirror-from-headless
allowed where C aborts (only in `config-migration.md`); the spawn
env-string narrowings (documented in code, not §10). §10's preamble says it
is "the running record" — it isn't, currently. Either consolidate or
re-point §10 at the per-file records.

### GES-S3 — comment/doc drift (each item verified)

* `westonite-shell/lib.rs:230–234` misstates C's shutdown teardown order
  (C destroys desktop surfaces via `weston_desktop_destroy` *before* the
  layer walk; the layer list is empty by then).
* `events.rs:134` documents `SessionActivated` as sync; the wiring enqueues
  it (`shell_init.rs:635–645`).
* `output_policy.rs:200–210`: `decide_pipewire` carries `decide_vnc`'s doc
  comment; `decide_vnc` has none.
* Citation drift (claims true, line numbers stale): `vnc.c:1260`→1196,
  `compositor.c:3405`→3461, `output-capture.c:801`→699, `input.c:3954`→3949.
* README says "~45 tests"; the e2e suite has **115**. `ci.yml:100`'s step
  name says "e2e subset (R2a…)"; the script runs the entire suite (its own
  header says so). e2e-test-plan says test_shell_windows has 10 tests; 11.
* `config-migration.md`: claims "most entries change section syntax only"
  while omitting real underscore→kebab renames (`keymap_rules`,
  `icc_profile`, `allow_hdcp`, `touchscreen_calibrator`); line 84 calls
  clone-of/colour attributes "not ported yet" though DRM-ported;
  `[vnc]/[rdp] port` are undocumented re-spec additions.
* `westonite-spawn/lib.rs:20–24`'s seteuid rationale is wrong (GES-C13).

### GES-S4 — six hardcoded windowed-API name literals

`compositor.rs:937/1145/1219/2085–2087` hardcode
`c"weston_windowed_output_api_*_v2"` while bindgen exports the header
constants (`WESTON_WINDOWED_OUTPUT_API_NAME_*`, `bindings.rs:203–206`).
A v3 header bump becomes a runtime null instead of a compile error.
`xwayland.rs:110` shows the right pattern.

### GES-S5 — small items

* `rustix` is declared in `[workspace.dependencies]` "for the spawn crate"
  and used by **no crate** (spawn uses libc). Dead dep + stale comment.
* No `rust-version` (MSRV) in the workspace despite edition 2024 and a
  distro-moving toolchain.
* Fence-check holes: `cargo metadata` without `--all-features` (a
  feature-gated dep on weston-sys would evade fence 1); fence crates
  matched by *name* not workspace identity; fence 2 greps one root file
  per crate.
* `shim.c:79–84` labels a `vsnprintf` encoding error "Out of memory"
  (copied from the C vasprintf path where it was true).
* `curtain_get_label` returns copied-length vs C's would-be-length —
  harmless, but the file documents its other divergences and not this one.
* The XOR listener-key scheme (`ec ^ 1 … ^ 6`, shell_init.rs) is sound
  (misaligned keys can't collide with object addresses) but uncommented.
* `layoutput.rs:132`'s skip-index-0 attach quirk is bug-for-bug C parity
  (`drm_try_attach` starts at 1) with no comment, unlike the project's
  other reproduced quirks.

## 5. What this review could NOT prove (and nobody should claim)

* **Branch protection.** Whether the `rust` CI job (fences, clippy, unit
  tests) is a *required* check is invisible from the repo. If it is
  optional, every fence is advisory. Worth one line in CONTRIBUTING/README
  stating the required-check set.
* **EPEL header fidelity.** The bindings spot-check (≈15 load-bearing
  declarations — structs, enums, callback typedefs, all six backend config
  version constants — diffed against tag 14.0.1: all match; no bitfields
  bound anywhere, the classic bindgen hazard absent) was against the
  *upstream tag*, the best proxy available here. Whether EPEL carries
  header-touching downstream patches is only provable in the container
  (fence 3 does prove it in CI — modulo GES-C16's race).
* **Real-hardware DRM, VT switching, touch hardware.** vkms proves what it
  proves; `drm-testing.md` is honest about the rest. The R3 gate stands.
* **The e2e suite in this sandbox** (needs the container stack). Audited
  as code: the harness design — per-test compositor, poll-don't-sleep,
  every test doubling as a clean-shutdown assertion, pixel-exact flat-color
  scenes — is genuinely good.

## 6. What held up under attack (the positive findings)

For balance, and because "we better be able to prove it" cuts both ways —
the following were each adversarially checked and survived:

* **The generational-id registry** (`registry.rs`): generation-0 never
  minted, wrap handled, recycle tested; all `insert` sites guarded against
  double-registration (`id_of` pre-checks / `SEAT_RECS` key check).
* **The listener discipline** (`listener.rs`): repr(C) offset-0 recovery,
  oneshot self-detach *before* the owner's list head dies, detach-on-drop,
  `mark_attached` + the drop-time assert; every `raw_ptr()` attach site in
  the tree carries `mark_attached()` (verified by grep).
* **The teardown ordering** (`Compositor::drop` + `Ctx::teardown` +
  GRAVEYARD): idempotent, C-frame-aware, and the parked boxes have no
  FFI-touching Drop (verified: only `Listener`/`LogContext`/`Compositor`
  implement Drop). The load-bearing C claim behind the pointer-guard design
  — `weston_seat_release` frees the pointer *before* emitting the seat
  destroy signal — verified exact at `input.c:4360–4376`.
* **The panic barrier**: every trampoline body guarded; the plugin shim
  adds a boundary backstop; `panic = "unwind"` kept in all profiles as D16
  requires.
* **The dispatch design's A3 machinery**: the loud-in-debug fallback exists
  and is tested (loud / mid-drain-quiet / pre-app-quiet all pinned).
* **`westonite-spawn`'s pre_exec** is genuinely async-signal-safe (no
  allocation, no locks, alloc-free error path, pre-forked Vec) and its fd
  lifecycle is leak-free on all paths; the sigmask/setsid parity with C is
  exact (modulo GES-C13).
* **The bring-up order** (flush → init_failed → socket → xwayland → wake)
  matches `main.c:4671+` with the shell-first divergence deliberate and
  reasoned; PR16-C6/PR20-C1/PR36-C1/PR19-C6 fixes all verified real.
* **The `bool**` UB claim** in C (`shell.c`), the C unbounded activation
  recursion, and the no-outputs wild-pointer screenshooter fix: all
  verified real in the C sources — the port genuinely fixes C defects.
* **The review paper trail itself**: every "fixed in #NN" sampled (six of
  ten) was true as claimed, with tests where promised. `PROVENANCE.md` and
  `VENDOR.md` check out.
* **The fence dependency-graph check** exists, runs, fails on unclassified
  crates, and passes; clippy `-D warnings` + rustfmt + the forbid greps all
  green locally.
* Shell policy parity spot-checks (resize-commit offset signs, focus
  bookkeeping, placement math, first-output refit, ping-timeout subsurface
  divergence): all match C or carry documented, tested divergences.
* Zero `TODO`/`FIXME` in the Rust tree; all `unwrap_used` allows are
  test-scoped; libinput config logging is punctuation-faithful to C.

## 7. Recommended order of work

1. **GES-C1 + GES-C2** — install the exit hook, propagate the exit code,
   and add a backend-initiated-exit test. Small, mechanical, closes a
   user-visible crash and a falsified parity claim.
2. **GES-C3 + GES-C4** — restore the fence's actual contract (re-resolve
   after drain; `unsafe fn shell_init`), then **GES-T9** (wire
   cargo-public-api) so the contract stays restored.
3. **GES-C5/C8/C14/C15** — one config-surface sweep: apply or refuse every
   parsed key, make range overflow loud. The fail-loud rule is the port's
   best idea; these are its four violations.
4. **GES-C6/C16** — fix the rerun-if-changed set; pin or single-build the
   CI container; consider recording the pkg-config *release* alongside
   modversion.
5. **GES-S1** — mechanize the entry-point audit as fence 4. Cheap, and it
   converts the fence's central manual discipline into a gate.
6. **GES-T1–T4** — grab/touch/track-untrack/transient coverage, using the
   existing testsupport harness where possible.
7. The §10 consolidation (GES-S2) and the drift sweep (GES-S3) in one
   documentation pass; the rest of §4 as convenient.

---

*Method note: four independent deep-read passes (fence core; fence
peripherals; weston-sys/spawn/plugin/build-system; frontend/config; shell +
test story) were cross-checked against each other and against execution in
a from-source libweston 14.0.1 sandbox; every finding above was verified at
the cited lines by a second pass before inclusion, and the two most severe
findings (GES-C1, GES-C3) were confirmed independently by two passes with
different scopes. Findings the passes proposed that did NOT survive
verification were dropped.*
